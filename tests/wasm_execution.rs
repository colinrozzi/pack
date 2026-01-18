//! Basic WASM execution tests
//!
//! These tests verify that we can load and run WASM modules through the Runtime.

use composite::abi::Value;
use composite::Runtime;
use std::path::Path;

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

// ============================================================================
// Memory access tests
// ============================================================================

/// A module with memory - sums bytes in a range
const SUM_BYTES_MODULE: &str = r#"
(module
    (memory (export "memory") 1)

    ;; Sum bytes from ptr to ptr+len
    (func $sum_bytes (param $ptr i32) (param $len i32) (result i32)
        (local $sum i32)
        (local $end i32)

        ;; end = ptr + len
        (local.set $end (i32.add (local.get $ptr) (local.get $len)))

        ;; while ptr < end
        (block $break
            (loop $continue
                ;; if ptr >= end, break
                (br_if $break (i32.ge_u (local.get $ptr) (local.get $end)))

                ;; sum += *ptr
                (local.set $sum
                    (i32.add
                        (local.get $sum)
                        (i32.load8_u (local.get $ptr))))

                ;; ptr++
                (local.set $ptr (i32.add (local.get $ptr) (i32.const 1)))

                (br $continue)
            )
        )

        (local.get $sum)
    )
    (export "sum_bytes" (func $sum_bytes))
)
"#;

#[test]
fn memory_read_write() {
    let wasm_bytes = wat::parse_str(SUM_BYTES_MODULE).expect("failed to parse WAT");

    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load module");
    let mut instance = module.instantiate().expect("failed to instantiate");

    // Write some bytes to memory
    let data = [1u8, 2, 3, 4, 5];
    instance.write_memory(0, &data).expect("failed to write memory");

    // Call sum_bytes(0, 5) - should return 1+2+3+4+5 = 15
    let result = instance
        .call_i32_i32_to_i32("sum_bytes", 0, 5)
        .expect("failed to call sum_bytes");
    assert_eq!(result, 15);

    // Read the bytes back
    let read_back = instance.read_memory(0, 5).expect("failed to read memory");
    assert_eq!(read_back, data);
}

#[test]
fn memory_string_roundtrip() {
    let wasm_bytes = wat::parse_str(SUM_BYTES_MODULE).expect("failed to parse WAT");

    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load module");
    let mut instance = module.instantiate().expect("failed to instantiate");

    // Write a string to memory
    let message = "Hello, WebAssembly!";
    instance
        .write_memory(100, message.as_bytes())
        .expect("failed to write string");

    // Read it back
    let read_back = instance
        .read_memory(100, message.len())
        .expect("failed to read string");

    let read_string = String::from_utf8(read_back).expect("invalid utf8");
    assert_eq!(read_string, message);

    // Sum the bytes (just to prove the module can read our string)
    let sum: i32 = message.as_bytes().iter().map(|&b| b as i32).sum();
    let result = instance
        .call_i32_i32_to_i32("sum_bytes", 100, message.len() as i32)
        .expect("failed to call sum_bytes");
    assert_eq!(result, sum);
}

/// A module that reverses bytes in place
const REVERSE_MODULE: &str = r#"
(module
    (memory (export "memory") 1)

    ;; Reverse bytes from ptr to ptr+len in place
    (func $reverse (param $ptr i32) (param $len i32)
        (local $left i32)
        (local $right i32)
        (local $tmp i32)

        (local.set $left (local.get $ptr))
        (local.set $right (i32.sub (i32.add (local.get $ptr) (local.get $len)) (i32.const 1)))

        (block $break
            (loop $continue
                ;; if left >= right, break
                (br_if $break (i32.ge_u (local.get $left) (local.get $right)))

                ;; swap *left and *right
                (local.set $tmp (i32.load8_u (local.get $left)))
                (i32.store8 (local.get $left) (i32.load8_u (local.get $right)))
                (i32.store8 (local.get $right) (local.get $tmp))

                ;; left++, right--
                (local.set $left (i32.add (local.get $left) (i32.const 1)))
                (local.set $right (i32.sub (local.get $right) (i32.const 1)))

                (br $continue)
            )
        )
    )
    (export "reverse" (func $reverse))
)
"#;

