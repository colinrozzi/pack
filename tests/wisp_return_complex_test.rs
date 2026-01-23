//! Test wisp returning complex types with composite runtime

use composite::abi::Value;
use composite::Runtime;

const WISP_MODULE_PATH: &str = "/home/colin/work/wisp/examples/return-complex-test.wasm";

#[test]
fn test_wisp_return_record() {
    let wasm_bytes = std::fs::read(WISP_MODULE_PATH).expect("failed to read wisp wasm");
    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load module");
    let mut instance = module.instantiate().expect("failed to instantiate");

    // make-point(3, 4) should return point { x: 3, y: 4 }
    let input = Value::Tuple(vec![Value::S32(3), Value::S32(4)]);
    let output = instance
        .call_with_value("make-point", &input, 0)
        .expect("failed to call make-point");

    // Record should be returned as a Record with named fields
    // Note: CGRF doesn't preserve field names, so they come back as "field0", "field1", etc.
    match output {
        Value::Record(fields) => {
            assert_eq!(fields.len(), 2);
            // Fields come back with generic names in order
            assert_eq!(fields[0], ("field0".to_string(), Value::S32(3)));
            assert_eq!(fields[1], ("field1".to_string(), Value::S32(4)));
        }
        other => panic!("expected Record, got {:?}", other),
    }
}

#[test]
fn test_wisp_return_option_some() {
    let wasm_bytes = std::fs::read(WISP_MODULE_PATH).expect("failed to read wisp wasm");
    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load module");
    let mut instance = module.instantiate().expect("failed to instantiate");

    // make-some(42) should return Some(42)
    let input = Value::S32(42);
    let output = instance
        .call_with_value("make-some", &input, 0)
        .expect("failed to call make-some");

    assert_eq!(output, Value::Option(Some(Box::new(Value::S32(42)))));
}

#[test]
fn test_wisp_return_option_none() {
    let wasm_bytes = std::fs::read(WISP_MODULE_PATH).expect("failed to read wisp wasm");
    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load module");
    let mut instance = module.instantiate().expect("failed to instantiate");

    // make-none() should return None
    // No input parameters - use empty tuple
    let input = Value::Tuple(vec![]);
    let output = instance
        .call_with_value("make-none", &input, 0)
        .expect("failed to call make-none");

    assert_eq!(output, Value::Option(None));
}

#[test]
fn test_wisp_return_result_ok() {
    let wasm_bytes = std::fs::read(WISP_MODULE_PATH).expect("failed to read wisp wasm");
    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load module");
    let mut instance = module.instantiate().expect("failed to instantiate");

    // make-ok(100) should return Ok(100) as Variant { tag: 0, payload: 100 }
    let input = Value::S32(100);
    let output = instance
        .call_with_value("make-ok", &input, 0)
        .expect("failed to call make-ok");

    // Result is represented as Variant (tag 0 = Ok, tag 1 = Err)
    assert_eq!(output, Value::Variant {
        tag: 0,
        payload: Some(Box::new(Value::S32(100)))
    });
}

#[test]
fn test_wisp_return_result_err() {
    let wasm_bytes = std::fs::read(WISP_MODULE_PATH).expect("failed to read wisp wasm");
    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load module");
    let mut instance = module.instantiate().expect("failed to instantiate");

    // make-err(999) should return Err(999) as Variant { tag: 1, payload: 999 }
    let input = Value::S32(999);
    let output = instance
        .call_with_value("make-err", &input, 0)
        .expect("failed to call make-err");

    // Result is represented as Variant (tag 0 = Ok, tag 1 = Err)
    assert_eq!(output, Value::Variant {
        tag: 1,
        payload: Some(Box::new(Value::S32(999)))
    });
}

#[test]
fn test_wisp_return_string() {
    let wasm_bytes = std::fs::read(WISP_MODULE_PATH).expect("failed to read wisp wasm");
    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load module");
    let mut instance = module.instantiate().expect("failed to instantiate");

    // get-greeting() should return "hello"
    let input = Value::Tuple(vec![]);
    let output = instance
        .call_with_value("get-greeting", &input, 0)
        .expect("failed to call get-greeting");

    assert_eq!(output, Value::String("hello".to_string()));
}

#[test]
fn test_wisp_return_variant_red() {
    let wasm_bytes = std::fs::read(WISP_MODULE_PATH).expect("failed to read wisp wasm");
    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load module");
    let mut instance = module.instantiate().expect("failed to instantiate");

    // get-red() should return red variant (tag 0, no payload)
    let input = Value::Tuple(vec![]);
    let output = instance
        .call_with_value("get-red", &input, 0)
        .expect("failed to call get-red");

    assert_eq!(output, Value::Variant { tag: 0, payload: None });
}

#[test]
fn test_wisp_return_variant_green() {
    let wasm_bytes = std::fs::read(WISP_MODULE_PATH).expect("failed to read wisp wasm");
    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load module");
    let mut instance = module.instantiate().expect("failed to instantiate");

    // get-green() should return green variant (tag 1, no payload)
    let input = Value::Tuple(vec![]);
    let output = instance
        .call_with_value("get-green", &input, 0)
        .expect("failed to call get-green");

    assert_eq!(output, Value::Variant { tag: 1, payload: None });
}
