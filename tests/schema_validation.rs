use composite::abi::{encode, GraphBuffer, Node, NodeKind, Value, ValueType};
use composite::wit_plus::{
    decode_with_schema, encode_with_schema, parse_interface, validate_graph_against_type, Type,
    ValidationError,
};

#[test]
fn validate_graph_against_schema() {
    let src = r#"
        interface api {
            variant node { leaf(s64), list(list<node>) }
        }
    "#;
    let interface = parse_interface(src).expect("parse");

    let leaf = Value::Variant {
        type_name: "node".to_string(),
        case_name: "leaf".to_string(),
        tag: 0,
        payload: vec![Value::S64(7)],
    };
    let list = Value::Variant {
        type_name: "node".to_string(),
        case_name: "list".to_string(),
        tag: 1,
        payload: vec![Value::List {
            elem_type: ValueType::Variant("node".to_string()),
            items: vec![leaf.clone()],
        }],
    };

    let bytes = encode(&list).expect("encode");
    let buffer = GraphBuffer::from_bytes(&bytes).expect("from_bytes");

    validate_graph_against_type(&interface.types, &buffer, &Type::Named("node".to_string()))
        .expect("schema validate");
}

#[test]
fn reject_variant_tag_out_of_range() {
    let src = r#"
        interface api {
            variant node { leaf(s64), list(list<node>) }
        }
    "#;
    let interface = parse_interface(src).expect("parse");

    let mut bytes = Vec::new();
    bytes.extend_from_slice(&u32::from_le_bytes(*b"CGRF").to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes()); // VERSION 2
    bytes.extend_from_slice(&0u16.to_le_bytes());
    bytes.extend_from_slice(&1u32.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.push(0x08);
    bytes.push(0);
    bytes.extend_from_slice(&0u16.to_le_bytes());
    // v2 variant payload: type_name_len + type_name + case_name_len + case_name + tag + payload_count
    let type_name = "node";
    let case_name = "bad";
    let payload_len = 4 + type_name.len() + 4 + case_name.len() + 4 + 4;
    bytes.extend_from_slice(&(payload_len as u32).to_le_bytes());
    bytes.extend_from_slice(&(type_name.len() as u32).to_le_bytes());
    bytes.extend_from_slice(type_name.as_bytes());
    bytes.extend_from_slice(&(case_name.len() as u32).to_le_bytes());
    bytes.extend_from_slice(case_name.as_bytes());
    bytes.extend_from_slice(&99u32.to_le_bytes()); // bad tag
    bytes.extend_from_slice(&0u32.to_le_bytes()); // no payload

    let buffer = GraphBuffer::from_bytes(&bytes).expect("from_bytes");
    let err = validate_graph_against_type(&interface.types, &buffer, &Type::Named("node".to_string()))
        .expect_err("expected validation error");

    match err {
        ValidationError::VariantTagOutOfRange { .. } => {}
        _ => panic!("unexpected error: {err:?}"),
    }
}

#[test]
fn reject_tuple_arity_mismatch() {
    let buffer = GraphBuffer {
        root: 0,
        nodes: vec![
            Node {
                kind: NodeKind::Tuple,
                payload: {
                    let mut payload = Vec::new();
                    payload.extend_from_slice(&1u32.to_le_bytes());
                    payload.extend_from_slice(&1u32.to_le_bytes());
                    payload
                },
            },
            Node {
                kind: NodeKind::S32,
                payload: 1i32.to_le_bytes().to_vec(),
            },
        ],
    };

    let err = validate_graph_against_type(&[], &buffer, &Type::Tuple(vec![Type::S32, Type::S32]))
        .expect_err("expected validation error");

    match err {
        ValidationError::TypeMismatch { .. } => {}
        _ => panic!("unexpected error: {err:?}"),
    }
}

#[test]
fn reject_wrong_node_kind() {
    let buffer = GraphBuffer {
        root: 0,
        nodes: vec![Node {
            kind: NodeKind::Bool,
            payload: vec![1],
        }],
    };

    let err = validate_graph_against_type(&[], &buffer, &Type::String)
        .expect_err("expected validation error");

    match err {
        ValidationError::TypeMismatch { .. } => {}
        _ => panic!("unexpected error: {err:?}"),
    }
}

#[test]
fn decode_with_schema_roundtrip() {
    let src = r#"
        interface api {
            variant node { leaf(s64), list(list<node>) }
        }
    "#;
    let interface = parse_interface(src).expect("parse");

    let value = Value::Variant {
        type_name: "node".to_string(),
        case_name: "leaf".to_string(),
        tag: 0,
        payload: vec![Value::S64(42)],
    };
    let bytes = encode(&value).expect("encode");
    let decoded =
        decode_with_schema(&interface.types, &bytes, &Type::Named("node".to_string()), None)
            .expect("decode");

    assert_eq!(decoded, value);
}

#[test]
fn decode_with_schema_rejects_mismatch() {
    let src = r#"
        interface api {
            variant node { leaf(s64), list(list<node>) }
        }
    "#;
    let interface = parse_interface(src).expect("parse");

    let value = Value::String("bad".to_string());
    let bytes = encode(&value).expect("encode");

    let err = decode_with_schema(
        &interface.types,
        &bytes,
        &Type::Named("node".to_string()),
        None,
    )
    .expect_err("expected validation error");

    match err {
        ValidationError::TypeMismatch { .. } => {}
        _ => panic!("unexpected error: {err:?}"),
    }
}

#[test]
fn validate_flags_mask() {
    let src = r#"
        interface api {
            flags mode { read, write, exec }
        }
    "#;
    let interface = parse_interface(src).expect("parse");

    let value = Value::Flags(0b101);
    let bytes = encode(&value).expect("encode");
    let buffer = GraphBuffer::from_bytes(&bytes).expect("from_bytes");

    validate_graph_against_type(
        &interface.types,
        &buffer,
        &Type::Named("mode".to_string()),
    )
    .expect("schema validate");
}

#[test]
fn encode_with_schema_rejects_record_field_order() {
    let src = r#"
        interface api {
            record config { name: string, enabled: bool }
        }
    "#;
    let interface = parse_interface(src).expect("parse");

    let value = Value::Record {
        type_name: "config".to_string(),
        fields: vec![
            ("enabled".to_string(), Value::Bool(true)),
            ("name".to_string(), Value::String("x".to_string())),
        ],
    };

    let err = encode_with_schema(&interface.types, &value, &Type::Named("config".to_string()))
        .expect_err("expected error");

    match err {
        ValidationError::TypeMismatch { .. } => {}
        _ => panic!("unexpected error: {err:?}"),
    }
}
