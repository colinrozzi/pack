//! Test wisp compound parameter type decoding with composite runtime

use composite::abi::Value;
use composite::Runtime;

const WISP_MODULE_PATH: &str = "/home/colin/work/wisp/examples/compound-param-test.wasm";

#[test]
fn test_wisp_record_param() {
    let wasm_bytes = std::fs::read(WISP_MODULE_PATH).expect("failed to read wisp wasm");
    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load module");
    let mut instance = module.instantiate().expect("failed to instantiate");

    // Test point-sum: point(10, 20) should return 30
    let input = Value::Record(vec![
        ("field0".to_string(), Value::S32(10)),
        ("field1".to_string(), Value::S32(20)),
    ]);
    let output = instance
        .call_with_value("point-sum", &input, 0)
        .expect("failed to call point-sum");
    assert_eq!(output, Value::S32(30));

    // Test with different values
    let input = Value::Record(vec![
        ("field0".to_string(), Value::S32(100)),
        ("field1".to_string(), Value::S32(-50)),
    ]);
    let output = instance
        .call_with_value("point-sum", &input, 0)
        .expect("failed to call point-sum");
    assert_eq!(output, Value::S32(50));
}

#[test]
fn test_wisp_option_param_some() {
    let wasm_bytes = std::fs::read(WISP_MODULE_PATH).expect("failed to read wisp wasm");
    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load module");
    let mut instance = module.instantiate().expect("failed to instantiate");

    // Test unwrap-or-zero with Some(42) should return 42
    let input = Value::Option(Some(Box::new(Value::S32(42))));
    let output = instance
        .call_with_value("unwrap-or-zero", &input, 0)
        .expect("failed to call unwrap-or-zero with Some");
    assert_eq!(output, Value::S32(42));
}

#[test]
fn test_wisp_option_param_none() {
    let wasm_bytes = std::fs::read(WISP_MODULE_PATH).expect("failed to read wisp wasm");
    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load module");
    let mut instance = module.instantiate().expect("failed to instantiate");

    // Test unwrap-or-zero with None should return 0
    let input = Value::Option(None);
    let output = instance
        .call_with_value("unwrap-or-zero", &input, 0)
        .expect("failed to call unwrap-or-zero with None");
    assert_eq!(output, Value::S32(0));
}

#[test]
fn test_wisp_string_in_tuple() {
    let wasm_bytes = std::fs::read(WISP_MODULE_PATH).expect("failed to read wisp wasm");
    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load module");
    let mut instance = module.instantiate().expect("failed to instantiate");

    // Test greet-with-count("hello", 10) should return 5 + 10 = 15
    let input = Value::Tuple(vec![
        Value::String("hello".to_string()),
        Value::S32(10),
    ]);
    let output = instance
        .call_with_value("greet-with-count", &input, 0)
        .expect("failed to call greet-with-count");
    assert_eq!(output, Value::S32(15));
}
