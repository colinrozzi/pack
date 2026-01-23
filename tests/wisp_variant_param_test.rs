//! Test wisp variant parameter type decoding with composite runtime

use composite::abi::Value;
use composite::Runtime;

const WISP_MODULE_PATH: &str = "/home/colin/work/wisp/examples/variant-param-test.wasm";

#[test]
fn test_wisp_variant_no_payload_red() {
    let wasm_bytes = std::fs::read(WISP_MODULE_PATH).expect("failed to read wisp wasm");
    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load module");
    let mut instance = module.instantiate().expect("failed to instantiate");

    // Test color-to-num with red (tag 0) should return 1
    let input = Value::Variant { tag: 0, payload: None };
    let output = instance
        .call_with_value("color-to-num", &input, 0)
        .expect("failed to call color-to-num");
    assert_eq!(output, Value::S32(1));
}

#[test]
fn test_wisp_variant_no_payload_green() {
    let wasm_bytes = std::fs::read(WISP_MODULE_PATH).expect("failed to read wisp wasm");
    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load module");
    let mut instance = module.instantiate().expect("failed to instantiate");

    // Test color-to-num with green (tag 1) should return 2
    let input = Value::Variant { tag: 1, payload: None };
    let output = instance
        .call_with_value("color-to-num", &input, 0)
        .expect("failed to call color-to-num");
    assert_eq!(output, Value::S32(2));
}

#[test]
fn test_wisp_variant_no_payload_blue() {
    let wasm_bytes = std::fs::read(WISP_MODULE_PATH).expect("failed to read wisp wasm");
    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load module");
    let mut instance = module.instantiate().expect("failed to instantiate");

    // Test color-to-num with blue (tag 2) should return 3
    let input = Value::Variant { tag: 2, payload: None };
    let output = instance
        .call_with_value("color-to-num", &input, 0)
        .expect("failed to call color-to-num");
    assert_eq!(output, Value::S32(3));
}

#[test]
fn test_wisp_variant_with_payload_circle() {
    let wasm_bytes = std::fs::read(WISP_MODULE_PATH).expect("failed to read wisp wasm");
    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load module");
    let mut instance = module.instantiate().expect("failed to instantiate");

    // Test get-dimension with circle(10) should return 10
    let input = Value::Variant {
        tag: 0,
        payload: Some(Box::new(Value::S32(10)))
    };
    let output = instance
        .call_with_value("get-dimension", &input, 0)
        .expect("failed to call get-dimension");
    assert_eq!(output, Value::S32(10));
}

#[test]
fn test_wisp_variant_with_payload_square() {
    let wasm_bytes = std::fs::read(WISP_MODULE_PATH).expect("failed to read wisp wasm");
    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load module");
    let mut instance = module.instantiate().expect("failed to instantiate");

    // Test get-dimension with square(5) should return 5
    let input = Value::Variant {
        tag: 1,
        payload: Some(Box::new(Value::S32(5)))
    };
    let output = instance
        .call_with_value("get-dimension", &input, 0)
        .expect("failed to call get-dimension");
    assert_eq!(output, Value::S32(5));
}

#[test]
fn test_wisp_variant_double_dimension() {
    let wasm_bytes = std::fs::read(WISP_MODULE_PATH).expect("failed to read wisp wasm");
    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load module");
    let mut instance = module.instantiate().expect("failed to instantiate");

    // Test double-dimension with circle(7) should return 14
    let input = Value::Variant {
        tag: 0,
        payload: Some(Box::new(Value::S32(7)))
    };
    let output = instance
        .call_with_value("double-dimension", &input, 0)
        .expect("failed to call double-dimension");
    assert_eq!(output, Value::S32(14));

    // Test double-dimension with square(8) should return 16
    let input = Value::Variant {
        tag: 1,
        payload: Some(Box::new(Value::S32(8)))
    };
    let output = instance
        .call_with_value("double-dimension", &input, 0)
        .expect("failed to call double-dimension");
    assert_eq!(output, Value::S32(16));
}
