//! End-to-end proof of the browser backend, run in **Node** via
//! `wasm-bindgen-test` (no GUI): a real self-contained actor driven through the
//! backend-agnostic `packr_core` orchestration on the `packr-web` (JS
//! `WebAssembly`) backend.
//!
//! The fixture is a prebuilt `math-real` (exports `double`, `double(S64(n))=2n`,
//! imports nothing) — embedded because the test itself runs inside wasm and
//! can't shell out to build it. Rebuild it with:
//!   RUSTFLAGS="-C link-arg=--export-memory -C link-arg=--no-entry" \
//!     cargo build --manifest-path packages/math-real/Cargo.toml \
//!     --target wasm32-unknown-unknown --release

#![cfg(target_arch = "wasm32")]

use packr_core::{call_with_value, HostImports, Value, WasmEngine};
use packr_web::WebEngine;
use wasm_bindgen_test::wasm_bindgen_test;

const MATH_REAL: &[u8] = include_bytes!("fixtures/math_real.wasm");

#[wasm_bindgen_test]
async fn double_through_the_web_backend() {
    let engine = WebEngine::new();
    let module = engine.compile(MATH_REAL).await.expect("compile");
    let mut instance = engine
        .instantiate(&module, HostImports::new())
        .await
        .expect("instantiate");

    let result = call_with_value(&mut instance, "double", &Value::S64(5))
        .await
        .expect("call double(5)");
    assert_eq!(result, Value::S64(10));

    let result2 = call_with_value(&mut instance, "double", &Value::S64(21))
        .await
        .expect("call double(21)");
    assert_eq!(result2, Value::S64(42));
}
