//! Guest-side executor for the pending/resume async-ABI (`docs/async-abi.md`,
//! model B).
//!
//! The guest owns this single-threaded executor and suspends *itself*: when an
//! actor `.await`s a host call, the call either returns a result synchronously
//! (`READY`) or a `completion_id` (`PENDING`). On `PENDING` the task parks; the
//! host later re-enters via `resume(completion_id, value)`, which deposits the
//! result and re-polls the one in-flight task. No engine stack-suspension, so
//! this works on any wasm engine.
//!
//! This module is the backend/glue-free **core**: the [`Executor`], the
//! [`Park`] future, and the [`RawCaller`] seam that abstracts the actual host
//! import (a wasm extern in production; a mock in tests). The wasm extern glue
//! (`__pack_resume`, the reshaped export/import wrappers) wraps a global
//! `Executor` and is a later increment.

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::rc::Rc;
use core::cell::RefCell;
use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

use packr_abi::{decode, encode, Value};

/// Wire tags (mirror `packr_core::resume`). Imports return one of these; the
/// export entry / `__pack_resume` return `READY`(=DONE) / `PENDING` / `ERROR`.
pub const TAG_ERROR: i32 = -1;
pub const TAG_READY: i32 = 0;
pub const TAG_PENDING: i32 = 1;

/// The outcome of a raw host-import call under the pending/resume ABI.
pub enum HostOutcome {
    /// The host had the result synchronously — no parking.
    Ready(Value),
    /// The host started async work; re-entry will arrive under this id.
    Pending(u32),
}

/// Makes the actual host-import call. In production this is a wasm extern that
/// marshals `input` and reads the `READY`/`PENDING(completion_id)` tag; in tests
/// it's a mock. Keeping it a seam is what lets the executor be tested natively.
pub trait RawCaller {
    fn call(&self, interface: &str, function: &str, input: Value) -> HostOutcome;
}

/// Shared executor state the parked futures also hold: `completion_id -> result`,
/// filled by [`Executor::resume`] and drained by [`Park`].
#[derive(Default)]
struct Inner {
    results: BTreeMap<u32, Value>,
}

/// A handle to the shared executor state, held by both the executor and every
/// parked [`Park`] future. Single-threaded (guest wasm), so `Rc<RefCell<_>>`.
#[derive(Clone, Default)]
pub struct Runtime(Rc<RefCell<Inner>>);

impl Runtime {
    pub fn new() -> Self {
        Self::default()
    }

    fn take_result(&self, id: u32) -> Option<Value> {
        self.0.borrow_mut().results.remove(&id)
    }

    fn deposit(&self, id: u32, value: Value) {
        self.0.borrow_mut().results.insert(id, value);
    }
}

/// Await a host call: on the first poll it makes the raw call; if `PENDING` it
/// parks under the returned `completion_id` and resolves once the result is
/// deposited. `caller` is only touched on the first poll (before any await), so
/// no borrow is held across a suspension.
pub async fn host_call<C: RawCaller>(
    runtime: Runtime,
    caller: &C,
    interface: &str,
    function: &str,
    input: Value,
) -> Value {
    match caller.call(interface, function, input) {
        HostOutcome::Ready(v) => v,
        HostOutcome::Pending(id) => Park { runtime, id }.await,
    }
}

/// The parked half of a pending host call: resolves when its `completion_id`'s
/// result has been deposited by the host via [`Executor::resume`].
struct Park {
    runtime: Runtime,
    id: u32,
}

impl Future for Park {
    type Output = Value;

    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Value> {
        match self.runtime.take_result(self.id) {
            Some(v) => Poll::Ready(v),
            None => Poll::Pending,
        }
    }
}

/// The single-threaded guest executor. Holds the one in-flight actor task (per
/// export invocation); driven by the host across `poll`/`resume` re-entries.
pub struct Executor {
    runtime: Runtime,
    task: Option<Pin<Box<dyn Future<Output = Value>>>>,
}

impl Executor {
    pub fn new(runtime: Runtime) -> Self {
        Self {
            runtime,
            task: None,
        }
    }

    pub fn runtime(&self) -> Runtime {
        self.runtime.clone()
    }

    /// Install the actor task for this export invocation.
    pub fn spawn(&mut self, task: Pin<Box<dyn Future<Output = Value>>>) {
        self.task = Some(task);
    }

    /// Poll the in-flight task once. `Ready(value)` = the export is DONE and the
    /// task is cleared; `Pending` = it parked on one or more host calls.
    pub fn poll(&mut self) -> Poll<Value> {
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);
        let Some(task) = self.task.as_mut() else {
            return Poll::Pending;
        };
        match task.as_mut().poll(&mut cx) {
            Poll::Ready(value) => {
                self.task = None;
                Poll::Ready(value)
            }
            Poll::Pending => Poll::Pending,
        }
    }

    /// Deliver a resolved host result and re-poll the task. The parked future
    /// keyed by `completion_id` will observe its result on this re-poll.
    pub fn resume(&mut self, completion_id: u32, value: Value) -> Poll<Value> {
        self.runtime.deposit(completion_id, value);
        self.poll()
    }
}

