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
/// The (at most one) in-flight host future. Actors are **sequential** (a task has
/// ≤1 host call outstanding — see the project decision), so this is a single slot,
/// not a map: no concurrency machinery, and a second concurrent stash is a bug
/// that would corrupt replay ordering, so it panics loudly rather than silently.
#[derive(Default)]
pub struct CompletionRegistry {
    pending: Option<(u32, BoxFuture<'static, Result<Value, HostError>>)>,
}

impl CompletionRegistry {
    pub fn new() -> Self {
        Self { pending: None }
    }

    /// Stash the in-flight host future under the `completion_id` the backend
    /// assigned. **Panics** if one is already outstanding — an actor may have at
    /// most one in-flight host call (an accidental `join!` over host calls is
    /// forbidden; concurrency would break replay's issue-order matching).
    pub fn insert(&mut self, id: u32, fut: BoxFuture<'static, Result<Value, HostError>>) {
        assert!(
            self.pending.is_none(),
            "concurrent host call: an actor may have at most one in-flight host \
             call (actors are sequential)"
        );
        self.pending = Some((id, fut));
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_none()
    }

    /// Await the in-flight host future to resolve. `None` if nothing is in flight
    /// (a deadlock: the guest parked but issued no host call).
    pub async fn await_next(&mut self) -> Option<(u32, Result<Value, HostError>)> {
        match self.pending.take() {
            Some((id, fut)) => Some((id, fut.await)),
            None => None,
        }
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

    /// A mock guest: `process(n)` makes `n_calls` **sequential** host calls that
    /// each return `per_call`, sums them with the input, and finishes. Each call
    /// parks (one in flight), the resume adds it and issues the next — exercising
    /// the full pump loop the sequential way (never more than one outstanding).
    struct MockGuest {
        n_calls: usize,
        per_call: i64,
        acc: i64,
        remaining: usize,
        next_id: u32,
    }

    impl MockGuest {
        fn new(n_calls: usize, per_call: i64) -> Self {
            Self {
                n_calls,
                per_call,
                acc: 0,
                remaining: 0,
                next_id: 1,
            }
        }

        fn issue(&mut self, registry: &mut CompletionRegistry) {
            let id = self.next_id;
            self.next_id += 1;
            registry.insert(id, ready(Value::S64(self.per_call)));
        }
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
            self.remaining = self.n_calls;
            if self.n_calls == 0 {
                return Ok((TAG_READY, Some(Value::S64(self.acc))));
            }
            self.issue(registry); // one call in flight
            Ok((TAG_PENDING, None))
        }

        async fn resume(
            &mut self,
            _completion_id: u32,
            result: Value,
            registry: &mut CompletionRegistry,
        ) -> Result<(i32, Option<Value>), EngineError> {
            if let Value::S64(k) = result {
                self.acc += k;
            }
            self.remaining -= 1;
            if self.remaining == 0 {
                Ok((TAG_READY, Some(Value::S64(self.acc))))
            } else {
                self.issue(registry); // issue the NEXT, one at a time
                Ok((TAG_PENDING, None))
            }
        }
    }

    #[tokio::test]
    async fn drives_a_single_pending_host_call() {
        let mut guest = MockGuest::new(1, 10);
        // process(5) with one host call returning 10 -> 15.
        let out = drive(&mut guest, "process", &Value::S64(5)).await.unwrap();
        assert_eq!(out, Value::S64(15));
    }

    #[tokio::test]
    async fn drives_sequential_host_calls() {
        let mut guest = MockGuest::new(3, 7);
        // process(1) + 3 sequential host calls of 7 -> 1 + 21 = 22. One in flight
        // at a time; exercises the resume loop without any concurrency.
        let out = drive(&mut guest, "process", &Value::S64(1)).await.unwrap();
        assert_eq!(out, Value::S64(22));
    }

    #[tokio::test]
    async fn synchronous_guest_never_parks() {
        let mut guest = MockGuest::new(0, 0);
        let out = drive(&mut guest, "process", &Value::S64(42)).await.unwrap();
        assert_eq!(out, Value::S64(42));
    }

    #[tokio::test]
    #[should_panic(expected = "concurrent host call")]
    async fn concurrent_host_calls_panic() {
        // A misbehaving guest that stashes two host calls at once (an accidental
        // `join!`) must trip the sequential guard, not silently corrupt replay.
        struct Concurrent;
        impl ResumeTarget for Concurrent {
            async fn call_entry(
                &mut self,
                _: &str,
                _: &Value,
                registry: &mut CompletionRegistry,
            ) -> Result<(i32, Option<Value>), EngineError> {
                registry.insert(1, ready(Value::S64(1)));
                registry.insert(2, ready(Value::S64(2))); // second in flight → panic
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
        let _ = drive(&mut Concurrent, "x", &Value::S64(0)).await;
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
