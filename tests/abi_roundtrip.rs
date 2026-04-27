use pack::abi::{decode, encode, GraphBuffer, Node, NodeKind, Value, ValueType};

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

#[test]
fn roundtrip_array_u8() {
    // A List<U8> should use the Array encoding (single node)
    let byte_data: Vec<u8> = (0..=255).collect();
    let value = Value::List {
        elem_type: ValueType::U8,
        items: byte_data.iter().map(|&b| Value::U8(b)).collect(),
    };

    let bytes = encode(&value).expect("encode");
    let decoded = decode(&bytes).expect("decode");
    assert_eq!(decoded, value);

    // Verify: single Array node, not 256+ individual nodes
    let buffer = GraphBuffer::from_bytes(&bytes).expect("from_bytes");
    assert_eq!(buffer.nodes.len(), 1, "Array should use a single node");
    assert_eq!(buffer.nodes[0].kind, NodeKind::Array);
}

#[test]
fn roundtrip_array_s32() {
    // List<S32> should also use compact Array encoding
    let value = Value::List {
        elem_type: ValueType::S32,
        items: vec![Value::S32(-1), Value::S32(0), Value::S32(i32::MAX as i32)],
    };

    let bytes = encode(&value).expect("encode");
    let decoded = decode(&bytes).expect("decode");
    assert_eq!(decoded, value);

    let buffer = GraphBuffer::from_bytes(&bytes).expect("from_bytes");
    assert_eq!(buffer.nodes.len(), 1);
    assert_eq!(buffer.nodes[0].kind, NodeKind::Array);
}

#[test]
fn roundtrip_array_f64() {
    let value = Value::List {
        elem_type: ValueType::F64,
        items: vec![
            Value::F64(3.14),
            Value::F64(-0.0),
            Value::F64(f64::INFINITY),
        ],
    };

    let bytes = encode(&value).expect("encode");
    let decoded = decode(&bytes).expect("decode");
    assert_eq!(decoded, value);

    let buffer = GraphBuffer::from_bytes(&bytes).expect("from_bytes");
    assert_eq!(buffer.nodes.len(), 1);
    assert_eq!(buffer.nodes[0].kind, NodeKind::Array);
}

#[test]
fn roundtrip_array_bool() {
    let value = Value::List {
        elem_type: ValueType::Bool,
        items: vec![Value::Bool(true), Value::Bool(false), Value::Bool(true)],
    };

    let bytes = encode(&value).expect("encode");
    let decoded = decode(&bytes).expect("decode");
    assert_eq!(decoded, value);

    let buffer = GraphBuffer::from_bytes(&bytes).expect("from_bytes");
    assert_eq!(buffer.nodes.len(), 1);
    assert_eq!(buffer.nodes[0].kind, NodeKind::Array);
}

#[test]
fn roundtrip_large_byte_array() {
    // The original bug: large byte payloads that blew past max_buffer_size
    let size = 100_000;
    let byte_data: Vec<Value> = (0..size).map(|i| Value::U8((i % 256) as u8)).collect();
    let value = Value::List {
        elem_type: ValueType::U8,
        items: byte_data,
    };

    let bytes = encode(&value).expect("encode");
    // 100KB of data should encode to ~100KB, not megabytes
    assert!(
        bytes.len() < 200_000,
        "Array encoding should be near 1:1, got {}",
        bytes.len()
    );

    let decoded = decode(&bytes).expect("decode");
    assert_eq!(decoded, value);
}

#[test]
fn roundtrip_compound_list_uses_list_node() {
    // List of strings (compound type) should still use indirect List encoding
    let value = Value::List {
        elem_type: ValueType::String,
        items: vec![
            Value::String("a".to_string()),
            Value::String("b".to_string()),
        ],
    };

    let bytes = encode(&value).expect("encode");
    let decoded = decode(&bytes).expect("decode");
    assert_eq!(decoded, value);

    let buffer = GraphBuffer::from_bytes(&bytes).expect("from_bytes");
    assert_eq!(buffer.nodes[buffer.root as usize].kind, NodeKind::List);
}
