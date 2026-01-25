use composite::abi::{decode, encode, GraphBuffer, Node, NodeKind, Value, ValueType};

#[test]
fn roundtrip_primitives() {
    let values = vec![
        Value::Bool(true),
        Value::U8(250),
        Value::U16(65000),
        Value::U32(4_000_000_000),
        Value::U64(9_223_372_036_854_775_000),
        Value::S8(-5),
        Value::S16(-32000),
        Value::S32(-42),
        Value::S64(9_223_372_036_854_775_000),
        Value::F32(3.5),
        Value::F64(-1.25),
        Value::Char('z'),
        Value::Flags(0b1011),
        Value::String("hello".to_string()),
    ];

    for value in values {
        let bytes = encode(&value).expect("encode");
        let decoded = decode(&bytes).expect("decode");
        assert_eq!(decoded, value);
    }
}

#[test]
fn roundtrip_nested_values() {
    let value = Value::List {
        elem_type: ValueType::String,
        items: vec![
            Value::String("a".to_string()),
            Value::Tuple(vec![Value::S64(1), Value::S64(2)]),
            Value::Option {
                inner_type: ValueType::Bool,
                value: Some(Box::new(Value::Bool(false))),
            },
        ],
    };

    let bytes = encode(&value).expect("encode");
    let decoded = decode(&bytes).expect("decode");
    assert_eq!(decoded, value);
}

#[test]
fn roundtrip_variant() {
    let value = Value::Variant {
        type_name: "test".to_string(),
        case_name: "case2".to_string(),
        tag: 2,
        payload: vec![Value::String("payload".to_string())],
    };

    let bytes = encode(&value).expect("encode");
    let decoded = decode(&bytes).expect("decode");
    assert_eq!(decoded, value);
}

#[test]
fn graphbuffer_serialization_roundtrip() {
    let buffer = GraphBuffer {
        root: 0,
        nodes: vec![Node {
            kind: NodeKind::Bool,
            payload: vec![1],
        }],
    };

    let bytes = buffer.to_bytes();
    let decoded = GraphBuffer::from_bytes(&bytes).expect("from_bytes");
    decoded.validate_basic().expect("validate");
    assert_eq!(decoded.root, 0);
    assert_eq!(decoded.nodes.len(), 1);
    assert_eq!(decoded.nodes[0].kind, NodeKind::Bool);
    assert_eq!(decoded.nodes[0].payload, vec![1]);
}
