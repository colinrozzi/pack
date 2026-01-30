//! Integration tests for embedded type metadata.

use pack::metadata::{decode_metadata, MetadataError, TypeDesc};
use pack::runtime::{CompositionBuilder, Runtime};
use pack::abi::{encode, Value, ValueType};

fn load_wasm(name: &str) -> Vec<u8> {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let path = std::path::Path::new(manifest_dir).join(format!(
        "packages/{}/target/wasm32-unknown-unknown/release/{}_package.wasm",
        name,
        name.replace('-', "_")
    ));
    std::fs::read(&path).unwrap_or_else(|e| panic!("Failed to read {}: {}", path.display(), e))
}

#[test]
fn test_echo_metadata() {
    let runtime = Runtime::new();
    let module = runtime.load_module(&load_wasm("echo")).expect("load echo");
    let mut instance = module.instantiate().expect("instantiate echo");

    let metadata = instance.types().expect("should have metadata");

    // Echo has no imports
    assert!(metadata.imports.is_empty(), "echo should have no imports");

    // Echo has 2 exports: echo and transform
    assert_eq!(metadata.exports.len(), 2, "echo should have 2 exports");

    let echo_fn = metadata.exports.iter().find(|f| f.name == "echo").expect("echo export");
    assert_eq!(echo_fn.params.len(), 1);
    assert_eq!(echo_fn.params[0].name, "input");
    assert_eq!(echo_fn.params[0].ty, TypeDesc::Value);
    assert_eq!(echo_fn.results.len(), 1);
    assert_eq!(echo_fn.results[0], TypeDesc::Value);

    let transform_fn = metadata.exports.iter().find(|f| f.name == "transform").expect("transform export");
    assert_eq!(transform_fn.params.len(), 1);
    assert_eq!(transform_fn.params[0].name, "input");
    assert_eq!(transform_fn.params[0].ty, TypeDesc::Value);
    assert_eq!(transform_fn.results.len(), 1);
    assert_eq!(transform_fn.results[0], TypeDesc::Value);
}

#[test]
fn test_doubler_metadata() {
    let runtime = Runtime::new();
    let module = runtime.load_module(&load_wasm("doubler")).expect("load doubler");
    let mut instance = module.instantiate().expect("instantiate doubler");

    let metadata = instance.types().expect("should have metadata");

    assert!(metadata.imports.is_empty());
    assert_eq!(metadata.exports.len(), 1);

    let double_fn = &metadata.exports[0];
    assert_eq!(double_fn.name, "double");
    assert_eq!(double_fn.params.len(), 1);
    assert_eq!(double_fn.params[0].name, "input");
    assert_eq!(double_fn.params[0].ty, TypeDesc::Value);
    assert_eq!(double_fn.results.len(), 1);
    assert_eq!(double_fn.results[0], TypeDesc::Value);
}

#[test]
fn test_adder_metadata_with_imports() {
    let doubler_wasm = load_wasm("doubler");
    let adder_wasm = load_wasm("adder");

    let mut composition = CompositionBuilder::new()
        .add_package("doubler", doubler_wasm)
        .add_package("adder", adder_wasm)
        .wire("adder", "math", "double", "doubler", "double")
        .build()
        .expect("build composition");

    let metadata = composition.types("adder").expect("adder metadata");

    // Adder imports from "math"
    assert_eq!(metadata.imports.len(), 1);
    let import_fn = &metadata.imports[0];
    assert_eq!(import_fn.interface, "math");
    assert_eq!(import_fn.name, "double");
    assert_eq!(import_fn.params.len(), 1);
    assert_eq!(import_fn.params[0].name, "n");
    assert_eq!(import_fn.params[0].ty, TypeDesc::S64);
    assert_eq!(import_fn.results.len(), 1);
    assert_eq!(import_fn.results[0], TypeDesc::S64);

    // Adder exports "process"
    assert_eq!(metadata.exports.len(), 1);
    let export_fn = &metadata.exports[0];
    assert_eq!(export_fn.name, "process");
    assert_eq!(export_fn.params[0].ty, TypeDesc::Value);
}

#[test]
fn test_no_metadata() {
    // A minimal WAT module without __pack_types
    let wat = r#"
        (module
            (memory (export "memory") 1)
            (func (export "noop") (param i32 i32 i32 i32) (result i32)
                i32.const 0
            )
        )
    "#;
    let wasm = wat::parse_str(wat).expect("parse wat");

    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm).expect("load module");
    let mut instance = module.instantiate().expect("instantiate");

    let result = instance.types();
    assert!(result.is_err(), "should error without __pack_types");

    match result.unwrap_err() {
        MetadataError::NotFound => {} // expected
        other => panic!("expected NotFound, got: {:?}", other),
    }
}

