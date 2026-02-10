//! Integration tests for the new host function registration API

use pack::abi::Value;
use pack::runtime::{HostLinkerBuilder, LinkerError};
use pack::Runtime;
use wasmtime::Caller;

/// Simple state for testing
#[derive(Clone)]
struct TestState {
    actor_id: i32,
}

impl TestState {
    fn new(actor_id: i32) -> Self {
        Self { actor_id }
    }
}

#[test]
fn test_namespaced_interface_registration() {
    // Module with namespaced imports - uses (i32, i32) -> i32 signature for compatibility
    let module_wat = r#"
    (module
        (import "theater:simple/runtime" "add_offset" (func $add_offset (param i32 i32) (result i32)))
        (memory (export "memory") 1)

        (func $compute (param $a i32) (param $b i32) (result i32)
            (call $add_offset (local.get $a) (local.get $b))
        )

        (export "compute" (func $compute))
    )
    "#;

    let wasm_bytes = wat::parse_str(module_wat).expect("parse WAT");
    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("load module");

    let state = TestState::new(100); // offset of 100

    let mut instance = module
        .instantiate_with_host(state, |builder| {
            builder
                .interface("theater:simple/runtime")?
                .func_raw(
                    "add_offset",
                    |caller: Caller<'_, TestState>, a: i32, b: i32| -> i32 {
                        a + b + caller.data().actor_id
                    },
                )?;
            Ok(())
        })
        .expect("instantiate");

    // compute(5, 10) should return 5 + 10 + 100 = 115
    let result = instance.call_i32_i32_to_i32("compute", 5, 10).expect("call");
    assert_eq!(result, 115);
}

#[test]
fn test_func_typed_with_value() {
    // Module that wraps a host function call - the host function uses Graph ABI
    // Guest-allocates ABI: (in_ptr, in_len, out_ptr_ptr, out_len_ptr) -> status
    let module_wat = r#"
    (module
        (import "test" "double" (func $double (param i32 i32 i32 i32) (result i32)))
        (memory (export "memory") 1)

        ;; Reserve space for result slots at fixed offset (16KB)
        (global $result_ptr_offset i32 (i32.const 16384))
        (global $result_len_offset i32 (i32.const 16388))
        ;; Output data offset
        (global $output_offset i32 (i32.const 16392))

        ;; Wrapper that calls host function with guest-allocates ABI
        ;; and copies result to caller's slots
        (func $call_double (param $in_ptr i32) (param $in_len i32) (param $out_ptr_ptr i32) (param $out_len_ptr i32) (result i32)
            (local $status i32)
            (local $result_ptr i32)
            (local $result_len i32)

            ;; Call host function - it writes ptr/len to our slots
            (local.set $status
                (call $double
                    (local.get $in_ptr)
                    (local.get $in_len)
                    (global.get $result_ptr_offset)
                    (global.get $result_len_offset)))

            ;; If error, propagate
            (if (i32.ne (local.get $status) (i32.const 0))
                (then (return (local.get $status))))

            ;; Read result ptr/len from slots
            (local.set $result_ptr (i32.load (global.get $result_ptr_offset)))
            (local.set $result_len (i32.load (global.get $result_len_offset)))

            ;; Copy result to our output area
            (memory.copy (global.get $output_offset) (local.get $result_ptr) (local.get $result_len))

            ;; Write our output location to caller's slots
            (i32.store (local.get $out_ptr_ptr) (global.get $output_offset))
            (i32.store (local.get $out_len_ptr) (local.get $result_len))

            (i32.const 0)
        )

        (export "call_double" (func $call_double))
    )
    "#;

    let wasm_bytes = wat::parse_str(module_wat).expect("parse WAT");
    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("load module");

    let mut instance = module
        .instantiate_with_host((), |builder| {
            builder.interface("test")?.func_typed(
                "double",
                |_ctx: &mut pack::Ctx<'_, ()>, input: Value| -> Value {
                    // Double any S64 value
                    match input {
                        Value::S64(n) => Value::S64(n * 2),
                        other => other,
                    }
                },
            )?;
            Ok(())
        })
        .expect("instantiate");

    // Use call_with_value which handles the full Graph ABI flow
    let input = Value::S64(21);
    let output = instance
        .call_with_value("call_double", &input)
        .expect("call");

    assert_eq!(output, Value::S64(42)); // 21 * 2 = 42
}

