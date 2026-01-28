//! Test wisp list parameter type decoding with composite runtime

use pack::abi::{Value, ValueType};
use pack::Runtime;

const WISP_MODULE_PATH: &str = "/home/colin/work/wisp/examples/list-param-test.wasm";

#[test]
fn test_wisp_list_len() {
    let wasm_bytes = std::fs::read(WISP_MODULE_PATH).expect("failed to read wisp wasm");
    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load module");
    let mut instance = module.instantiate().expect("failed to instantiate");

    // Test get-list-len with [1, 2, 3] should return 3
    let input = Value::List {
        elem_type: ValueType::S32,
        items: vec![Value::S32(1), Value::S32(2), Value::S32(3)],
    };
    let output = instance
        .call_with_value("get-list-len", &input, 0)
        .expect("failed to call get-list-len");
    assert_eq!(output, Value::S32(3));

    // Test with empty list should return 0
    let input = Value::List {
        elem_type: ValueType::S32,
        items: vec![],
    };
    let output = instance
        .call_with_value("get-list-len", &input, 0)
        .expect("failed to call get-list-len with empty list");
    assert_eq!(output, Value::S32(0));
}

#[test]
fn test_wisp_list_first() {
    let wasm_bytes = std::fs::read(WISP_MODULE_PATH).expect("failed to read wisp wasm");
    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load module");
    let mut instance = module.instantiate().expect("failed to instantiate");

    // Test list-first with [42, 10, 5] should return 42
    let input = Value::List {
        elem_type: ValueType::S32,
        items: vec![Value::S32(42), Value::S32(10), Value::S32(5)],
    };
    let output = instance
        .call_with_value("list-first", &input, 0)
        .expect("failed to call list-first");
    assert_eq!(output, Value::S32(42));

    // Test with empty list should return -1
    let input = Value::List {
        elem_type: ValueType::S32,
        items: vec![],
    };
    let output = instance
        .call_with_value("list-first", &input, 0)
        .expect("failed to call list-first with empty list");
    assert_eq!(output, Value::S32(-1));
}

#[test]
fn test_wisp_list_second() {
    let wasm_bytes = std::fs::read(WISP_MODULE_PATH).expect("failed to read wisp wasm");
    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load module");
    let mut instance = module.instantiate().expect("failed to instantiate");

    // Test list-second with [1, 99, 3] should return 99
    let input = Value::List {
        elem_type: ValueType::S32,
        items: vec![Value::S32(1), Value::S32(99), Value::S32(3)],
    };
    let output = instance
        .call_with_value("list-second", &input, 0)
        .expect("failed to call list-second");
    assert_eq!(output, Value::S32(99));

    // Test with single element should return -1
    let input = Value::List {
        elem_type: ValueType::S32,
        items: vec![Value::S32(1)],
    };
    let output = instance
        .call_with_value("list-second", &input, 0)
        .expect("failed to call list-second with single element");
    assert_eq!(output, Value::S32(-1));
}

#[test]
fn test_wisp_sum_first_two() {
    let wasm_bytes = std::fs::read(WISP_MODULE_PATH).expect("failed to read wisp wasm");
    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load module");
    let mut instance = module.instantiate().expect("failed to instantiate");

    // Test sum-first-two with [10, 20, 30] should return 30
    let input = Value::List {
        elem_type: ValueType::S32,
        items: vec![Value::S32(10), Value::S32(20), Value::S32(30)],
    };
    let output = instance
        .call_with_value("sum-first-two", &input, 0)
        .expect("failed to call sum-first-two");
    assert_eq!(output, Value::S32(30));

    // Test with single element [7] should return 7
    let input = Value::List {
        elem_type: ValueType::S32,
        items: vec![Value::S32(7)],
    };
    let output = instance
        .call_with_value("sum-first-two", &input, 0)
        .expect("failed to call sum-first-two with single element");
    assert_eq!(output, Value::S32(7));

    // Test with empty list should return 0
    let input = Value::List {
        elem_type: ValueType::S32,
        items: vec![],
    };
    let output = instance
        .call_with_value("sum-first-two", &input, 0)
        .expect("failed to call sum-first-two with empty list");
    assert_eq!(output, Value::S32(0));
}