#[test]
fn memory_reverse_string() {
    let wasm_bytes = wat::parse_str(REVERSE_MODULE).expect("failed to parse WAT");

    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load module");
    let mut instance = module.instantiate().expect("failed to instantiate");

    // Write a string
    let message = "Hello";
    instance
        .write_memory(0, message.as_bytes())
        .expect("failed to write");

    // Call the WASM reverse function
    instance
        .call_i32_i32("reverse", 0, message.len() as i32)
        .expect("failed to call reverse");

    // Read back the reversed string
    let result = instance.read_memory(0, 5).expect("read back");
    assert_eq!(result, b"olleH");
}

// ============================================================================
// Graph ABI tests
// ============================================================================

/// An echo module that copies input bytes to the caller-provided output buffer.
/// New calling convention: (in_ptr, in_len, out_ptr, out_cap) -> out_len
const ECHO_MODULE: &str = r#"
(module
    (memory (export "memory") 1)

    ;; Echo: copy input bytes to caller-provided output buffer
    ;; Returns out_len (bytes written), or -1 if buffer too small
    (func $echo (param $in_ptr i32) (param $in_len i32) (param $out_ptr i32) (param $out_cap i32) (result i32)
        (local $i i32)

        ;; Check if output buffer is large enough
        (if (i32.gt_u (local.get $in_len) (local.get $out_cap))
            (then (return (i32.const -1)))
        )

        ;; Copy input to output buffer using memory.copy
        (memory.copy (local.get $out_ptr) (local.get $in_ptr) (local.get $in_len))

        ;; Return the length
        (local.get $in_len)
    )
    (export "echo" (func $echo))
)
"#;

#[test]
fn graph_abi_echo_roundtrip() {
    let wasm_bytes = wat::parse_str(ECHO_MODULE).expect("failed to parse WAT");

    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load module");
    let mut instance = module.instantiate().expect("failed to instantiate");

    // Test with a simple integer
    let input = Value::S64(42);
    let output = instance
        .call_with_value("echo", &input, 0)
        .expect("failed to call echo");
    assert_eq!(output, input);
}

#[test]
fn graph_abi_echo_string() {
    let wasm_bytes = wat::parse_str(ECHO_MODULE).expect("failed to parse WAT");

    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load module");
    let mut instance = module.instantiate().expect("failed to instantiate");

    let input = Value::String("Hello, Graph ABI!".to_string());
    let output = instance
        .call_with_value("echo", &input, 0)
        .expect("failed to call echo");
    assert_eq!(output, input);
}

#[test]
fn graph_abi_echo_list() {
    let wasm_bytes = wat::parse_str(ECHO_MODULE).expect("failed to parse WAT");

    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load module");
    let mut instance = module.instantiate().expect("failed to instantiate");

    let input = Value::List(vec![
        Value::S64(1),
        Value::S64(2),
        Value::S64(3),
    ]);
    let output = instance
        .call_with_value("echo", &input, 0)
        .expect("failed to call echo");
    assert_eq!(output, input);
}

#[test]
fn graph_abi_echo_variant() {
    let wasm_bytes = wat::parse_str(ECHO_MODULE).expect("failed to parse WAT");

    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load module");
    let mut instance = module.instantiate().expect("failed to instantiate");

    // This is like a simple S-expression node
    let input = Value::Variant {
        tag: 1,
        payload: Some(Box::new(Value::String("symbol".to_string()))),
    };
    let output = instance
        .call_with_value("echo", &input, 0)
        .expect("failed to call echo");
    assert_eq!(output, input);
}

