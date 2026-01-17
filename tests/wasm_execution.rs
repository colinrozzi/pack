//! Basic WASM execution tests
//!
//! These tests verify that we can load and run WASM modules through the Runtime.

use composite::Runtime;

/// A minimal WAT module that exports an `add` function
const ADD_MODULE: &str = r#"
(module
    (func $add (param $a i32) (param $b i32) (result i32)
        local.get $a
        local.get $b
        i32.add
    )
    (export "add" (func $add))
)
"#;

#[test]
fn run_add_module() {
    // Parse WAT to WASM bytes
    let wasm_bytes = wat::parse_str(ADD_MODULE).expect("failed to parse WAT");

    // Create the runtime and load the module
    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load module");

    // Instantiate and run
    let mut instance = module.instantiate().expect("failed to instantiate");

    // Call the add function
    let result = instance.call_i32_i32_to_i32("add", 2, 3).expect("failed to call add");
    assert_eq!(result, 5);

    // Try a few more values
    assert_eq!(instance.call_i32_i32_to_i32("add", 0, 0).unwrap(), 0);
    assert_eq!(instance.call_i32_i32_to_i32("add", -1, 1).unwrap(), 0);
    assert_eq!(instance.call_i32_i32_to_i32("add", 100, 200).unwrap(), 300);
}

/// A module that uses i64 instead
const ADD64_MODULE: &str = r#"
(module
    (func $add64 (param $a i64) (param $b i64) (result i64)
        local.get $a
        local.get $b
        i64.add
    )
    (export "add64" (func $add64))
)
"#;

#[test]
fn run_add64_module() {
    let wasm_bytes = wat::parse_str(ADD64_MODULE).expect("failed to parse WAT");

    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load module");
    let mut instance = module.instantiate().expect("failed to instantiate");

    let result = instance
        .call_i64_i64_to_i64("add64", 1_000_000_000_000, 2_000_000_000_000)
        .expect("failed to call add64");
    assert_eq!(result, 3_000_000_000_000);
}

/// Test calling a non-existent function
#[test]
fn call_missing_function() {
    let wasm_bytes = wat::parse_str(ADD_MODULE).expect("failed to parse WAT");

    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load module");
    let mut instance = module.instantiate().expect("failed to instantiate");

    let err = instance.call_i32_i32_to_i32("nonexistent", 1, 2);
    assert!(err.is_err());
}
