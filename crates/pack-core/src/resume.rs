//! Host side of the pending/resume async-ABI (`docs/async-abi.md`).
//!
//! The guest owns an executor and suspends *itself*: a host import poll-returns a
//! tag (`READY` with the result, or `PENDING` with a `completion_id`), and the
//! host later re-enters the guest via `__pack_resume` when the corresponding host
//! future resolves. All awaiting happens here, in the **pump**, *between* guest
//! re-entries — so async host calls work on any engine, no engine suspension.
//!
//! This module is the backend-free core of that: the [`CompletionRegistry`] of
//! in-flight host futures, and [`drive`] — the pump loop that runs an export to
//! completion by resolving pending host calls and resuming the guest. It is
//! written against the [`ResumeTarget`] seam so it can be unit-tested with a mock
//! guest (below) before any real guest executor or backend wiring exists.

use std::collections::HashMap;
use std::task::Poll;

use packr_abi::Value;

use crate::abi_call::CallError;
use crate::backend::EngineError;
use crate::host::{BoxFuture, HostError};

/// Wire tag shared by guest imports and by the export/`__pack_resume` path.
/// Imports: `READY` → result in the slots; `PENDING` → `completion_id` in the
/// out-ptr slot. Exports: `READY` doubles as `DONE`; `PENDING` → in progress.
pub const TAG_ERROR: i32 = -1;
pub const TAG_READY: i32 = 0;
pub const TAG_PENDING: i32 = 1;

/// Registry of in-flight host futures, keyed by a `completion_id` the guest holds
/// while its task is parked. Populated (poll-once dispatch stashes a not-yet-ready
/// host future) as a side effect of a guest call; drained by the pump.
#[derive(Default)]
pub struct CompletionRegistry {
    next_id: u32,
    pending: HashMap<u32, BoxFuture<'static, Result<Value, HostError>>>,
}

impl CompletionRegistry {
    pub fn new() -> Self {
        Self {
            next_id: 1,
            pending: HashMap::new(),
        }
    }

    /// Stash a not-yet-ready host future and return its `completion_id`.
    pub fn stash(&mut self, fut: BoxFuture<'static, Result<Value, HostError>>) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        self.pending.insert(id, fut);
        id
    }

    /// Number of in-flight completions.
    pub fn len(&self) -> usize {
        self.pending.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// Await the *next* in-flight future to resolve — joining over ALL of them,
    /// so whichever host call finishes first is delivered first (an actor task
    /// may have several outstanding at once). Returns `None` if the registry is
    /// empty (a deadlock: the guest is parked but nothing is in flight).
    pub async fn await_next(&mut self) -> Option<(u32, Result<Value, HostError>)> {
        if self.pending.is_empty() {
            return None;
        }
        let pending = &mut self.pending;
        std::future::poll_fn(move |cx| {
            let mut done = None;
            for (id, fut) in pending.iter_mut() {
                if let Poll::Ready(result) = fut.as_mut().poll(cx) {
                    done = Some((*id, result));
                    break;
                }
            }
            match done {
                Some((id, result)) => {
                    pending.remove(&id);
                    Poll::Ready(Some((id, result)))
                }
                None => Poll::Pending,
            }
        })
        .await
    }
}

/// What the pump drives: the guest, as seen from the host. A real implementation
/// (packr-guest executor + a backend) runs the guest export / `__pack_resume` and
/// stashes any host futures the guest's imports raised into `registry`. Modeled
/// as a trait so the pump ([`drive`]) is testable with a mock guest.
///
/// Each method returns `(tag, output)`: `tag == TAG_READY` means DONE and
/// `output` is `Some(result)`; `tag == TAG_PENDING` means the guest parked and
/// `output` is `None`.
#[allow(async_fn_in_trait)]
pub trait ResumeTarget {
    /// Run the export entry to its first parking point (or completion). May stash
    /// host futures into `registry` (the guest's imports, dispatched poll-once).
    async fn call_entry(
        &mut self,
        name: &str,
        input: &Value,
        registry: &mut CompletionRegistry,
    ) -> Result<(i32, Option<Value>), EngineError>;