#[test]
fn graph_abi_echo_nested() {
    let wasm_bytes = wat::parse_str(ECHO_MODULE).expect("failed to parse WAT");

    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load module");
    let mut instance = module.instantiate().expect("failed to instantiate");

    // A nested structure like (list (sym "a") (num 42))
    let input = Value::List(vec![
        Value::Variant {
            tag: 0,
            payload: Some(Box::new(Value::String("list".to_string()))),
        },
        Value::List(vec![
            Value::Variant {
                tag: 0,
                payload: Some(Box::new(Value::String("a".to_string()))),
            },
            Value::Variant {
                tag: 1,
                payload: Some(Box::new(Value::S64(42))),
            },
        ]),
    ]);
    let output = instance
        .call_with_value("echo", &input, 0)
        .expect("failed to call echo");
    assert_eq!(output, input);
}

// ============================================================================
// Rust component tests
// ============================================================================

/// Load the Rust-compiled echo component
fn load_rust_echo_component() -> Vec<u8> {
    let wasm_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("components/echo/target/wasm32-unknown-unknown/release/echo_component.wasm");
    std::fs::read(&wasm_path).unwrap_or_else(|e| {
        panic!(
            "Failed to read Rust component at {}: {}. Run: cd components/echo && cargo build --target wasm32-unknown-unknown --release",
            wasm_path.display(),
            e
        )
    })
}

#[test]
fn rust_component_echo_roundtrip() {
    let wasm_bytes = load_rust_echo_component();

    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load Rust component");
    let mut instance = module.instantiate().expect("failed to instantiate");

    // Test with a simple integer
    let input = Value::S64(42);
    let output = instance
        .call_with_value("echo", &input, 0)
        .expect("failed to call echo");
    assert_eq!(output, input);
}

#[test]
fn rust_component_echo_complex() {
    let wasm_bytes = load_rust_echo_component();

    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load Rust component");
    let mut instance = module.instantiate().expect("failed to instantiate");

    // Test with a nested structure
    let input = Value::List(vec![
        Value::String("hello".to_string()),
        Value::Variant {
            tag: 2,
            payload: Some(Box::new(Value::List(vec![
                Value::S64(1),
                Value::S64(2),
                Value::S64(3),
            ]))),
        },
        Value::Option(Some(Box::new(Value::Bool(true)))),
    ]);

    let output = instance
        .call_with_value("echo", &input, 0)
        .expect("failed to call echo");
    assert_eq!(output, input);
}

#[test]
fn rust_component_transform_doubles_s64() {
    let wasm_bytes = load_rust_echo_component();

    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load Rust component");
    let mut instance = module.instantiate().expect("failed to instantiate");

    // Test transform doubles S64
    let input = Value::S64(21);
    let output = instance
        .call_with_value("transform", &input, 0)
        .expect("failed to call transform");
    assert_eq!(output, Value::S64(42));
}

#[test]
fn rust_component_transform_nested() {
    let wasm_bytes = load_rust_echo_component();

    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load Rust component");
    let mut instance = module.instantiate().expect("failed to instantiate");

    // Test transform on nested structure - doubles all S64 values
    let input = Value::List(vec![
        Value::S64(10),
        Value::S64(20),
        Value::Variant {
            tag: 1,
            payload: Some(Box::new(Value::S64(50))),
        },
    ]);

    let expected = Value::List(vec![
        Value::S64(20),  // 10 * 2
        Value::S64(40),  // 20 * 2
        Value::Variant {
            tag: 1,
            payload: Some(Box::new(Value::S64(100))),  // 50 * 2
        },
    ]);

    let output = instance
        .call_with_value("transform", &input, 0)
        .expect("failed to call transform");
    assert_eq!(output, expected);
}

#[test]
fn rust_component_transform_preserves_strings() {
    let wasm_bytes = load_rust_echo_component();

    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load Rust component");
    let mut instance = module.instantiate().expect("failed to instantiate");

    // Strings should pass through unchanged
    let input = Value::String("hello world".to_string());
    let output = instance
        .call_with_value("transform", &input, 0)
        .expect("failed to call transform");
    assert_eq!(output, input);
}

