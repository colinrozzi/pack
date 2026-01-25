//! Test wisp multi-param functions with complex types

use composite::abi::{Value, ValueType};
use composite::Runtime;

const WISP_MODULE_PATH: &str = "/home/colin/work/wisp/examples/multi-param-complex-test.wasm";

#[test]
fn test_wisp_scalar_and_record() {
    let wasm_bytes = std::fs::read(WISP_MODULE_PATH).expect("failed to read wisp wasm");
    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load module");
    let mut instance = module.instantiate().expect("failed to instantiate");

    // add-to-point(10, point{x:3, y:4}) should return 10 + 3 + 4 = 17
    let input = Value::Tuple(vec![
        Value::S32(10),
        Value::Record {
            type_name: "point".to_string(),
            fields: vec![
                ("x".to_string(), Value::S32(3)),
                ("y".to_string(), Value::S32(4)),
            ],
        },
    ]);
    let output = instance
        .call_with_value("add-to-point", &input, 0)
        .expect("failed to call add-to-point");
    assert_eq!(output, Value::S32(17));
}

#[test]
fn test_wisp_scalar_and_option_some() {
    let wasm_bytes = std::fs::read(WISP_MODULE_PATH).expect("failed to read wisp wasm");
    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load module");
    let mut instance = module.instantiate().expect("failed to instantiate");

    // add-or-default(5, Some(7)) should return 5 + 7 = 12
    let input = Value::Tuple(vec![
        Value::S32(5),
        Value::Option {
            inner_type: ValueType::S32,
            value: Some(Box::new(Value::S32(7))),
        },
    ]);
    let output = instance
        .call_with_value("add-or-default", &input, 0)
        .expect("failed to call add-or-default");
    assert_eq!(output, Value::S32(12));
}

#[test]
fn test_wisp_scalar_and_option_none() {
    let wasm_bytes = std::fs::read(WISP_MODULE_PATH).expect("failed to read wisp wasm");
    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load module");
    let mut instance = module.instantiate().expect("failed to instantiate");

    // add-or-default(5, None) should return 5
    let input = Value::Tuple(vec![
        Value::S32(5),
        Value::Option {
            inner_type: ValueType::S32,
            value: None,
        },
    ]);
    let output = instance
        .call_with_value("add-or-default", &input, 0)
        .expect("failed to call add-or-default");
    assert_eq!(output, Value::S32(5));
}

#[test]
fn test_wisp_two_options_both_some() {
    let wasm_bytes = std::fs::read(WISP_MODULE_PATH).expect("failed to read wisp wasm");
    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load module");
    let mut instance = module.instantiate().expect("failed to instantiate");

    // both-or-zero(Some(10), Some(20)) should return 30
    let input = Value::Tuple(vec![
        Value::Option {
            inner_type: ValueType::S32,
            value: Some(Box::new(Value::S32(10))),
        },
        Value::Option {
            inner_type: ValueType::S32,
            value: Some(Box::new(Value::S32(20))),
        },
    ]);
    let output = instance
        .call_with_value("both-or-zero", &input, 0)
        .expect("failed to call both-or-zero");
    assert_eq!(output, Value::S32(30));
}

#[test]
fn test_wisp_two_options_first_none() {
    let wasm_bytes = std::fs::read(WISP_MODULE_PATH).expect("failed to read wisp wasm");
    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load module");
    let mut instance = module.instantiate().expect("failed to instantiate");

    // both-or-zero(None, Some(20)) should return 0
    let input = Value::Tuple(vec![
        Value::Option {
            inner_type: ValueType::S32,
            value: None,
        },
        Value::Option {
            inner_type: ValueType::S32,
            value: Some(Box::new(Value::S32(20))),
        },
    ]);
    let output = instance
        .call_with_value("both-or-zero", &input, 0)
        .expect("failed to call both-or-zero");
    assert_eq!(output, Value::S32(0));
}

#[test]
fn test_wisp_two_options_second_none() {
    let wasm_bytes = std::fs::read(WISP_MODULE_PATH).expect("failed to read wisp wasm");
    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load module");
    let mut instance = module.instantiate().expect("failed to instantiate");

    // both-or-zero(Some(10), None) should return 0
    let input = Value::Tuple(vec![
        Value::Option {
            inner_type: ValueType::S32,
            value: Some(Box::new(Value::S32(10))),
        },
        Value::Option {
            inner_type: ValueType::S32,
            value: None,
        },
    ]);
    let output = instance
        .call_with_value("both-or-zero", &input, 0)
        .expect("failed to call both-or-zero");
    assert_eq!(output, Value::S32(0));
}
