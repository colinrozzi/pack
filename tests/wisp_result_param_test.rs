//! Test wisp result parameter type decoding with composite runtime

use composite::abi::Value;
use composite::Runtime;

const WISP_MODULE_PATH: &str = "/home/colin/work/wisp/examples/result-param-test.wasm";

// Result is represented as Variant with tag 0 = Ok, tag 1 = Err
fn ok_value(v: Value) -> Value {
    Value::Variant { tag: 0, payload: Some(Box::new(v)) }
}

fn err_value(v: Value) -> Value {
    Value::Variant { tag: 1, payload: Some(Box::new(v)) }
}

#[test]
fn test_wisp_result_ok() {
    let wasm_bytes = std::fs::read(WISP_MODULE_PATH).expect("failed to read wisp wasm");
    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load module");
    let mut instance = module.instantiate().expect("failed to instantiate");

    // Test handle-result with Ok(42) should return 42
    let input = ok_value(Value::S32(42));
    let output = instance
        .call_with_value("handle-result", &input, 0)
        .expect("failed to call handle-result");
    assert_eq!(output, Value::S32(42));
}

#[test]
fn test_wisp_result_err() {
    let wasm_bytes = std::fs::read(WISP_MODULE_PATH).expect("failed to read wisp wasm");
    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load module");
    let mut instance = module.instantiate().expect("failed to instantiate");

    // Test handle-result with Err(10) should return -10
    let input = err_value(Value::S32(10));
    let output = instance
        .call_with_value("handle-result", &input, 0)
        .expect("failed to call handle-result");
    assert_eq!(output, Value::S32(-10));
}

#[test]
fn test_wisp_result_double_ok() {
    let wasm_bytes = std::fs::read(WISP_MODULE_PATH).expect("failed to read wisp wasm");
    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load module");
    let mut instance = module.instantiate().expect("failed to instantiate");

    // Test double-ok with Ok(7) should return 14
    let input = ok_value(Value::S32(7));
    let output = instance
        .call_with_value("double-ok", &input, 0)
        .expect("failed to call double-ok");
    assert_eq!(output, Value::S32(14));
}

#[test]
fn test_wisp_result_double_err() {
    let wasm_bytes = std::fs::read(WISP_MODULE_PATH).expect("failed to read wisp wasm");
    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load module");
    let mut instance = module.instantiate().expect("failed to instantiate");

    // Test double-ok with Err(99) should return 0
    let input = err_value(Value::S32(99));
    let output = instance
        .call_with_value("double-ok", &input, 0)
        .expect("failed to call double-ok");
    assert_eq!(output, Value::S32(0));
}

#[test]
fn test_wisp_is_ok_true() {
    let wasm_bytes = std::fs::read(WISP_MODULE_PATH).expect("failed to read wisp wasm");
    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load module");
    let mut instance = module.instantiate().expect("failed to instantiate");

    // Test is-ok with Ok(123) should return 1
    let input = ok_value(Value::S32(123));
    let output = instance
        .call_with_value("is-ok", &input, 0)
        .expect("failed to call is-ok");
    assert_eq!(output, Value::S32(1));
}

#[test]
fn test_wisp_is_ok_false() {
    let wasm_bytes = std::fs::read(WISP_MODULE_PATH).expect("failed to read wisp wasm");
    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load module");
    let mut instance = module.instantiate().expect("failed to instantiate");

    // Test is-ok with Err(456) should return 0
    let input = err_value(Value::S32(456));
    let output = instance
        .call_with_value("is-ok", &input, 0)
        .expect("failed to call is-ok");
    assert_eq!(output, Value::S32(0));
}