// ============================================================================
// Host imports tests
// ============================================================================

use composite::runtime::HostImports;

/// Load the Rust-compiled logger component (uses host imports)
fn load_rust_logger_component() -> Vec<u8> {
    let wasm_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("components/logger/target/wasm32-unknown-unknown/release/logger_component.wasm");
    std::fs::read(&wasm_path).unwrap_or_else(|e| {
        panic!(
            "Failed to read logger component at {}: {}. Run: cd components/logger && cargo build --target wasm32-unknown-unknown --release",
            wasm_path.display(),
            e
        )
    })
}

#[test]
fn host_imports_logging() {
    let wasm_bytes = load_rust_logger_component();

    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load logger component");

    // Instantiate with host imports
    let imports = HostImports::new();
    let mut instance = module
        .instantiate_with_imports(imports)
        .expect("failed to instantiate with imports");

    // Call process with a value - this should log messages
    let input = Value::S64(42);
    let output = instance
        .call_with_value("process", &input, 0)
        .expect("failed to call process");

    // The transform doubles S64 values
    assert_eq!(output, Value::S64(84));

    // Check that we captured log messages
    let logs = instance.get_logs();
    assert!(!logs.is_empty(), "Expected log messages");

    // Verify specific log messages
    assert!(logs.iter().any(|m| m.contains("starting")), "Expected 'starting' log");
    assert!(logs.iter().any(|m| m.contains("got S64")), "Expected S64 type log");
    assert!(logs.iter().any(|m| m.contains("done")), "Expected 'done' log");
}

#[test]
fn host_imports_logging_with_string() {
    let wasm_bytes = load_rust_logger_component();

    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load logger component");

    let imports = HostImports::new();
    let mut instance = module
        .instantiate_with_imports(imports)
        .expect("failed to instantiate with imports");

    // Call with a string value
    let input = Value::String("test message".to_string());
    let output = instance
        .call_with_value("process", &input, 0)
        .expect("failed to call process");

    // Strings pass through unchanged
    assert_eq!(output, input);

    let logs = instance.get_logs();
    assert!(logs.iter().any(|m| m.contains("got String")), "Expected String type log");
    assert!(logs.iter().any(|m| m.contains("test message")), "Expected the actual string in logs");
}

#[test]
fn host_imports_transform_nested() {
    let wasm_bytes = load_rust_logger_component();

    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load logger component");

    let imports = HostImports::new();
    let mut instance = module
        .instantiate_with_imports(imports)
        .expect("failed to instantiate with imports");

    // Test with a list of S64 values
    let input = Value::List(vec![
        Value::S64(10),
        Value::S64(20),
        Value::S64(30),
    ]);

    let expected = Value::List(vec![
        Value::S64(20),
        Value::S64(40),
        Value::S64(60),
    ]);

    let output = instance
        .call_with_value("process", &input, 0)
        .expect("failed to call process");

    assert_eq!(output, expected);

    // Verify logging happened
    let logs = instance.get_logs();
    assert!(logs.iter().any(|m| m.contains("got List")), "Expected List type log");
}

#[test]
fn host_imports_clear_logs() {
    let wasm_bytes = load_rust_logger_component();

    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load logger component");

    let imports = HostImports::new();
    let mut instance = module
        .instantiate_with_imports(imports)
        .expect("failed to instantiate with imports");

    // First call
    let input = Value::S64(1);
    let _ = instance.call_with_value("process", &input, 0).unwrap();

    let logs1 = instance.get_logs();
    assert!(!logs1.is_empty());

    // Clear logs
    instance.clear_logs();
    assert!(instance.get_logs().is_empty(), "Logs should be cleared");

    // Second call
    let _ = instance.call_with_value("process", &input, 0).unwrap();
    let logs2 = instance.get_logs();
    assert!(!logs2.is_empty());

    // Should only have logs from second call
    assert!(logs2.len() < logs1.len() * 2, "Should only have logs from second call");
}

