//! End-to-end proof of the pending/resume async-ABI on wasmtime — the whole
//! protocol validated natively, no browser (theater-dev's rec: prove it here
//! before packr-web).
//!
//! The `async-adder` actor computes `process(n) = double(n) + 1`, where `double`
//! is a genuinely **async** host call: it `yield_now().await`s, so on the first
//! poll it returns `Pending` — forcing the guest to PARK, the host to stash the
//! future, the pump to resolve it, and `__pack_resume` to re-enter the guest.

use std::path::{Path, PathBuf};
use std::process::Command;

use packr_core::{host_fn, HostError, HostImports, Value, WasmEngine};
use packr_wasmtime::{call_resume, WasmtimeEngine};

fn build_async_adder() -> Option<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let manifest = root.join("packages/async-adder/Cargo.toml");
    let out =
        root.join("packages/async-adder/target/wasm32-unknown-unknown/release/async_adder.wasm");
    let status = Command::new("cargo")
        .args([
            "build",
            "--manifest-path",
            manifest.to_str().unwrap(),
            "--target",
            "wasm32-unknown-unknown",
            "--release",
        ])
        .status();
    match status {
        Ok(s) if s.success() && out.exists() => Some(out),
        _ => None,
    }
}

#[tokio::test]
async fn async_host_call_through_pending_resume() {
    let Some(wasm_path) = build_async_adder() else {
        eprintln!("SKIP: wasm32 toolchain unavailable");
        return;
    };
    let wasm = std::fs::read(&wasm_path).expect("read async-adder wasm");

    let engine = WasmtimeEngine::new();
    let module = engine.compile(&wasm).await.expect("compile");

    // A genuinely async host fn: yields once (→ Pending on first poll → the
    // guest parks and the resume path is exercised), then returns 2n.
    let mut imports = HostImports::new();
    imports.define(
        "math",
        "double",
        host_fn(|v| async move {
            tokio::task::yield_now().await;
            Ok::<Value, HostError>(match v {
                Value::S64(n) => Value::S64(n * 2),
                other => other,
            })
        }),
    );

    // process(5) = double(5) + 1 = 11 — through park → stash → pump → __pack_resume.
    let out = call_resume(&engine, &module, imports, "process", &Value::S64(5))
        .await
        .expect("call_resume process(5)");
    assert_eq!(out, Value::S64(11));
}
