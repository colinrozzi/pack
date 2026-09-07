//! End-to-end proof of the engine axis: real actors driven entirely through the
//! backend-agnostic `packr_core` orchestration on the `packr-wasmtime` backend —
//! no `wasmtime` types named at the call site.
//!
//! Packages are built fresh from the workspace so the guest's `packr-abi` always
//! matches `packr-core`'s (a prebuilt fixture would go stale on any wire change).
//! Built with `--export-memory --no-entry` they export their own memory. Tests
//! skip (rather than fail) if the wasm toolchain is unavailable.

use std::path::{Path, PathBuf};
use std::process::Command;

use packr_core::{call_with_value, host_fn, HostError, HostImports, Value, WasmEngine};
use packr_wasmtime::WasmtimeEngine;

/// Build `packages/<pkg_dir>` to a self-contained actor wasm and return the path
/// to `<wasm_name>.wasm`, or `None` if the wasm toolchain isn't available.
fn build_pkg(pkg_dir: &str, wasm_name: &str) -> Option<PathBuf> {
    // CARGO_MANIFEST_DIR = crates/pack-wasmtime; packages live at the repo root.
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let manifest = root.join(format!("packages/{pkg_dir}/Cargo.toml"));
    let out = root.join(format!(
        "packages/{pkg_dir}/target/wasm32-unknown-unknown/release/{wasm_name}.wasm"
    ));

    let status = Command::new("cargo")
        .args([
            "build",
            "--manifest-path",
            manifest.to_str().unwrap(),
            "--target",
            "wasm32-unknown-unknown",
            "--release",
        ])
        .env(
            "RUSTFLAGS",
            "-C link-arg=--export-memory -C link-arg=--no-entry",
        )
        .status();

    match status {
        Ok(s) if s.success() && out.exists() => Some(out),
        _ => None,
    }
}

/// A self-contained actor (imports nothing) — exercises the execution cluster.
/// `math-real` exports `double`, where `double(S64(n)) = 2n`.
#[tokio::test]
async fn self_contained_actor_roundtrips_through_the_backend() {
    let Some(wasm_path) = build_pkg("math-real", "math_real") else {
        eprintln!("SKIP: wasm32 toolchain unavailable");
        return;
    };
    let wasm = std::fs::read(&wasm_path).expect("read the built wasm");

    let engine = WasmtimeEngine::new();
    let module = engine.compile(&wasm).await.expect("compile");
    let mut instance = engine
        .instantiate(&module, HostImports::new())
        .await
        .expect("instantiate");

    let result = call_with_value(&mut instance, "double", &Value::S64(5))
        .await
        .expect("call double(5)");
    assert_eq!(result, Value::S64(10), "double(5) = 10 through the backend");

    let result2 = call_with_value(&mut instance, "double", &Value::S64(20))
        .await
        .expect("call double(20)");
    assert_eq!(result2, Value::S64(40), "double(20) = 40");
}

/// A host-import actor — exercises the guest → host cluster. `adder-pic` imports
/// `math.double`; we satisfy it with a capture-based host fn (`n -> 2n`), so
/// `process(n) = double(n) + 1`, running the full guest→host→guest round trip
/// (input marshalled out, host fn invoked, output guest-allocated back).
#[tokio::test]
async fn host_import_actor_runs_through_the_backend() {
    let Some(wasm_path) = build_pkg("adder-pic", "adder_pic_package") else {
        eprintln!("SKIP: wasm32 toolchain unavailable");
        return;
    };
    let wasm = std::fs::read(&wasm_path).expect("read the built wasm");

    let engine = WasmtimeEngine::new();
    let module = engine.compile(&wasm).await.expect("compile");

    // adder-pic imports math.double AND math.big; provide both (big is unused
    // by `process` but must be satisfied at instantiate).
    let mut imports = HostImports::new();
    imports
        .define(
            "math",
            "double",
            host_fn(|v| async move {
                Ok::<Value, HostError>(match v {
                    Value::S64(n) => Value::S64(n * 2),
                    other => other,
                })
            }),
        )
        .define("math", "big", host_fn(|v| async move { Ok::<Value, HostError>(v) }));

    let mut instance = engine
        .instantiate(&module, imports)
        .await
        .expect("instantiate with host imports");

    // process(5) = double(5) + 1 = 11, where double is our HOST function.
    let result = call_with_value(&mut instance, "process", &Value::S64(5))
        .await
        .expect("call process(5)");
    assert_eq!(result, Value::S64(11), "process(5) = 2*5 + 1 = 11 via the host fn");

    let result2 = call_with_value(&mut instance, "process", &Value::S64(50))
        .await
        .expect("call process(50)");
    assert_eq!(result2, Value::S64(101), "process(50) = 2*50 + 1 = 101");
}
