//! Test wisp multi-parameter function decoding with composite runtime

use pack::abi::Value;
use pack::Runtime;

const WISP_MODULE_PATH: &str = "/home/colin/work/wisp/examples/multi-param-test.wasm";

#[test]
fn test_wisp_add_two_s32() {
    let wasm_bytes = std::fs::read(WISP_MODULE_PATH).expect("failed to read wisp wasm");
    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load module");
    let mut instance = module.instantiate().expect("failed to instantiate");

    // Test add(3, 5) = 8
    let input = Value::Tuple(vec![Value::S32(3), Value::S32(5)]);
    let output = instance
        .call_with_value("add", &input)
        .expect("failed to call add");
    assert_eq!(output, Value::S32(8));

    // Test add(100, -50) = 50
    let input = Value::Tuple(vec![Value::S32(100), Value::S32(-50)]);
    let output = instance
        .call_with_value("add", &input)
        .expect("failed to call add");
    assert_eq!(output, Value::S32(50));
}

#[test]
fn test_wisp_sum_three_s32() {
    let wasm_bytes = std::fs::read(WISP_MODULE_PATH).expect("failed to read wisp wasm");
    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load module");
    let mut instance = module.instantiate().expect("failed to instantiate");

    // Test sum3(1, 2, 3) = 6
    let input = Value::Tuple(vec![Value::S32(1), Value::S32(2), Value::S32(3)]);
    let output = instance
        .call_with_value("sum3", &input)
        .expect("failed to call sum3");
    assert_eq!(output, Value::S32(6));

    // Test sum3(10, 20, 30) = 60
    let input = Value::Tuple(vec![Value::S32(10), Value::S32(20), Value::S32(30)]);
    let output = instance
        .call_with_value("sum3", &input)
        .expect("failed to call sum3");
    assert_eq!(output, Value::S32(60));
}

#[test]
fn test_wisp_mixed_types() {
    let wasm_bytes = std::fs::read(WISP_MODULE_PATH).expect("failed to read wisp wasm");
    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load module");
    let mut instance = module.instantiate().expect("failed to instantiate");

    // Test mixed-add(10, 100) = 110
    let input = Value::Tuple(vec![Value::S32(10), Value::S64(100)]);
    let output = instance
        .call_with_value("mixed-add", &input)
        .expect("failed to call mixed-add");
    assert_eq!(output, Value::S64(110));
}
