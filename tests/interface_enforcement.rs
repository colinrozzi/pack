//! Interface enforcement tests
//!
//! Tests that validate WASM modules implement WIT interfaces correctly.

use pack::runtime::InterfaceError;
use pack::wit_plus::parse_interface;
use pack::Runtime;

/// A module with the correct Graph ABI signature: (i32, i32) -> i64
const VALID_PROCESS_MODULE: &str = r#"
(module
    (memory (export "memory") 1)

    ;; process: takes (ptr, len), returns packed (out_ptr, out_len)
    (func $process (param $in_ptr i32) (param $in_len i32) (result i64)
        ;; Just return the same pointer/length (echo behavior)
        (i64.or
            (i64.extend_i32_u (local.get $in_ptr))
            (i64.shl
                (i64.extend_i32_u (local.get $in_len))
                (i64.const 32)))
    )
    (export "process" (func $process))
)
"#;

/// A module with wrong signature: (i32, i32) -> i32 instead of i64
const WRONG_SIGNATURE_MODULE: &str = r#"
(module
    (memory (export "memory") 1)

    (func $process (param $in_ptr i32) (param $in_len i32) (result i32)
        (local.get $in_ptr)
    )
    (export "process" (func $process))
)
"#;

/// A module missing the required function
const MISSING_FUNCTION_MODULE: &str = r#"
(module
    (memory (export "memory") 1)

    (func $other (param $x i32) (result i32)
        (local.get $x)
    )
    (export "other" (func $other))
)
"#;

/// A module missing memory export
const NO_MEMORY_MODULE: &str = r#"
(module
    (func $process (param $in_ptr i32) (param $in_len i32) (result i64)
        (i64.const 0)
    )
    (export "process" (func $process))
)
"#;

/// A module with multiple functions
const MULTI_FUNCTION_MODULE: &str = r#"
(module
    (memory (export "memory") 1)

    (func $parse (param $in_ptr i32) (param $in_len i32) (result i64)
        (i64.or
            (i64.extend_i32_u (local.get $in_ptr))
            (i64.shl (i64.extend_i32_u (local.get $in_len)) (i64.const 32)))
    )

    (func $eval (param $in_ptr i32) (param $in_len i32) (result i64)
        (i64.or
            (i64.extend_i32_u (local.get $in_ptr))
            (i64.shl (i64.extend_i32_u (local.get $in_len)) (i64.const 32)))
    )

    (export "parse" (func $parse))
    (export "eval" (func $eval))
)
"#;

#[test]
fn valid_module_passes_validation() {
    let interface = parse_interface(
        r#"
        interface api {
            process: func(input: string) -> string;
        }
    "#,
    )
    .expect("parse interface");

    let wasm_bytes = wat::parse_str(VALID_PROCESS_MODULE).expect("parse WAT");
    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("load module");
    let mut instance = module.instantiate().expect("instantiate");

    // Should pass validation
    let result = instance.validate_interface(&interface);
    assert!(result.is_ok(), "Expected Ok, got {:?}", result);
}

#[test]
fn wrong_signature_fails_validation() {
    let interface = parse_interface(
        r#"
        interface api {
            process: func(input: string) -> string;
        }
    "#,
    )
    .expect("parse interface");

    let wasm_bytes = wat::parse_str(WRONG_SIGNATURE_MODULE).expect("parse WAT");
    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("load module");
    let mut instance = module.instantiate().expect("instantiate");

    let result = instance.validate_interface(&interface);
    assert!(matches!(
        result,
        Err(InterfaceError::SignatureMismatch { name, .. }) if name == "process"
    ));
}

#[test]
fn missing_function_fails_validation() {
    let interface = parse_interface(
        r#"
        interface api {
            process: func(input: string) -> string;
        }
    "#,
    )
    .expect("parse interface");

    let wasm_bytes = wat::parse_str(MISSING_FUNCTION_MODULE).expect("parse WAT");
    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("load module");
    let mut instance = module.instantiate().expect("instantiate");

    let result = instance.validate_interface(&interface);
    assert!(matches!(
        result,
        Err(InterfaceError::MissingFunction { name }) if name == "process"
    ));
}

#[test]
fn missing_memory_fails_validation() {
    let interface = parse_interface(
        r#"
        interface api {
            process: func(input: string) -> string;
        }
    "#,
    )
    .expect("parse interface");

    let wasm_bytes = wat::parse_str(NO_MEMORY_MODULE).expect("parse WAT");
    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("load module");
    let mut instance = module.instantiate().expect("instantiate");

    let result = instance.validate_interface(&interface);
    assert!(matches!(result, Err(InterfaceError::MissingMemory)));
}

#[test]
fn multi_function_interface_validation() {
    let interface = parse_interface(
        r#"
        interface lisp {
            variant sexpr {
                sym(string),
                num(s64),
                lst(list<sexpr>),
            }

            parse: func(input: string) -> result<sexpr, string>;
            eval: func(expr: sexpr) -> result<sexpr, string>;
        }
    "#,
    )
    .expect("parse interface");

    let wasm_bytes = wat::parse_str(MULTI_FUNCTION_MODULE).expect("parse WAT");
    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("load module");
    let mut instance = module.instantiate().expect("instantiate");

    let result = instance.validate_interface(&interface);
    assert!(result.is_ok(), "Expected Ok, got {:?}", result);
}

#[test]
fn partial_implementation_fails_validation() {
    let interface = parse_interface(
        r#"
        interface lisp {
            parse: func(input: string) -> string;
            eval: func(expr: string) -> string;
            compile: func(expr: string) -> string;
        }
    "#,
    )
    .expect("parse interface");

    // Module only has parse and eval, missing compile
    let wasm_bytes = wat::parse_str(MULTI_FUNCTION_MODULE).expect("parse WAT");
    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("load module");
    let mut instance = module.instantiate().expect("instantiate");

    let result = instance.validate_interface(&interface);
    assert!(matches!(
        result,
        Err(InterfaceError::MissingFunction { name }) if name == "compile"
    ));
}

#[test]
fn export_block_functions_validated() {
    let interface = parse_interface(
        r#"
        interface api {
            export api {
                process: func(input: string) -> string;
            }
        }
    "#,
    )
    .expect("parse interface");

    let wasm_bytes = wat::parse_str(VALID_PROCESS_MODULE).expect("parse WAT");
    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("load module");
    let mut instance = module.instantiate().expect("instantiate");

    let result = instance.validate_interface(&interface);
    assert!(result.is_ok(), "Expected Ok, got {:?}", result);
}

#[test]
fn export_block_missing_function_fails() {
    let interface = parse_interface(
        r#"
        interface api {
            export api {
                process: func(input: string) -> string;
                transform: func(input: string) -> string;
            }
        }
    "#,
    )
    .expect("parse interface");

    let wasm_bytes = wat::parse_str(VALID_PROCESS_MODULE).expect("parse WAT");
    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("load module");
    let mut instance = module.instantiate().expect("instantiate");

    let result = instance.validate_interface(&interface);
    assert!(matches!(
        result,
        Err(InterfaceError::MissingFunction { name }) if name == "transform"
    ));
}

#[test]
fn empty_interface_always_passes() {
    let interface = parse_interface(
        r#"
        interface empty {
        }
    "#,
    )
    .expect("parse interface");

    let wasm_bytes = wat::parse_str(VALID_PROCESS_MODULE).expect("parse WAT");
    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes).expect("load module");
    let mut instance = module.instantiate().expect("instantiate");

    let result = instance.validate_interface(&interface);
    assert!(result.is_ok(), "Expected Ok, got {:?}", result);
}