// ============================================================================
// S-expression evaluator tests
// ============================================================================

/// Load the Rust-compiled sexpr component
fn load_rust_sexpr_component() -> Vec<u8> {
    let wasm_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("components/sexpr/target/wasm32-unknown-unknown/release/sexpr_component.wasm");
    std::fs::read(&wasm_path).unwrap_or_else(|e| {
        panic!(
            "Failed to read sexpr component at {}: {}. Run: cd components/sexpr && cargo build --target wasm32-unknown-unknown --release",
            wasm_path.display(),
            e
        )
    })
}

/// Helper to create an SExpr value for testing
#[allow(dead_code)]
mod sexpr {
    use composite::abi::Value;

    pub fn sym(s: &str) -> Value {
        Value::Variant {
            tag: 0,
            payload: Some(Box::new(Value::String(s.to_string()))),
        }
    }

    pub fn num(n: i64) -> Value {
        Value::Variant {
            tag: 1,
            payload: Some(Box::new(Value::S64(n))),
        }
    }

    pub fn float(f: f64) -> Value {
        Value::Variant {
            tag: 2,
            payload: Some(Box::new(Value::F64(f))),
        }
    }

    pub fn boolean(b: bool) -> Value {
        Value::Variant {
            tag: 4,
            payload: Some(Box::new(Value::Bool(b))),
        }
    }

    pub fn nil() -> Value {
        Value::Variant { tag: 5, payload: None }
    }

    pub fn cons(head: Value, tail: Value) -> Value {
        Value::Variant {
            tag: 6,
            payload: Some(Box::new(Value::Tuple(vec![head, tail]))),
        }
    }

    pub fn list(items: Vec<Value>) -> Value {
        let mut result = nil();
        for item in items.into_iter().rev() {
            result = cons(item, result);
        }
        result
    }

    pub fn err(msg: &str) -> Value {
        Value::Variant {
            tag: 7,
            payload: Some(Box::new(Value::String(msg.to_string()))),
        }
    }
}

#[test]
fn sexpr_eval_simple_addition() {
    let wasm_bytes = load_rust_sexpr_component();

    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load sexpr component");
    let mut instance = module.instantiate().expect("failed to instantiate");

    // (+ 1 2 3) => 6
    let input = sexpr::list(vec![
        sexpr::sym("+"),
        sexpr::num(1),
        sexpr::num(2),
        sexpr::num(3),
    ]);

    let output = instance
        .call_with_value("evaluate", &input, 0)
        .expect("failed to call evaluate");

    assert_eq!(output, sexpr::num(6));
}

#[test]
fn sexpr_eval_nested_arithmetic() {
    let wasm_bytes = load_rust_sexpr_component();

    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load sexpr component");
    let mut instance = module.instantiate().expect("failed to instantiate");

    // (* (+ 2 3) (- 10 4)) => 5 * 6 = 30
    let input = sexpr::list(vec![
        sexpr::sym("*"),
        sexpr::list(vec![sexpr::sym("+"), sexpr::num(2), sexpr::num(3)]),
        sexpr::list(vec![sexpr::sym("-"), sexpr::num(10), sexpr::num(4)]),
    ]);

    let output = instance
        .call_with_value("evaluate", &input, 0)
        .expect("failed to call evaluate");

    assert_eq!(output, sexpr::num(30));
}

#[test]
fn sexpr_eval_comparison() {
    let wasm_bytes = load_rust_sexpr_component();

    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load sexpr component");
    let mut instance = module.instantiate().expect("failed to instantiate");

    // (< 5 10) => true
    let input = sexpr::list(vec![
        sexpr::sym("<"),
        sexpr::num(5),
        sexpr::num(10),
    ]);

    let output = instance
        .call_with_value("evaluate", &input, 0)
        .expect("failed to call evaluate");

    assert_eq!(output, sexpr::boolean(true));
}

