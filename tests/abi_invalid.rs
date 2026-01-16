use composite::abi::{decode, GraphBuffer, Limits, Node, NodeKind};

fn base_header() -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&u32::from_le_bytes(*b"CGRF").to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&0u16.to_le_bytes());
    bytes
}

#[test]
fn rejects_bad_magic() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&u32::from_le_bytes(*b"BAD!").to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&0u16.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());

    assert!(decode(&bytes).is_err());
}

#[test]
fn rejects_trailing_bytes() {
    let buffer = GraphBuffer {
        root: 0,
        nodes: vec![Node {
            kind: NodeKind::Bool,
            payload: vec![1],
        }],
    };

    let mut bytes = buffer.to_bytes();
    bytes.push(0xFF);

    assert!(decode(&bytes).is_err());
}

#[test]
fn rejects_child_out_of_range() {
    let mut bytes = base_header();
    bytes.extend_from_slice(&1u32.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.push(NodeKind::List as u8);
    bytes.push(0);
    bytes.extend_from_slice(&0u16.to_le_bytes());
    bytes.extend_from_slice(&8u32.to_le_bytes());
    bytes.extend_from_slice(&1u32.to_le_bytes());
    bytes.extend_from_slice(&5u32.to_le_bytes());

    assert!(decode(&bytes).is_err());
}

#[test]
fn rejects_invalid_utf8() {
    let mut bytes = base_header();
    bytes.extend_from_slice(&1u32.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.push(NodeKind::String as u8);
    bytes.push(0);
    bytes.extend_from_slice(&0u16.to_le_bytes());
    bytes.extend_from_slice(&5u32.to_le_bytes());
    bytes.extend_from_slice(&2u32.to_le_bytes());
    bytes.extend_from_slice(&[0xFF, 0xFF]);

    assert!(decode(&bytes).is_err());
}

#[test]
fn rejects_node_count_limit() {
    let mut bytes = base_header();
    bytes.extend_from_slice(&5u32.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());

    for _ in 0..5 {
        bytes.push(NodeKind::Bool as u8);
        bytes.push(0);
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.push(1);
    }

    let limits = Limits {
        max_node_count: 1,
        ..Limits::default()
    };

    assert!(GraphBuffer::from_bytes_with_limits(&bytes, &limits).is_err());
}