/// A [`RawCaller`] that drives a real wasm host-import function of the shape
/// `(in_ptr, in_len, out_ptr_slot, out_len_slot) -> tag`. Marshals `input`,
/// then interprets the tag: `READY` → decode the result from the slots (and free
/// the guest-owned buffer); `PENDING` → the `completion_id` is the value the host
/// wrote into the out-ptr slot.
pub struct WasmCaller<F: Fn(i32, i32, i32, i32) -> i32> {
    raw: F,
}

impl<F: Fn(i32, i32, i32, i32) -> i32> WasmCaller<F> {
    pub fn new(raw: F) -> Self {
        Self { raw }
    }
}

impl<F: Fn(i32, i32, i32, i32) -> i32> RawCaller for WasmCaller<F> {
    fn call(&self, _interface: &str, _function: &str, input: Value) -> HostOutcome {
        let input_bytes = encode(&input).expect("encode host-call input");
        let mut out_ptr: i32 = 0;
        let mut out_len: i32 = 0;
        let tag = (self.raw)(
            input_bytes.as_ptr() as i32,
            input_bytes.len() as i32,
            &mut out_ptr as *mut i32 as i32,
            &mut out_len as *mut i32 as i32,
        );
        match tag {
            TAG_PENDING => HostOutcome::Pending(out_ptr as u32),
            TAG_READY => {
                let value = {
                    let bytes = unsafe {
                        core::slice::from_raw_parts(out_ptr as *const u8, out_len as usize)
                    };
                    decode(bytes).expect("decode host-call result")
                };
                crate::__pack_free(out_ptr, out_len);
                HostOutcome::Ready(value)
            }
            other => panic!("host import returned error tag {other}"),
        }
    }
}

/// Marshal a completed export result into the ABI out-slots (encode, leak, write
/// ptr/len) and return `TAG_READY`; or return `TAG_PENDING` if the task parked.
/// The host frees the output buffer via `__pack_free`.
fn finish(poll: Poll<Value>, out_ptr_ptr: i32, out_len_ptr: i32) -> i32 {
    match poll {
        Poll::Ready(value) => {
            let mut bytes = match encode(&value) {
                Ok(b) => b,
                Err(_) => return TAG_ERROR,
            };
            bytes.shrink_to_fit();
            let ptr = bytes.as_ptr() as i32;
            let len = bytes.len() as i32;
            core::mem::forget(bytes);
            unsafe {
                core::ptr::write(out_ptr_ptr as *mut i32, ptr);
                core::ptr::write(out_len_ptr as *mut i32, len);
            }
            TAG_READY
        }
        Poll::Pending => TAG_PENDING,
    }
}

/// Run an export: install `task`, poll the executor once, and marshal the result
/// (or `PENDING`) to the out-slots. This is the model-B export entry.
pub fn run_export(
    exec: &mut Executor,
    task: Pin<Box<dyn Future<Output = Value>>>,
    out_ptr_ptr: i32,
    out_len_ptr: i32,
) -> i32 {
    exec.spawn(task);
    finish(exec.poll(), out_ptr_ptr, out_len_ptr)
}

/// Re-enter after a host future resolved: decode the delivered result bytes,
/// resume the parked task, and marshal the outcome. This is `__pack_resume`.
pub fn run_resume(
    exec: &mut Executor,
    completion_id: u32,
    result_ptr: i32,
    result_len: i32,
    out_ptr_ptr: i32,
    out_len_ptr: i32,
) -> i32 {
    let value = {
        let bytes =
            unsafe { core::slice::from_raw_parts(result_ptr as *const u8, result_len as usize) };
        match decode(bytes) {
            Ok(v) => v,
            Err(_) => return TAG_ERROR,
        }
    };
    finish(exec.resume(completion_id, value), out_ptr_ptr, out_len_ptr)
}

// ---------------------------------------------------------------------------
// Global executor — the single per-instance executor the macros drive through
// (so `#[async_export]`/`#[async_import]`/`setup_async_guest!` reference only
// `packr_guest::executor::` paths, never fragile cross-macro identifiers).
// Single-threaded wasm → `static mut` is sound.
// ---------------------------------------------------------------------------

static mut GLOBAL_EXEC: Option<Executor> = None;

fn global_exec() -> &'static mut Executor {
    unsafe {
        (*core::ptr::addr_of_mut!(GLOBAL_EXEC)).get_or_insert_with(|| Executor::new(Runtime::new()))
    }
}

/// The global executor's runtime — used by `#[async_import]` shims for `host_call`.
pub fn global_runtime() -> Runtime {
    global_exec().runtime()
}