#[test]
fn test_multiple_interfaces() {
    // Create a module that imports from multiple namespaces
    let module_wat = r#"
    (module
        (import "api:v1/math" "add" (func $add (param i32 i32) (result i32)))
        (import "api:v1/util" "double" (func $double (param i32 i32) (result i32)))
        (memory (export "memory") 1)

        (func $compute (param $a i32) (param $b i32) (result i32)
            ;; double(add(a, b), 0)
            (call $double (call $add (local.get $a) (local.get $b)) (i32.const 0))
        )

        (export "compute" (func $compute))
    )
    "#;

    let wasm_bytes = wat::parse_str(module_wat).expect("parse WAT");
    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("load module");

    let mut instance = module
        .instantiate_with_host((), |builder| {
            builder
                .interface("api:v1/math")?
                .func_raw("add", |_: Caller<'_, ()>, a: i32, b: i32| a + b)?;

            builder
                .interface("api:v1/util")?
                // Takes two args but only uses first (for signature compatibility)
                .func_raw("double", |_: Caller<'_, ()>, x: i32, _unused: i32| x * 2)?;

            Ok(())
        })
        .expect("instantiate");

    let result = instance.call_i32_i32_to_i32("compute", 5, 10).expect("call");
    assert_eq!(result, 30); // double(add(5, 10)) = double(15) = 30
}

#[test]
fn test_provider_pattern() {
    use pack::runtime::HostFunctionProvider;

    struct MathProvider;

    impl HostFunctionProvider<()> for MathProvider {
        fn register(
            &self,
            builder: &mut HostLinkerBuilder<'_, ()>,
        ) -> Result<(), LinkerError> {
            builder
                .interface("math")?
                .func_raw("add", |_: Caller<'_, ()>, a: i32, b: i32| a + b)?
                .func_raw("mul", |_: Caller<'_, ()>, a: i32, b: i32| a * b)?;
            Ok(())
        }
    }

    let module_wat = r#"
    (module
        (import "math" "add" (func $add (param i32 i32) (result i32)))
        (import "math" "mul" (func $mul (param i32 i32) (result i32)))
        (memory (export "memory") 1)

        (func $calc (param $a i32) (param $b i32) (result i32)
            (call $mul (call $add (local.get $a) (local.get $b)) (i32.const 2))
        )
        (export "calc" (func $calc))
    )
    "#;

    let wasm_bytes = wat::parse_str(module_wat).expect("parse WAT");
    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("load module");

    let mut instance = module
        .instantiate_with_host((), |builder| {
            builder.register_provider(&MathProvider)?;
            Ok(())
        })
        .expect("instantiate");

    let result = instance.call_i32_i32_to_i32("calc", 3, 4).expect("call");
    assert_eq!(result, 14); // (3 + 4) * 2 = 14
}

#[test]
fn test_backward_compatibility() {
    // Ensure the old API still works
    use pack::runtime::HostImports;

    let module_wat = r#"
    (module
        (import "host" "log" (func $log (param i32 i32)))
        (import "host" "alloc" (func $alloc (param i32) (result i32)))
        (memory (export "memory") 1)

        ;; Takes two i32 args for API compatibility, ignores them
        (func $test (param $a i32) (param $b i32) (result i32)
            ;; Store "test" at offset 0
            (i32.store8 (i32.const 0) (i32.const 116))
            (i32.store8 (i32.const 1) (i32.const 101))
            (i32.store8 (i32.const 2) (i32.const 115))
            (i32.store8 (i32.const 3) (i32.const 116))
            (call $log (i32.const 0) (i32.const 4))

            ;; Allocate some memory
            (call $alloc (i32.const 100))
        )
        (export "test" (func $test))
    )
    "#;

    let wasm_bytes = wat::parse_str(module_wat).expect("parse WAT");
    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("load module");

    let imports = HostImports::new();
    let mut instance = module
        .instantiate_with_imports(imports)
        .expect("instantiate");

    let ptr = instance.call_i32_i32_to_i32("test", 0, 0).expect("call");

    // Verify log was captured
    let logs = instance.get_logs();
    assert_eq!(logs, vec!["test"]);

    // Verify allocation happened (pointer should be >= 48KB base)
    assert!(ptr >= 48 * 1024);
}

// ============================================================================
// Async Host Function Tests
// ============================================================================

#[tokio::test]
async fn test_async_runtime_basic() {
    use pack::AsyncRuntime;

    // Simple module that echoes input - guest-allocates ABI
    // (in_ptr, in_len, out_ptr_ptr, out_len_ptr) -> status
    let module_wat = r#"
    (module
        (memory (export "memory") 1)

        ;; Output data offset
        (global $output_offset i32 (i32.const 16392))

        ;; Echo function: copies input to output area, writes ptr/len to slots
        (func $echo (param $in_ptr i32) (param $in_len i32) (param $out_ptr_ptr i32) (param $out_len_ptr i32) (result i32)
            ;; Copy input to output area
            (memory.copy (global.get $output_offset) (local.get $in_ptr) (local.get $in_len))
            ;; Write output ptr/len to caller's slots
            (i32.store (local.get $out_ptr_ptr) (global.get $output_offset))
            (i32.store (local.get $out_len_ptr) (local.get $in_len))
            ;; Return success
            (i32.const 0)
        )
        (export "echo" (func $echo))
    )
    "#;

    let wasm_bytes = wat::parse_str(module_wat).expect("parse WAT");
    let runtime = AsyncRuntime::new();
    let module = runtime.load_module(&wasm_bytes).expect("load module");

    let mut instance = module.instantiate_async().await.expect("instantiate");

    // Call with value async
    let input = Value::S64(42);
    let output = instance
        .call_with_value_async("echo", &input)
        .await
        .expect("call");

    assert_eq!(output, input);
}

#[tokio::test]
async fn test_func_async_registration() {
    use pack::AsyncRuntime;

    // Module that calls an async host function - guest-allocates ABI
    let module_wat = r#"
    (module
        (import "test" "async_double" (func $async_double (param i32 i32 i32 i32) (result i32)))
        (memory (export "memory") 1)

        ;; Reserve space for result slots
        (global $result_ptr_offset i32 (i32.const 16384))
        (global $result_len_offset i32 (i32.const 16388))
        (global $output_offset i32 (i32.const 16392))

        ;; Wrapper that calls async host function with guest-allocates ABI
        (func $call_async (param $in_ptr i32) (param $in_len i32) (param $out_ptr_ptr i32) (param $out_len_ptr i32) (result i32)
            (local $status i32)
            (local $result_ptr i32)
            (local $result_len i32)

            ;; Call host function
            (local.set $status
                (call $async_double
                    (local.get $in_ptr)
                    (local.get $in_len)
                    (global.get $result_ptr_offset)
                    (global.get $result_len_offset)))

            (if (i32.ne (local.get $status) (i32.const 0))
                (then (return (local.get $status))))

            ;; Read result
            (local.set $result_ptr (i32.load (global.get $result_ptr_offset)))
            (local.set $result_len (i32.load (global.get $result_len_offset)))

            ;; Copy to output area
            (memory.copy (global.get $output_offset) (local.get $result_ptr) (local.get $result_len))

            ;; Write to caller's slots
            (i32.store (local.get $out_ptr_ptr) (global.get $output_offset))
            (i32.store (local.get $out_len_ptr) (local.get $result_len))

            (i32.const 0)
        )

        (export "call_async" (func $call_async))
    )
    "#;

    let wasm_bytes = wat::parse_str(module_wat).expect("parse WAT");
    let runtime = AsyncRuntime::new();
    let module = runtime.load_module(&wasm_bytes).expect("load module");

    let mut instance = module
        .instantiate_with_host_async((), |builder| {
            builder.interface("test")?.func_async(
                "async_double",
                |_ctx: pack::AsyncCtx<()>, input: Value| async move {
                    // Simulate async operation
                    match input {
                        Value::S64(n) => Value::S64(n * 2),
                        other => other,
                    }
                },
            )?;
            Ok(())
        })
        .await
        .expect("instantiate");

    // Use call_with_value_async which handles the full Graph ABI flow
    let input = Value::S64(21);
    let output = instance
        .call_with_value_async("call_async", &input)
        .await
        .expect("call");

    assert_eq!(output, Value::S64(42)); // 21 * 2 = 42
}

#[tokio::test]
async fn test_async_ctx_state_access() {
    use pack::AsyncRuntime;

    /// State that holds a multiplier
    #[derive(Clone)]
    struct MultiplierState {
        multiplier: i64,
    }

    // Module that calls an async host function - guest-allocates ABI
    let module_wat = r#"
    (module
        (import "math" "multiply" (func $multiply (param i32 i32 i32 i32) (result i32)))
        (memory (export "memory") 1)

        ;; Reserve space for result slots
        (global $result_ptr_offset i32 (i32.const 16384))
        (global $result_len_offset i32 (i32.const 16388))
        (global $output_offset i32 (i32.const 16392))

        (func $call_multiply (param $in_ptr i32) (param $in_len i32) (param $out_ptr_ptr i32) (param $out_len_ptr i32) (result i32)
            (local $status i32)
            (local $result_ptr i32)
            (local $result_len i32)

            ;; Call host function
            (local.set $status
                (call $multiply
                    (local.get $in_ptr)
                    (local.get $in_len)
                    (global.get $result_ptr_offset)
                    (global.get $result_len_offset)))

            (if (i32.ne (local.get $status) (i32.const 0))
                (then (return (local.get $status))))

            ;; Read result
            (local.set $result_ptr (i32.load (global.get $result_ptr_offset)))
            (local.set $result_len (i32.load (global.get $result_len_offset)))

            ;; Copy to output area
            (memory.copy (global.get $output_offset) (local.get $result_ptr) (local.get $result_len))

            ;; Write to caller's slots
            (i32.store (local.get $out_ptr_ptr) (global.get $output_offset))
            (i32.store (local.get $out_len_ptr) (local.get $result_len))

            (i32.const 0)
        )

        (export "call_multiply" (func $call_multiply))
    )
    "#;

    let wasm_bytes = wat::parse_str(module_wat).expect("parse WAT");
    let runtime = AsyncRuntime::new();
    let module = runtime.load_module(&wasm_bytes).expect("load module");

    // Create state with a multiplier of 10
    let state = MultiplierState { multiplier: 10 };

    let mut instance = module
        .instantiate_with_host_async(state, |builder| {
            builder.interface("math")?.func_async(
                "multiply",
                |ctx: pack::AsyncCtx<MultiplierState>, input: Value| async move {
                    // Access state through ctx.data()
                    let multiplier = ctx.data().multiplier;
                    match input {
                        Value::S64(n) => Value::S64(n * multiplier),
                        other => other,
                    }
                },
            )?;
            Ok(())
        })
        .await
        .expect("instantiate");

    // 7 * 10 (from state) = 70
    let input = Value::S64(7);
    let output = instance
        .call_with_value_async("call_multiply", &input)
        .await
        .expect("call");

    assert_eq!(output, Value::S64(70));
}

#[test]
fn test_error_handler_callback() {
    use pack::{HostFunctionError, HostFunctionErrorKind};
    use std::sync::{Arc, Mutex};

    // Track errors via a shared vec
    let errors: Arc<Mutex<Vec<HostFunctionError>>> = Arc::new(Mutex::new(Vec::new()));
    let errors_clone = errors.clone();

    // Module that:
    // 1. Has a host function that uses typed interface (new calling convention)
    // 2. Exports a function that writes bad data and calls the host function
    let module_wat = r#"
    (module
        (import "test" "process" (func $process (param i32 i32 i32 i32) (result i32)))
        (memory (export "memory") 1)

        ;; Write garbage data to memory and call the host function
        ;; Returns i32 result directly (new calling convention)
        (func $trigger_error (param $unused i32) (param $unused2 i32) (result i32)
            ;; Write invalid Graph ABI data at offset 100
            (i32.store (i32.const 100) (i32.const 0xDEADBEEF))
            ;; Call host function with bad data
            ;; Args: in_ptr=100, in_len=4, out_ptr=200, out_cap=100
            (call $process (i32.const 100) (i32.const 4) (i32.const 200) (i32.const 100))
        )

        (export "trigger_error" (func $trigger_error))
    )
    "#;

    let wasm_bytes = wat::parse_str(module_wat).expect("parse WAT");
    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("load module");

    let mut instance = module
        .instantiate_with_host((), |builder| {
            // Set custom error handler
            builder.on_error(move |err| {
                errors_clone.lock().unwrap().push(err.clone());
            });

            builder.interface("test")?.func_typed(
                "process",
                |_ctx: &mut pack::Ctx<'_, ()>, input: Value| -> Value {
                    // This will never be reached due to decode error
                    input
                },
            )?;
            Ok(())
        })
        .expect("instantiate");

    // Call the function that writes bad data and triggers the host call
    let result = instance
        .call_i32_i32_to_i32("trigger_error", 0, 0)
        .expect("call");

    // Should return -1 (error indicator from host function)
    assert_eq!(result, -1);

    // Check that our error handler was called
    let captured_errors = errors.lock().unwrap();
    assert_eq!(captured_errors.len(), 1);
    assert_eq!(captured_errors[0].interface, "test");
    assert_eq!(captured_errors[0].function, "process");
    assert!(matches!(
        captured_errors[0].kind,
        HostFunctionErrorKind::Decode(_)
    ));
}

// ============================================================================
// Result Encoding Tests (Bug fix verification)
// ============================================================================

/// Test that func_typed_result encodes Ok results as Value::Result, not Value::Variant
#[test]
fn test_func_typed_result_encodes_as_value_result_ok() {
    // Module that wraps a host function call - the host function uses Graph ABI
    let module_wat = r#"
    (module
        (import "test" "maybe_double" (func $maybe_double (param i32 i32 i32 i32) (result i32)))
        (memory (export "memory") 1)

        (global $result_ptr_offset i32 (i32.const 16384))
        (global $result_len_offset i32 (i32.const 16388))
        (global $output_offset i32 (i32.const 16392))

        (func $call_maybe_double (param $in_ptr i32) (param $in_len i32) (param $out_ptr_ptr i32) (param $out_len_ptr i32) (result i32)
            (local $status i32)
            (local $result_ptr i32)
            (local $result_len i32)

            (local.set $status
                (call $maybe_double
                    (local.get $in_ptr)
                    (local.get $in_len)
                    (global.get $result_ptr_offset)
                    (global.get $result_len_offset)))

            (if (i32.ne (local.get $status) (i32.const 0))
                (then (return (local.get $status))))

            (local.set $result_ptr (i32.load (global.get $result_ptr_offset)))
            (local.set $result_len (i32.load (global.get $result_len_offset)))

            (memory.copy (global.get $output_offset) (local.get $result_ptr) (local.get $result_len))

            (i32.store (local.get $out_ptr_ptr) (global.get $output_offset))
            (i32.store (local.get $out_len_ptr) (local.get $result_len))

            (i32.const 0)
        )

        (export "call_maybe_double" (func $call_maybe_double))
    )
    "#;

    let wasm_bytes = wat::parse_str(module_wat).expect("parse WAT");
    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("load module");

    let mut instance = module
        .instantiate_with_host((), |builder| {
            builder.interface("test")?.func_typed_result(
                "maybe_double",
                |_ctx: &mut pack::Ctx<'_, ()>, input: Value| -> Result<Value, Value> {
                    // Return Ok with doubled value
                    match input {
                        Value::S64(n) => Ok(Value::S64(n * 2)),
                        other => Ok(other),
                    }
                },
            )?;
            Ok(())
        })
        .expect("instantiate");

    let input = Value::S64(21);
    let output = instance
        .call_with_value("call_maybe_double", &input)
        .expect("call");

    // Verify output is Value::Result with Ok variant, NOT Value::Variant
    match output {
        Value::Result { value: Ok(inner), .. } => {
            assert_eq!(*inner, Value::S64(42)); // 21 * 2 = 42
        }
        Value::Variant { type_name, case_name, .. } => {
            panic!(
                "Expected Value::Result but got Value::Variant(type_name={}, case_name={})",
                type_name, case_name
            );
        }
        other => {
            panic!("Expected Value::Result but got {:?}", other);
        }
    }
}

/// Test that func_typed_result encodes Err results as Value::Result, not Value::Variant
#[test]
fn test_func_typed_result_encodes_as_value_result_err() {
    let module_wat = r#"
    (module
        (import "test" "maybe_double" (func $maybe_double (param i32 i32 i32 i32) (result i32)))
        (memory (export "memory") 1)

        (global $result_ptr_offset i32 (i32.const 16384))
        (global $result_len_offset i32 (i32.const 16388))
        (global $output_offset i32 (i32.const 16392))

        (func $call_maybe_double (param $in_ptr i32) (param $in_len i32) (param $out_ptr_ptr i32) (param $out_len_ptr i32) (result i32)
            (local $status i32)
            (local $result_ptr i32)
            (local $result_len i32)

            (local.set $status
                (call $maybe_double
                    (local.get $in_ptr)
                    (local.get $in_len)
                    (global.get $result_ptr_offset)
                    (global.get $result_len_offset)))

            (if (i32.ne (local.get $status) (i32.const 0))
                (then (return (local.get $status))))

            (local.set $result_ptr (i32.load (global.get $result_ptr_offset)))
            (local.set $result_len (i32.load (global.get $result_len_offset)))

            (memory.copy (global.get $output_offset) (local.get $result_ptr) (local.get $result_len))

            (i32.store (local.get $out_ptr_ptr) (global.get $output_offset))
            (i32.store (local.get $out_len_ptr) (local.get $result_len))

            (i32.const 0)
        )

        (export "call_maybe_double" (func $call_maybe_double))
    )
    "#;

    let wasm_bytes = wat::parse_str(module_wat).expect("parse WAT");
    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("load module");

    let mut instance = module
        .instantiate_with_host((), |builder| {
            builder.interface("test")?.func_typed_result(
                "maybe_double",
                |_ctx: &mut pack::Ctx<'_, ()>, input: Value| -> Result<Value, Value> {
                    // Return Err for negative numbers
                    match input {
                        Value::S64(n) if n < 0 => Err(Value::String("negative not allowed".to_string())),
                        Value::S64(n) => Ok(Value::S64(n * 2)),
                        other => Ok(other),
                    }
                },
            )?;
            Ok(())
        })
        .expect("instantiate");

    // Pass negative number to trigger error
    let input = Value::S64(-5);
    let output = instance
        .call_with_value("call_maybe_double", &input)
        .expect("call");

    // Verify output is Value::Result with Err variant, NOT Value::Variant
    match output {
        Value::Result { value: Err(inner), .. } => {
            assert_eq!(*inner, Value::String("negative not allowed".to_string()));
        }
        Value::Variant { type_name, case_name, .. } => {
            panic!(
                "Expected Value::Result but got Value::Variant(type_name={}, case_name={})",
                type_name, case_name
            );
        }
        other => {
            panic!("Expected Value::Result but got {:?}", other);
        }
    }
}

/// Test that func_async_result encodes Ok results as Value::Result, not Value::Variant
#[tokio::test]
async fn test_func_async_result_encodes_as_value_result_ok() {
    use pack::AsyncRuntime;

    let module_wat = r#"
    (module
        (import "test" "async_maybe_double" (func $async_maybe_double (param i32 i32 i32 i32) (result i32)))
        (memory (export "memory") 1)

        (global $result_ptr_offset i32 (i32.const 16384))
        (global $result_len_offset i32 (i32.const 16388))
        (global $output_offset i32 (i32.const 16392))

        (func $call_async (param $in_ptr i32) (param $in_len i32) (param $out_ptr_ptr i32) (param $out_len_ptr i32) (result i32)
            (local $status i32)
            (local $result_ptr i32)
            (local $result_len i32)

            (local.set $status
                (call $async_maybe_double
                    (local.get $in_ptr)
                    (local.get $in_len)
                    (global.get $result_ptr_offset)
                    (global.get $result_len_offset)))

            (if (i32.ne (local.get $status) (i32.const 0))
                (then (return (local.get $status))))

            (local.set $result_ptr (i32.load (global.get $result_ptr_offset)))
            (local.set $result_len (i32.load (global.get $result_len_offset)))

            (memory.copy (global.get $output_offset) (local.get $result_ptr) (local.get $result_len))

            (i32.store (local.get $out_ptr_ptr) (global.get $output_offset))
            (i32.store (local.get $out_len_ptr) (local.get $result_len))

            (i32.const 0)
        )

        (export "call_async" (func $call_async))
    )
    "#;

    let wasm_bytes = wat::parse_str(module_wat).expect("parse WAT");
    let runtime = AsyncRuntime::new();
    let module = runtime.load_module(&wasm_bytes).expect("load module");

    let mut instance = module
        .instantiate_with_host_async((), |builder| {
            builder.interface("test")?.func_async_result(
                "async_maybe_double",
                |_ctx: pack::AsyncCtx<()>, input: Value| async move {
                    // Return Ok with doubled value
                    let result: Result<Value, Value> = match input {
                        Value::S64(n) => Ok(Value::S64(n * 2)),
                        other => Ok(other),
                    };
                    result
                },
            )?;
            Ok(())
        })
        .await
        .expect("instantiate");

    let input = Value::S64(21);
    let output = instance
        .call_with_value_async("call_async", &input)
        .await
        .expect("call");

    // Verify output is Value::Result with Ok variant, NOT Value::Variant
    match output {
        Value::Result { value: Ok(inner), .. } => {
            assert_eq!(*inner, Value::S64(42)); // 21 * 2 = 42
        }
        Value::Variant { type_name, case_name, .. } => {
            panic!(
                "Expected Value::Result but got Value::Variant(type_name={}, case_name={})",
                type_name, case_name
            );
        }
        other => {
            panic!("Expected Value::Result but got {:?}", other);
        }
    }
}

/// Test that func_async_result encodes Err results as Value::Result, not Value::Variant
#[tokio::test]
async fn test_func_async_result_encodes_as_value_result_err() {
    use pack::AsyncRuntime;

    let module_wat = r#"
    (module
        (import "test" "async_maybe_double" (func $async_maybe_double (param i32 i32 i32 i32) (result i32)))
        (memory (export "memory") 1)

        (global $result_ptr_offset i32 (i32.const 16384))
        (global $result_len_offset i32 (i32.const 16388))
        (global $output_offset i32 (i32.const 16392))

        (func $call_async (param $in_ptr i32) (param $in_len i32) (param $out_ptr_ptr i32) (param $out_len_ptr i32) (result i32)
            (local $status i32)
            (local $result_ptr i32)
            (local $result_len i32)

            (local.set $status
                (call $async_maybe_double
                    (local.get $in_ptr)
                    (local.get $in_len)
                    (global.get $result_ptr_offset)
                    (global.get $result_len_offset)))

            (if (i32.ne (local.get $status) (i32.const 0))
                (then (return (local.get $status))))

            (local.set $result_ptr (i32.load (global.get $result_ptr_offset)))
            (local.set $result_len (i32.load (global.get $result_len_offset)))

            (memory.copy (global.get $output_offset) (local.get $result_ptr) (local.get $result_len))

            (i32.store (local.get $out_ptr_ptr) (global.get $output_offset))
            (i32.store (local.get $out_len_ptr) (local.get $result_len))

            (i32.const 0)
        )

        (export "call_async" (func $call_async))
    )
    "#;

    let wasm_bytes = wat::parse_str(module_wat).expect("parse WAT");
    let runtime = AsyncRuntime::new();
    let module = runtime.load_module(&wasm_bytes).expect("load module");

    let mut instance = module
        .instantiate_with_host_async((), |builder| {
            builder.interface("test")?.func_async_result(
                "async_maybe_double",
                |_ctx: pack::AsyncCtx<()>, input: Value| async move {
                    // Return Err for negative numbers
                    let result: Result<Value, Value> = match input {
                        Value::S64(n) if n < 0 => Err(Value::String("negative not allowed".to_string())),
                        Value::S64(n) => Ok(Value::S64(n * 2)),
                        other => Ok(other),
                    };
                    result
                },
            )?;
            Ok(())
        })
        .await
        .expect("instantiate");

    // Pass negative number to trigger error
    let input = Value::S64(-5);
    let output = instance
        .call_with_value_async("call_async", &input)
        .await
        .expect("call");

    // Verify output is Value::Result with Err variant, NOT Value::Variant
    match output {
        Value::Result { value: Err(inner), .. } => {
            assert_eq!(*inner, Value::String("negative not allowed".to_string()));
        }
        Value::Variant { type_name, case_name, .. } => {
            panic!(
                "Expected Value::Result but got Value::Variant(type_name={}, case_name={})",
                type_name, case_name
            );
        }
        other => {
            panic!("Expected Value::Result but got {:?}", other);
        }
    }
}

// ============================================================================
// Type Inference Tests (Bug fix verification for different type sizes)
// ============================================================================

/// Test that func_typed_result correctly encodes Result<Vec<u8>, String> types
/// even when returning Err (so we don't have an Ok value to infer from).
///
/// This is the specific case from the bug report: when ok_type is List<U8> but
/// we only have an Err value, the ok_type should still be List<U8>, not String.
#[test]
fn test_func_typed_result_preserves_ok_type_on_err() {
    use pack::abi::ValueType;

    let module_wat = r#"
    (module
        (import "test" "fetch_bytes" (func $fetch_bytes (param i32 i32 i32 i32) (result i32)))
        (memory (export "memory") 1)

        (global $result_ptr_offset i32 (i32.const 16384))
        (global $result_len_offset i32 (i32.const 16388))
        (global $output_offset i32 (i32.const 16392))

        (func $call_fetch (param $in_ptr i32) (param $in_len i32) (param $out_ptr_ptr i32) (param $out_len_ptr i32) (result i32)
            (local $status i32)
            (local $result_ptr i32)
            (local $result_len i32)

            (local.set $status
                (call $fetch_bytes
                    (local.get $in_ptr)
                    (local.get $in_len)
                    (global.get $result_ptr_offset)
                    (global.get $result_len_offset)))

            (if (i32.ne (local.get $status) (i32.const 0))
                (then (return (local.get $status))))

            (local.set $result_ptr (i32.load (global.get $result_ptr_offset)))
            (local.set $result_len (i32.load (global.get $result_len_offset)))

            (memory.copy (global.get $output_offset) (local.get $result_ptr) (local.get $result_len))

            (i32.store (local.get $out_ptr_ptr) (global.get $output_offset))
            (i32.store (local.get $out_len_ptr) (local.get $result_len))

            (i32.const 0)
        )

        (export "call_fetch" (func $call_fetch))
    )
    "#;

    let wasm_bytes = wat::parse_str(module_wat).expect("parse WAT");
    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("load module");

    let mut instance = module
        .instantiate_with_host((), |builder| {
            // Register function returning Result<Vec<u8>, String>
            builder.interface("test")?.func_typed_result(
                "fetch_bytes",
                |_ctx: &mut pack::Ctx<'_, ()>, _input: Value| -> Result<Vec<u8>, String> {
                    // Return error - we don't have an Ok value to infer from
                    Err("Resource not found".to_string())
                },
            )?;
            Ok(())
        })
        .expect("instantiate");

    let input = Value::String("some_url".to_string());
    let output = instance
        .call_with_value("call_fetch", &input)
        .expect("call");

    // Verify output has correct types even though we returned Err
    match output {
        Value::Result { ok_type, err_type, value: Err(inner) } => {
            // ok_type should be List<U8>, NOT String (the bug was defaulting to String)
            assert_eq!(ok_type, ValueType::List(Box::new(ValueType::U8)),
                "ok_type should be List<U8>, not {:?}", ok_type);
            assert_eq!(err_type, ValueType::String);
            assert_eq!(*inner, Value::String("Resource not found".to_string()));
        }
        other => {
            panic!("Expected Value::Result with Err, got {:?}", other);
        }
    }
}

