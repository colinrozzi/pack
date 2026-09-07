//! A test actor that makes a genuinely ASYNC host call, exercising the
//! pending/resume async-ABI (`docs/async-abi.md`) end to end.
//!
//! `process(n) = double(n) + 1`, where `double` is an **async host import**: the
//! guest task parks on it (the executor suspends *itself*) and the host resumes
//! it via `__pack_resume` once the host future resolves. Hand-written glue over
//! `packr_guest::executor` (the async `#[export]`/`#[import]` macros come later).

#![no_std]

extern crate alloc;

use alloc::boxed::Box;

use packr_guest::executor::{host_call, run_export, run_resume, Executor, Runtime, WasmCaller};
use packr_guest::{decode, Value};

packr_guest::setup_guest!();

// Host import `math.double` in the pack host-import ABI shape.
#[link(wasm_import_module = "math")]
extern "C" {
    #[link_name = "double"]
    fn raw_double(in_ptr: i32, in_len: i32, out_ptr_slot: i32, out_len_slot: i32) -> i32;
}

// The single per-instance executor. Single-threaded wasm → `static mut` is safe.
static mut EXEC: Option<Executor> = None;

fn executor() -> &'static mut Executor {
    unsafe {
        let slot = &mut *core::ptr::addr_of_mut!(EXEC);
        slot.get_or_insert_with(|| Executor::new(Runtime::new()))
    }
}

/// Export entry: build the actor task and run the executor to its first park (or
/// completion). Returns the pending/resume tag.
#[no_mangle]
pub extern "C" fn process(in_ptr: i32, in_len: i32, out_ptr_ptr: i32, out_len_ptr: i32) -> i32 {
    let input = {
        let bytes = unsafe { core::slice::from_raw_parts(in_ptr as *const u8, in_len as usize) };
        match decode(bytes) {
            Ok(v) => v,
            Err(_) => return -1,
        }
    };
    let exec = executor();
    let runtime = exec.runtime();
    let task = Box::pin(adder(runtime, input));
    run_export(exec, task, out_ptr_ptr, out_len_ptr)
}

/// Host re-entry with a resolved host result; re-polls the parked task.
#[no_mangle]
pub extern "C" fn __pack_resume(
    completion_id: i32,
    result_ptr: i32,
    result_len: i32,
    out_ptr_ptr: i32,
    out_len_ptr: i32,
) -> i32 {
    run_resume(
        executor(),
        completion_id as u32,
        result_ptr,
        result_len,
        out_ptr_ptr,
        out_len_ptr,
    )
}

/// The actor logic: await the async host call `math.double`, then add one.
async fn adder(runtime: Runtime, input: Value) -> Value {
    let caller = WasmCaller::new(|a, b, c, d| unsafe { raw_double(a, b, c, d) });
    let doubled = host_call(runtime, &caller, "math", "double", input).await;
    match doubled {
        Value::S64(n) => Value::S64(n + 1),
        other => other,
    }
}