    /// Re-enter the guest at `__pack_resume` with a resolved host result. May
    /// stash further host futures (the resumed task can make new host calls).
    async fn resume(
        &mut self,
        completion_id: u32,
        result: Value,
        registry: &mut CompletionRegistry,
    ) -> Result<(i32, Option<Value>), EngineError>;
}

/// The pump: run an export to completion under the pending/resume protocol.
/// Call the entry; while the guest is parked, resolve the next in-flight host
/// call and resume the guest with it; repeat until the top-level task is DONE.
pub async fn drive<T: ResumeTarget>(
    target: &mut T,
    name: &str,
    input: &Value,
) -> Result<Value, CallError> {
    let mut registry = CompletionRegistry::new();
    let (mut tag, mut output) = target.call_entry(name, input, &mut registry).await?;

    loop {
        match tag {
            TAG_READY => {
                return output.ok_or_else(|| {
                    CallError::Abi("guest reported DONE without an output value".into())
                });
            }
            TAG_PENDING => {
                let (completion_id, result) = registry.await_next().await.ok_or_else(|| {
                    CallError::Abi(
                        "guest parked but no host call is in flight (resume deadlock)".into(),
                    )
                })?;
                let value = result.map_err(|e| CallError::Guest {
                    name: name.to_string(),
                    message: e.to_string(),
                })?;
                let (t, o) = target.resume(completion_id, value, &mut registry).await?;
                tag = t;
                output = o;
            }
            other => {
                return Err(CallError::Guest {
                    name: name.to_string(),
                    message: format!("guest returned error tag {other}"),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready(v: Value) -> BoxFuture<'static, Result<Value, HostError>> {
        Box::pin(async move { Ok(v) })
    }

    /// A mock guest: `process(n)` makes `n_calls` host calls that each return
    /// `k`, sums them with the input, and finishes. It parks on the first call,
    /// then completes on the last resume — exercising the full pump loop and the
    /// concurrent-completion join.
    struct MockGuest {
        n_calls: usize,
        per_call: i64,
        started: bool,
        acc: i64,
        remaining: usize,
    }

    impl ResumeTarget for MockGuest {
        async fn call_entry(
            &mut self,
            _name: &str,
            input: &Value,
            registry: &mut CompletionRegistry,
        ) -> Result<(i32, Option<Value>), EngineError> {
            self.acc = match input {
                Value::S64(n) => *n,
                _ => 0,
            };
            self.started = true;
            self.remaining = self.n_calls;
            if self.n_calls == 0 {
                return Ok((TAG_READY, Some(Value::S64(self.acc))));
            }
            // The guest fires ALL its host calls up front (they're concurrent),
            // then parks waiting for the first to resolve.
            for _ in 0..self.n_calls {
                registry.stash(ready(Value::S64(self.per_call)));
            }
            Ok((TAG_PENDING, None))
        }

        async fn resume(
            &mut self,
            _completion_id: u32,
            result: Value,
            _registry: &mut CompletionRegistry,
        ) -> Result<(i32, Option<Value>), EngineError> {
            if let Value::S64(k) = result {
                self.acc += k;
            }
            self.remaining -= 1;
            if self.remaining == 0 {
                Ok((TAG_READY, Some(Value::S64(self.acc))))
            } else {
                Ok((TAG_PENDING, None))
            }
        }
    }

    #[tokio::test]
    async fn drives_a_single_pending_host_call() {
        let mut guest = MockGuest {
            n_calls: 1,
            per_call: 10,
            started: false,
            acc: 0,
            remaining: 0,
        };
        // process(5) with one host call returning 10 -> 15.
        let out = drive(&mut guest, "process", &Value::S64(5)).await.unwrap();
        assert_eq!(out, Value::S64(15));
    }

    #[tokio::test]
    async fn drives_multiple_concurrent_completions() {
        let mut guest = MockGuest {
            n_calls: 3,
            per_call: 7,
            started: false,
            acc: 0,
            remaining: 0,
        };
        // process(1) + 3 host calls of 7 each -> 1 + 21 = 22. Exercises the pump
        // draining a registry with several completions in flight at once.
        let out = drive(&mut guest, "process", &Value::S64(1)).await.unwrap();
        assert_eq!(out, Value::S64(22));
    }

    #[tokio::test]
    async fn synchronous_guest_never_parks() {
        let mut guest = MockGuest {
            n_calls: 0,
            per_call: 0,
            started: false,
            acc: 0,
            remaining: 0,
        };
        let out = drive(&mut guest, "process", &Value::S64(42)).await.unwrap();
        assert_eq!(out, Value::S64(42));
    }

    #[tokio::test]
    async fn empty_registry_while_pending_is_a_deadlock_error() {
        struct Stuck;
        impl ResumeTarget for Stuck {
            async fn call_entry(
                &mut self,
                _: &str,
                _: &Value,
                _: &mut CompletionRegistry,
            ) -> Result<(i32, Option<Value>), EngineError> {
                // Parks but stashes nothing — nothing can ever resume it.
                Ok((TAG_PENDING, None))
            }
            async fn resume(
                &mut self,
                _: u32,
                _: Value,
                _: &mut CompletionRegistry,
            ) -> Result<(i32, Option<Value>), EngineError> {
                Ok((TAG_READY, Some(Value::S64(0))))
            }
        }
        let err = drive(&mut Stuck, "x", &Value::S64(0)).await.unwrap_err();
        assert!(matches!(err, CallError::Abi(_)));
    }
}