/// Run an export on the global executor. Used by `#[async_export]`.
pub fn run_export_global(
    task: Pin<Box<dyn Future<Output = Value>>>,
    out_ptr_ptr: i32,
    out_len_ptr: i32,
) -> i32 {
    run_export(global_exec(), task, out_ptr_ptr, out_len_ptr)
}

/// Re-enter the global executor. Used by the `__pack_resume` export.
pub fn run_resume_global(
    completion_id: u32,
    result_ptr: i32,
    result_len: i32,
    out_ptr_ptr: i32,
    out_len_ptr: i32,
) -> i32 {
    run_resume(
        global_exec(),
        completion_id,
        result_ptr,
        result_len,
        out_ptr_ptr,
        out_len_ptr,
    )
}

/// A no-op `Waker`: the executor re-polls the whole task on every `resume`, so
/// individual futures don't need to schedule themselves.
fn noop_waker() -> Waker {
    fn no_op(_: *const ()) {}
    fn clone(_: *const ()) -> RawWaker {
        RawWaker::new(core::ptr::null(), &VTABLE)
    }
    static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, no_op, no_op, no_op);
    // SAFETY: the vtable's fns are all no-ops / return an equivalent RawWaker.
    unsafe { Waker::from_raw(RawWaker::new(core::ptr::null(), &VTABLE)) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::cell::Cell;

    /// A mock host: hands out incrementing completion_ids and records how many
    /// calls it saw. Every call is `PENDING` (forces the park/resume path).
    struct MockHost {
        next_id: Cell<u32>,
        calls: Cell<usize>,
    }
    impl MockHost {
        fn new() -> Self {
            Self {
                next_id: Cell::new(1),
                calls: Cell::new(0),
            }
        }
    }
    impl RawCaller for MockHost {
        fn call(&self, _interface: &str, _function: &str, _input: Value) -> HostOutcome {
            self.calls.set(self.calls.get() + 1);
            let id = self.next_id.get();
            self.next_id.set(id + 1);
            HostOutcome::Pending(id)
        }
    }

    /// A mock host that answers the first call synchronously (READY).
    struct SyncHost;
    impl RawCaller for SyncHost {
        fn call(&self, _: &str, _: &str, _: Value) -> HostOutcome {
            HostOutcome::Ready(Value::S64(99))
        }
    }

    #[test]
    fn single_pending_host_call_parks_then_resumes() {
        let rt = Runtime::new();
        let mut exec = Executor::new(rt.clone());
        // The mock host must outlive the task; leak it (test-only) so the task's
        // borrow is 'static, matching the boxed dyn Future.
        let host: &'static MockHost = Box::leak(Box::new(MockHost::new()));

        let rt2 = rt.clone();
        exec.spawn(Box::pin(async move {
            let a = host_call(rt2, host, "math", "double", Value::S64(5)).await;
            match a {
                Value::S64(n) => Value::S64(n + 1),
                other => other,
            }
        }));

        // First poll: the task fires its host call, gets PENDING, and parks.
        assert_eq!(exec.poll(), Poll::Pending);
        assert_eq!(host.calls.get(), 1);
        // Host resolves completion_id 1 with double(5)=10.
        assert_eq!(exec.resume(1, Value::S64(10)), Poll::Ready(Value::S64(11)));
    }

    #[test]
    fn sequential_host_calls_park_and_resume_twice() {
        let rt = Runtime::new();
        let mut exec = Executor::new(rt.clone());
        let host: &'static MockHost = Box::leak(Box::new(MockHost::new()));

        let rt2 = rt.clone();
        exec.spawn(Box::pin(async move {
            let a = host_call(rt2.clone(), host, "m", "f", Value::S64(0)).await;
            let b = host_call(rt2, host, "m", "g", Value::S64(0)).await;
            match (a, b) {
                (Value::S64(x), Value::S64(y)) => Value::S64(x + y),
                _ => Value::S64(-1),
            }
        }));

        assert_eq!(exec.poll(), Poll::Pending); // parks on first call (id 1)
        assert_eq!(exec.resume(1, Value::S64(3)), Poll::Pending); // now parks on second (id 2)
        assert_eq!(exec.resume(2, Value::S64(4)), Poll::Ready(Value::S64(7)));
        assert_eq!(host.calls.get(), 2);
    }

    #[test]
    fn synchronous_host_call_never_parks() {
        let rt = Runtime::new();
        let mut exec = Executor::new(rt.clone());
        let host: &'static SyncHost = Box::leak(Box::new(SyncHost));

        let rt2 = rt.clone();
        exec.spawn(Box::pin(async move {
            host_call(rt2, host, "m", "f", Value::S64(0)).await
        }));

        // READY on the first poll → completes immediately, no resume needed.
        assert_eq!(exec.poll(), Poll::Ready(Value::S64(99)));
    }
}