#[test]
fn test_metadata_roundtrip() {
    // Manually construct a metadata Value, encode it, then decode
    let metadata_value = Value::Record {
        type_name: "package-metadata".into(),
        fields: vec![
            (
                "imports".into(),
                Value::List {
                    elem_type: ValueType::Record("".into()),
                    items: vec![],
                },
            ),
            (
                "exports".into(),
                Value::List {
                    elem_type: ValueType::Record("".into()),
                    items: vec![Value::Record {
                        type_name: "function-sig".into(),
                        fields: vec![
                            ("interface".into(), Value::String("".into())),
                            ("name".into(), Value::String("my_func".into())),
                            (
                                "params".into(),
                                Value::List {
                                    elem_type: ValueType::Record("".into()),
                                    items: vec![Value::Record {
                                        type_name: "param-sig".into(),
                                        fields: vec![
                                            ("name".into(), Value::String("x".into())),
                                            (
                                                "type".into(),
                                                Value::Variant {
                                                    type_name: "type-desc".into(),
                                                    case_name: "s64".into(),
                                                    tag: 8,
                                                    payload: vec![],
                                                },
                                            ),
                                        ],
                                    }],
                                },
                            ),
                            (
                                "results".into(),
                                Value::List {
                                    elem_type: ValueType::Variant("".into()),
                                    items: vec![Value::Variant {
                                        type_name: "type-desc".into(),
                                        case_name: "string".into(),
                                        tag: 12,
                                        payload: vec![],
                                    }],
                                },
                            ),
                        ],
                    }],
                },
            ),
        ],
    };

    let bytes = encode(&metadata_value).expect("encode");
    let decoded = decode_metadata(&bytes).expect("decode");

    assert_eq!(decoded.imports.len(), 0);
    assert_eq!(decoded.exports.len(), 1);
    assert_eq!(decoded.exports[0].name, "my_func");
    assert_eq!(decoded.exports[0].params[0].name, "x");
    assert_eq!(decoded.exports[0].params[0].ty, TypeDesc::S64);
    assert_eq!(decoded.exports[0].results[0], TypeDesc::String);
}

#[test]
fn test_metadata_with_list_type() {
    // Test a function with list<s64> parameter type
    let metadata_value = Value::Record {
        type_name: "package-metadata".into(),
        fields: vec![
            (
                "imports".into(),
                Value::List {
                    elem_type: ValueType::Record("".into()),
                    items: vec![],
                },
            ),
            (
                "exports".into(),
                Value::List {
                    elem_type: ValueType::Record("".into()),
                    items: vec![Value::Record {
                        type_name: "function-sig".into(),
                        fields: vec![
                            ("interface".into(), Value::String("".into())),
                            ("name".into(), Value::String("sum".into())),
                            (
                                "params".into(),
                                Value::List {
                                    elem_type: ValueType::Record("".into()),
                                    items: vec![Value::Record {
                                        type_name: "param-sig".into(),
                                        fields: vec![
                                            ("name".into(), Value::String("numbers".into())),
                                            (
                                                "type".into(),
                                                Value::Variant {
                                                    type_name: "type-desc".into(),
                                                    case_name: "list".into(),
                                                    tag: 14,
                                                    payload: vec![Value::Variant {
                                                        type_name: "type-desc".into(),
                                                        case_name: "s64".into(),
                                                        tag: 8,
                                                        payload: vec![],
                                                    }],
                                                },
                                            ),
                                        ],
                                    }],
                                },
                            ),
                            (
                                "results".into(),
                                Value::List {
                                    elem_type: ValueType::Variant("".into()),
                                    items: vec![Value::Variant {
                                        type_name: "type-desc".into(),
                                        case_name: "s64".into(),
                                        tag: 8,
                                        payload: vec![],
                                    }],
                                },
                            ),
                        ],
                    }],
                },
            ),
        ],
    };

    let bytes = encode(&metadata_value).expect("encode");
    let decoded = decode_metadata(&bytes).expect("decode");

    assert_eq!(decoded.exports[0].name, "sum");
    assert_eq!(
        decoded.exports[0].params[0].ty,
        TypeDesc::List(Box::new(TypeDesc::S64))
    );
    assert_eq!(decoded.exports[0].results[0], TypeDesc::S64);
}