/// Test that func_typed_result correctly encodes Result<String, Vec<u8>> types
/// when returning Ok (so we don't have an Err value to infer from).
#[test]
fn test_func_typed_result_preserves_err_type_on_ok() {
    use pack::abi::ValueType;

    let module_wat = r#"
    (module
        (import "test" "process" (func $process (param i32 i32 i32 i32) (result i32)))
        (memory (export "memory") 1)

        (global $result_ptr_offset i32 (i32.const 16384))
        (global $result_len_offset i32 (i32.const 16388))
        (global $output_offset i32 (i32.const 16392))

        (func $call_process (param $in_ptr i32) (param $in_len i32) (param $out_ptr_ptr i32) (param $out_len_ptr i32) (result i32)
            (local $status i32)
            (local $result_ptr i32)
            (local $result_len i32)

            (local.set $status
                (call $process
                    (local.get $in_ptr)
                    (local.get $in_len)
                    (global.get $result_ptr_offset)
                    (global.get $result_len_offset)))

            (if (i32.ne (local.get $status) (i32.const 0))
                (then (return (local.get $status))))

            (local.set $result_ptr (i32.load (global.get $result_ptr_offset)))
            (local.set $result_len (i32.load (global.get $result_len_offset)))

            (memory.copy (global.get $output_offset) (local.get $result_ptr) (local.get $result_len))

            (i32.store (local.get $out_ptr_ptr) (global.get $output_offset))
            (i32.store (local.get $out_len_ptr) (local.get $result_len))

            (i32.const 0)
        )

        (export "call_process" (func $call_process))
    )
    "#;

    let wasm_bytes = wat::parse_str(module_wat).expect("parse WAT");
    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("load module");

    let mut instance = module
        .instantiate_with_host((), |builder| {
            // Register function returning Result<String, Vec<u8>>
            builder.interface("test")?.func_typed_result(
                "process",
                |_ctx: &mut pack::Ctx<'_, ()>, _input: Value| -> Result<String, Vec<u8>> {
                    // Return success - we don't have an Err value to infer from
                    Ok("Success!".to_string())
                },
            )?;
            Ok(())
        })
        .expect("instantiate");

    let input = Value::String("data".to_string());
    let output = instance
        .call_with_value("call_process", &input)
        .expect("call");

    // Verify output has correct types even though we returned Ok
    match output {
        Value::Result { ok_type, err_type, value: Ok(inner) } => {
            assert_eq!(ok_type, ValueType::String);
            // err_type should be List<U8>, NOT String (the bug was defaulting to String)
            assert_eq!(err_type, ValueType::List(Box::new(ValueType::U8)),
                "err_type should be List<U8>, not {:?}", err_type);
            assert_eq!(*inner, Value::String("Success!".to_string()));
        }
        other => {
            panic!("Expected Value::Result with Ok, got {:?}", other);
        }
    }
}
