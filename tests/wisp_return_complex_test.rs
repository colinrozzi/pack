//! Test wisp returning complex types with composite runtime

use pack::abi::Value;
use pack::Runtime;

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
        .call_with_value("make-point", &input)
        .expect("failed to call make-point");

    // Record should be returned as a Record with named fields
    // CGRF v2 preserves actual field names
    match output {
        Value::Record { fields, .. } => {
            assert_eq!(fields.len(), 2);
            // Fields come back with actual names
            assert_eq!(fields[0], ("x".to_string(), Value::S32(3)));
            assert_eq!(fields[1], ("y".to_string(), Value::S32(4)));
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
        .call_with_value("make-some", &input)
        .expect("failed to call make-some");

    // Check that it's a Some with correct inner value
    match output {
        Value::Option { value: Some(inner), .. } => {
            assert_eq!(*inner, Value::S32(42));
        }
        other => panic!("expected Option::Some, got {:?}", other),
    }
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
        .call_with_value("make-none", &input)
        .expect("failed to call make-none");

    match output {
        Value::Option { value: None, .. } => {}
        other => panic!("expected Option::None, got {:?}", other),
    }
}

#[test]
fn test_wisp_return_result_ok() {
    let wasm_bytes = std::fs::read(WISP_MODULE_PATH).expect("failed to read wisp wasm");
    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load module");
    let mut instance = module.instantiate().expect("failed to instantiate");

    // make-ok(100) should return Ok(100) as Result type
    let input = Value::S32(100);
    let output = instance
        .call_with_value("make-ok", &input)
        .expect("failed to call make-ok");

    // Result is now properly decoded as Value::Result
    match output {
        Value::Result { value: Ok(inner), .. } => {
            assert_eq!(*inner, Value::S32(100));
        }
        other => panic!("expected Result::Ok, got {:?}", other),
    }
}

#[test]
fn test_wisp_return_result_err() {
    let wasm_bytes = std::fs::read(WISP_MODULE_PATH).expect("failed to read wisp wasm");
    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load module");
    let mut instance = module.instantiate().expect("failed to instantiate");

    // make-err(999) should return Err(999) as Result type
    let input = Value::S32(999);
    let output = instance
        .call_with_value("make-err", &input)
        .expect("failed to call make-err");

    // Result is now properly decoded as Value::Result
    match output {
        Value::Result { value: Err(inner), .. } => {
            assert_eq!(*inner, Value::S32(999));
        }
        other => panic!("expected Result::Err, got {:?}", other),
    }
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
        .call_with_value("get-greeting", &input)
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
        .call_with_value("get-red", &input)
        .expect("failed to call get-red");

    match output {
        Value::Variant { tag: 0, payload, .. } => {
            assert!(payload.is_empty());
        }
        other => panic!("expected Variant with tag 0, got {:?}", other),
    }
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
        .call_with_value("get-green", &input)
        .expect("failed to call get-green");

    match output {
        Value::Variant { tag: 1, payload, .. } => {
            assert!(payload.is_empty());
        }
        other => panic!("expected Variant with tag 1, got {:?}", other),
    }
}
