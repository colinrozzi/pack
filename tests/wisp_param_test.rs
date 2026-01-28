//! Test wisp parameter decoding with composite runtime

use pack::abi::Value;
use pack::Runtime;

const WISP_MODULE_PATH: &str = "/home/colin/work/wisp/examples/param-decode-test.wasm";

#[test]
fn test_wisp_param_decode_s32() {
    let wasm_bytes = std::fs::read(WISP_MODULE_PATH).expect("failed to read wisp wasm");
    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load module");
    let mut instance = module.instantiate().expect("failed to instantiate");

    // Test add-ten: add-ten(5) should return 15
    let input = Value::S32(5);
    let output = instance
        .call_with_value("add-ten", &input)
        .expect("failed to call add-ten");
    assert_eq!(output, Value::S32(15));

    // Test double: double(7) should return 14
    let input = Value::S32(7);
    let output = instance
        .call_with_value("double", &input)
        .expect("failed to call double");
    assert_eq!(output, Value::S32(14));

    // Test with negative number
    let input = Value::S32(-10);
    let output = instance
        .call_with_value("add-ten", &input)
        .expect("failed to call add-ten with negative");
    assert_eq!(output, Value::S32(0));
}

#[test]
fn test_wisp_param_decode_s64() {
    let wasm_bytes = std::fs::read(WISP_MODULE_PATH).expect("failed to read wisp wasm");
    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load module");
    let mut instance = module.instantiate().expect("failed to instantiate");

    // Test add-hundred64: add-hundred64(1000) should return 1100
    let input = Value::S64(1000);
    let output = instance
        .call_with_value("add-hundred64", &input)
        .expect("failed to call add-hundred64");
    assert_eq!(output, Value::S64(1100));

    // Test with large number
    let input = Value::S64(10_000_000_000i64);
    let output = instance
        .call_with_value("add-hundred64", &input)
        .expect("failed to call add-hundred64 with large number");
    assert_eq!(output, Value::S64(10_000_000_100i64));
}

#[test]
fn test_wisp_param_decode_f32() {
    let wasm_bytes = std::fs::read(WISP_MODULE_PATH).expect("failed to read wisp wasm");
    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load module");
    let mut instance = module.instantiate().expect("failed to instantiate");

    // Test half-f32: half-f32(10.0) should return 5.0
    let input = Value::F32(10.0);
    let output = instance
        .call_with_value("half-f32", &input)
        .expect("failed to call half-f32");
    assert_eq!(output, Value::F32(5.0));
}

#[test]
fn test_wisp_param_decode_f64() {
    let wasm_bytes = std::fs::read(WISP_MODULE_PATH).expect("failed to read wisp wasm");
    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load module");
    let mut instance = module.instantiate().expect("failed to instantiate");

    // Test square-f64: square-f64(3.0) should return 9.0
    let input = Value::F64(3.0);
    let output = instance
        .call_with_value("square-f64", &input)
        .expect("failed to call square-f64");
    assert_eq!(output, Value::F64(9.0));

    // Test with pi
    let input = Value::F64(3.14159);
    let output = instance
        .call_with_value("square-f64", &input)
        .expect("failed to call square-f64 with pi");
    if let Value::F64(result) = output {
        assert!((result - 9.8695877281).abs() < 0.0001);
    } else {
        panic!("expected F64 result");
    }
}

#[test]
fn test_wisp_param_decode_string() {
    let wasm_bytes = std::fs::read(WISP_MODULE_PATH).expect("failed to read wisp wasm");
    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load module");
    let mut instance = module.instantiate().expect("failed to instantiate");

    // Test get-string-len: "hello" should return 5
    let input = Value::String("hello".to_string());
    let output = instance
        .call_with_value("get-string-len", &input)
        .expect("failed to call get-string-len");
    assert_eq!(output, Value::S32(5));

    // Test with longer string
    let input = Value::String("Hello, World!".to_string());
    let output = instance
        .call_with_value("get-string-len", &input)
        .expect("failed to call get-string-len with longer string");
    assert_eq!(output, Value::S32(13));

    // Test with empty string
    let input = Value::String("".to_string());
    let output = instance
        .call_with_value("get-string-len", &input)
        .expect("failed to call get-string-len with empty string");
    assert_eq!(output, Value::S32(0));
}
