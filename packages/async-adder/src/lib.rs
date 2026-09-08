//! A test actor that makes a genuinely ASYNC host call, exercising the
//! pending/resume async-ABI (`docs/async-abi.md`) end to end — now written with
//! the `#[async_export]` / `#[async_import]` macros instead of hand-rolled glue.
//!
//! `process(n) = double(n) + 1`, where `double` is an **async host import**.

#![no_std]

extern crate alloc;

use packr_guest::Value;

packr_guest::setup_async_guest!();

// Host import `math.double` — the macro generates the raw wasm binding + the
// async shim that parks the task until the host resumes it. (Body is discarded.)
#[packr_guest::async_import(module = "math", name = "double")]
async fn double(_input: Value) -> Value {}

// Export `process` — the macro generates the model-B entry (decode, spawn on the
// executor, marshal DONE/PENDING). The body may `.await` async imports.
#[packr_guest::async_export]
async fn process(input: Value) -> Value {
    match double(input).await {
        Value::S64(n) => Value::S64(n + 1),
        other => other,
    }
}
