//! Integration tests for the new host function registration API

use composite::abi::Value;
use composite::runtime::{HostLinkerBuilder, LinkerError};
use composite::Runtime;
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
    // but the wrapper just echoes back (for simpler testing)
    let module_wat = r#"
    (module
        (import "test" "double" (func $double (param i32 i32) (result i64)))
        (memory (export "memory") 1)

        ;; Wrapper that calls host function with Graph ABI signature
        (func $call_double (param $in_ptr i32) (param $in_len i32) (result i64)
            (call $double (local.get $in_ptr) (local.get $in_len))
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
                |_ctx: &mut composite::Ctx<'_, ()>, input: Value| -> Value {
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
        .call_with_value("call_double", &input, 0)
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
    use composite::runtime::HostFunctionProvider;

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
    use composite::runtime::HostImports;

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
    use composite::AsyncRuntime;

    // Simple module that echoes input
    let module_wat = r#"
    (module
        (memory (export "memory") 1)

        ;; Echo function: takes (ptr, len), returns packed (ptr, len)
        (func $echo (param $in_ptr i32) (param $in_len i32) (result i64)
            (i64.or
                (i64.extend_i32_u (local.get $in_ptr))
                (i64.shl
                    (i64.extend_i32_u (local.get $in_len))
                    (i64.const 32)))
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
        .call_with_value_async("echo", &input, 0)
        .await
        .expect("call");

    assert_eq!(output, input);
}

#[tokio::test]
async fn test_func_async_registration() {
    use composite::AsyncRuntime;

    // Module that calls an async host function
    let module_wat = r#"
    (module
        (import "test" "async_double" (func $async_double (param i32 i32) (result i64)))
        (memory (export "memory") 1)

        ;; Wrapper that calls async host function
        (func $call_async (param $in_ptr i32) (param $in_len i32) (result i64)
            (call $async_double (local.get $in_ptr) (local.get $in_len))
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
                |_ctx: composite::AsyncCtx<()>, input: Value| async move {
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
        .call_with_value_async("call_async", &input, 0)
        .await
        .expect("call");

    assert_eq!(output, Value::S64(42)); // 21 * 2 = 42
}
