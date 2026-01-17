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

/// An echo module that copies input bytes to output location.
/// Takes (in_ptr, in_len), copies to offset 4096, returns packed (out_ptr, out_len) as i64.
const ECHO_MODULE: &str = r#"
(module
    (memory (export "memory") 1)

    ;; Echo: copy input bytes to output location and return (out_ptr, out_len)
    ;; Returns i64 where low 32 bits = out_ptr, high 32 bits = out_len
    (func $echo (param $in_ptr i32) (param $in_len i32) (result i64)
        (local $out_ptr i32)
        (local $i i32)

        ;; Output starts at offset 4096
        (local.set $out_ptr (i32.const 4096))

        ;; Copy loop: memcpy(out_ptr, in_ptr, in_len)
        (local.set $i (i32.const 0))
        (block $break
            (loop $continue
                (br_if $break (i32.ge_u (local.get $i) (local.get $in_len)))

                (i32.store8
                    (i32.add (local.get $out_ptr) (local.get $i))
                    (i32.load8_u (i32.add (local.get $in_ptr) (local.get $i))))

                (local.set $i (i32.add (local.get $i) (i32.const 1)))
                (br $continue)
            )
        )

        ;; Return (out_len << 32) | out_ptr
        (i64.or
            (i64.extend_i32_u (local.get $out_ptr))
            (i64.shl
                (i64.extend_i32_u (local.get $in_len))
                (i64.const 32)))
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