#[test]
fn sexpr_eval_if_true() {
    let wasm_bytes = load_rust_sexpr_component();

    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load sexpr component");
    let mut instance = module.instantiate().expect("failed to instantiate");

    // (if (> 10 5) 42 0) => 42
    let input = sexpr::list(vec![
        sexpr::sym("if"),
        sexpr::list(vec![sexpr::sym(">"), sexpr::num(10), sexpr::num(5)]),
        sexpr::num(42),
        sexpr::num(0),
    ]);

    let output = instance
        .call_with_value("evaluate", &input, 0)
        .expect("failed to call evaluate");

    assert_eq!(output, sexpr::num(42));
}

#[test]
fn sexpr_eval_if_false() {
    let wasm_bytes = load_rust_sexpr_component();

    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load sexpr component");
    let mut instance = module.instantiate().expect("failed to instantiate");

    // (if (< 10 5) 42 0) => 0
    let input = sexpr::list(vec![
        sexpr::sym("if"),
        sexpr::list(vec![sexpr::sym("<"), sexpr::num(10), sexpr::num(5)]),
        sexpr::num(42),
        sexpr::num(0),
    ]);

    let output = instance
        .call_with_value("evaluate", &input, 0)
        .expect("failed to call evaluate");

    assert_eq!(output, sexpr::num(0));
}

#[test]
fn sexpr_eval_list_operations() {
    let wasm_bytes = load_rust_sexpr_component();

    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load sexpr component");
    let mut instance = module.instantiate().expect("failed to instantiate");

    // (car (list 1 2 3)) => 1
    let input = sexpr::list(vec![
        sexpr::sym("car"),
        sexpr::list(vec![
            sexpr::sym("list"),
            sexpr::num(1),
            sexpr::num(2),
            sexpr::num(3),
        ]),
    ]);

    let output = instance
        .call_with_value("evaluate", &input, 0)
        .expect("failed to call evaluate");

    assert_eq!(output, sexpr::num(1));
}

#[test]
fn sexpr_eval_length() {
    let wasm_bytes = load_rust_sexpr_component();

    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load sexpr component");
    let mut instance = module.instantiate().expect("failed to instantiate");

    // (length (list 1 2 3 4 5)) => 5
    let input = sexpr::list(vec![
        sexpr::sym("length"),
        sexpr::list(vec![
            sexpr::sym("list"),
            sexpr::num(1),
            sexpr::num(2),
            sexpr::num(3),
            sexpr::num(4),
            sexpr::num(5),
        ]),
    ]);

    let output = instance
        .call_with_value("evaluate", &input, 0)
        .expect("failed to call evaluate");

    assert_eq!(output, sexpr::num(5));
}

#[test]
fn sexpr_eval_complex_expression() {
    let wasm_bytes = load_rust_sexpr_component();

    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("failed to load sexpr component");
    let mut instance = module.instantiate().expect("failed to instantiate");

    // (if (and (> 10 5) (< 3 7))
    //     (+ (* 2 3) (/ 10 2))
    //     0)
    // => (6 + 5) = 11
    let input = sexpr::list(vec![
        sexpr::sym("if"),
        sexpr::list(vec![
            sexpr::sym("and"),
            sexpr::list(vec![sexpr::sym(">"), sexpr::num(10), sexpr::num(5)]),
            sexpr::list(vec![sexpr::sym("<"), sexpr::num(3), sexpr::num(7)]),
        ]),
        sexpr::list(vec![
            sexpr::sym("+"),
            sexpr::list(vec![sexpr::sym("*"), sexpr::num(2), sexpr::num(3)]),
            sexpr::list(vec![sexpr::sym("/"), sexpr::num(10), sexpr::num(2)]),
        ]),
        sexpr::num(0),
    ]);

    let output = instance
        .call_with_value("evaluate", &input, 0)
        .expect("failed to call evaluate");

    assert_eq!(output, sexpr::num(11));
}
