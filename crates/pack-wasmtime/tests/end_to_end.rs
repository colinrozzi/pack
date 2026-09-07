//! End-to-end proof of the engine axis: a real self-contained actor, driven
//! entirely through the backend-agnostic `packr_core` orchestration on the
//! `packr-wasmtime` backend — no `wasmtime` types named at the call site.
//!
//! We build `math-real` fresh from the workspace so the guest's `packr-abi`
//! always matches `packr-core`'s (a prebuilt fixture would go stale on any wire
//! change). Built with `--export-memory --no-entry` it is fully self-contained:
//! it imports nothing (provides its own memory + allocator) and exports
//! `double`, where `double(S64(n)) = 2n`. If the wasm toolchain isn't available
//! the test skips with a message rather than failing (mirrors the compose tests).

use std::path::{Path, PathBuf};
use std::process::Command;

use packr_core::{call_with_value, Value, WasmEngine};
use packr_wasmtime::WasmtimeEngine;

/// Build `packages/math-real` as a self-contained actor wasm. Returns the wasm
/// path, or `None` if the wasm toolchain isn't available (skip, don't fail).
fn build_self_contained_math() -> Option<PathBuf> {
    // CARGO_MANIFEST_DIR = crates/pack-wasmtime; the package lives at the repo root.
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let manifest = root.join("packages/math-real/Cargo.toml");
    let out = root.join("packages/math-real/target/wasm32-unknown-unknown/release/math_real.wasm");

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

#[tokio::test]
async fn self_contained_actor_roundtrips_through_the_backend() {
    let Some(wasm_path) = build_self_contained_math() else {
        eprintln!("SKIP: wasm32 toolchain unavailable — cannot build the self-contained fixture");
        return;
    };
    let wasm = std::fs::read(&wasm_path).expect("read the built self-contained wasm");

    // Compile + instantiate through the trait — the only backend-specific line
    // is choosing which engine to construct.
    let engine = WasmtimeEngine::new();
    let module = engine.compile(&wasm).await.expect("compile");
    let mut instance = engine.instantiate(&module).await.expect("instantiate");

    // Drive the pack ABI end to end via packr-core, generic over WasmInstance.
    let result = call_with_value(&mut instance, "double", &Value::S64(5))
        .await
        .expect("call double(5)");
    assert_eq!(
        result,
        Value::S64(10),
        "double(5) = 10, round-tripped through the graph ABI on the wasmtime backend"
    );

    // A second call on the same instance must also work (exercises the guest
    // allocator + memory across repeated calls).
    let result2 = call_with_value(&mut instance, "double", &Value::S64(20))
        .await
        .expect("call double(20)");
    assert_eq!(result2, Value::S64(40), "double(20) = 40");
}
