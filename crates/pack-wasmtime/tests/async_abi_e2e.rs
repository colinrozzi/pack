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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use packr_core::{host_fn, CallInterceptor, HostError, HostImports, Value, WasmEngine};
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

/// Records every `after_import` it sees.
#[derive(Default)]
struct Recorder {
    imports: Mutex<Vec<(String, Value, Value)>>, // (interface/function, input, output)
}

#[async_trait]
impl CallInterceptor for Recorder {
    async fn before_import(&self, _: &str, _: &str, _: &Value) -> Option<Value> {
        None
    }
    async fn after_import(&self, interface: &str, function: &str, input: &Value, output: &Value) {
        self.imports.lock().unwrap().push((
            format!("{interface}/{function}"),
            input.clone(),
            output.clone(),
        ));
    }
    async fn before_export(&self, _: &str, _: &Value) -> Option<Value> {
        None
    }
    async fn after_export(&self, _: &str, _: &Value, _: &Value) {}
}

/// The interceptor sees the async host call at resume time (record path).
#[tokio::test]
async fn interceptor_records_the_host_call() {
    let Some(wasm_path) = build_async_adder() else {
        eprintln!("SKIP: wasm32 toolchain unavailable");
        return;
    };
    let wasm = std::fs::read(&wasm_path).expect("read async-adder wasm");
    let engine = WasmtimeEngine::new();
    let module = engine.compile(&wasm).await.expect("compile");

    let recorder = Arc::new(Recorder::default());
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
    imports.with_interceptor(recorder.clone());

    let out = call_resume(&engine, &module, imports, "process", &Value::S64(5))
        .await
        .expect("call_resume");
    assert_eq!(out, Value::S64(11));

    let calls = recorder.imports.lock().unwrap();
    assert_eq!(calls.len(), 1, "one host call recorded");
    assert_eq!(
        calls[0],
        ("math/double".to_string(), Value::S64(5), Value::S64(10)),
        "recorded (input 5 -> output 10) at resume time"
    );
}

/// A replay interceptor: `before_import` returns the recorded value, so the host
/// fn is never invoked.
struct Replay(Value);

#[async_trait]
impl CallInterceptor for Replay {
    async fn before_import(&self, _: &str, _: &str, _: &Value) -> Option<Value> {
        Some(self.0.clone())
    }
    async fn after_import(&self, _: &str, _: &str, _: &Value, _: &Value) {}
    async fn before_export(&self, _: &str, _: &Value) -> Option<Value> {
        None
    }
    async fn after_export(&self, _: &str, _: &Value, _: &Value) {}
}

/// On replay the host fn must not run — `before_import` short-circuits to the
/// recorded value.
#[tokio::test]
async fn replay_short_circuits_the_host_fn() {
    let Some(wasm_path) = build_async_adder() else {
        eprintln!("SKIP: wasm32 toolchain unavailable");
        return;
    };
    let wasm = std::fs::read(&wasm_path).expect("read async-adder wasm");
    let engine = WasmtimeEngine::new();
    let module = engine.compile(&wasm).await.expect("compile");

    let called = Arc::new(AtomicBool::new(false));
    let called_in_fn = called.clone();
    let mut imports = HostImports::new();
    imports.define(
        "math",
        "double",
        host_fn(move |v| {
            let called = called_in_fn.clone();
            async move {
                called.store(true, Ordering::SeqCst); // would fire if the host fn ran
                Ok::<Value, HostError>(v)
            }
        }),
    );
    // Recorded double(5) = 100 → process = 101, and the real host fn (2n) is skipped.
    imports.with_interceptor(Arc::new(Replay(Value::S64(100))));

    let out = call_resume(&engine, &module, imports, "process", &Value::S64(5))
        .await
        .expect("call_resume");
    assert_eq!(out, Value::S64(101), "double replayed as 100 -> +1 = 101");
    assert!(
        !called.load(Ordering::SeqCst),
        "host fn must NOT run on replay (before_import short-circuited)"
    );
}
