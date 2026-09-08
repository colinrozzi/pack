//! End-to-end proof of the pending/resume async-ABI on the **browser** backend,
//! run in Node (no JSPI, no GUI): the guest parks on an async host call, the host
//! awaits it on the JS event loop, and re-enters via `__pack_resume`.
//!
//! `async-adder`: `process(n) = double(n) + 1`, with `double` a **genuinely
//! async** host fn — it `await`s a resolved JS promise, so on the first poll it's
//! `Pending`, forcing the park/stash/pump/resume path.

#![cfg(target_arch = "wasm32")]

use js_sys::Promise;
use packr_core::{host_fn, HostError, HostImports, Value, WasmEngine};
use packr_web::{call_resume, WebEngine};
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::JsFuture;
use wasm_bindgen_test::wasm_bindgen_test;

const ASYNC_ADDER: &[u8] = include_bytes!("fixtures/async_adder.wasm");

#[wasm_bindgen_test]
async fn async_host_call_through_pending_resume_in_the_browser() {
    let engine = WebEngine::new();
    let module = engine.compile(ASYNC_ADDER).await.expect("compile");

    let mut imports = HostImports::new();
    imports.define(
        "math",
        "double",
        host_fn(|v| async move {
            // Genuinely async: await a resolved JS promise → yields once → the
            // host call is Pending, so the guest parks and the resume path runs.
            let _ = JsFuture::from(Promise::resolve(&JsValue::NULL)).await;
            Ok::<Value, HostError>(match v {
                Value::S64(n) => Value::S64(n * 2),
                other => other,
            })
        }),
    );

    let out = call_resume(&engine, &module, imports, "process", &Value::S64(5))
        .await
        .expect("call_resume process(5)");
    assert_eq!(out, Value::S64(11));
}
