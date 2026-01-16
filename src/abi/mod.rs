//! ABI: Type Encoding and Decoding
//!
//! Handles marshaling data between host and WASM components.
//!
//! - All types use a graph-encoded ABI (schema-aware)

mod value;

pub use value::Value;

use std::collections::{HashMap, HashSet};

use thiserror::Error;

#[derive(Error, Debug)]
pub enum AbiError {
    #[error("Type mismatch: expected {expected}, got {got}")]
    TypeMismatch { expected: String, got: String },

    #[error("Invalid encoding: {0}")]
    InvalidEncoding(String),

    #[error("Buffer too small: need {need} bytes, have {have}")]
    BufferTooSmall { need: usize, have: usize },

    #[error("Invalid variant tag: {0}")]
    InvalidTag(u8),
}

const MAGIC: u32 = u32::from_le_bytes(*b"CGRF");
const VERSION: u16 = 1;

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_buffer_size: usize,
    pub max_node_count: usize,
    pub max_payload_size: usize,
    pub max_sequence_len: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_buffer_size: 16 * 1024 * 1024,
            max_node_count: 1_000_000,
            max_payload_size: 8 * 1024 * 1024,
            max_sequence_len: 1_000_000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    Bool = 0x01,
    S32 = 0x02,
    S64 = 0x03,
    F32 = 0x04,
    F64 = 0x05,
    String = 0x06,
    List = 0x07,
    Variant = 0x08,
    Record = 0x09,
    Option = 0x0A,
    Tuple = 0x0B,
    U8 = 0x0C,
    U16 = 0x0D,
    U32 = 0x0E,
    U64 = 0x0F,
    S8 = 0x10,
    S16 = 0x11,
    Char = 0x12,
}

#[derive(Debug, Clone)]
pub struct Node {
    pub kind: NodeKind,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct GraphBuffer {
    pub nodes: Vec<Node>,
    pub root: u32,
}

pub struct Encoder {
    nodes: Vec<Node>,
}

impl Encoder {
    pub fn new() -> Self {
        Self { nodes: Vec::new() }
    }

    pub fn push_node(&mut self, node: Node) -> u32 {
        let index = self.nodes.len() as u32;
        self.nodes.push(node);
        index
    }

    pub fn finish(self, root: u32) -> GraphBuffer {
        GraphBuffer {
            nodes: self.nodes,
            root,
        }
    }
}

pub struct Decoder<'a> {
    pub buffer: &'a GraphBuffer,
}

impl<'a> Decoder<'a> {
    pub fn new(buffer: &'a GraphBuffer) -> Self {
        Self { buffer }
    }

    pub fn node(&self, index: u32) -> Option<&'a Node> {
        self.buffer.nodes.get(index as usize)
    }
}

pub trait GraphCodec {
    fn encode_graph(&self, encoder: &mut Encoder) -> Result<u32, AbiError>;
    fn decode_graph(decoder: &Decoder<'_>, root: u32) -> Result<Self, AbiError>
    where
        Self: Sized;
}

/// Encode a value to bytes (graph-encoded ABI)
pub fn encode(value: &Value) -> Result<Vec<u8>, AbiError> {
    let mut encoder = Encoder::new();
    let root = value.encode_graph(&mut encoder)?;
    let buffer = encoder.finish(root);
    Ok(buffer.to_bytes())
}

/// Decode bytes to a value (graph-encoded ABI)
pub fn decode(bytes: &[u8]) -> Result<Value, AbiError> {
    let limits = Limits::default();
    let buffer = GraphBuffer::from_bytes_with_limits(bytes, &limits)?;
    buffer.validate_basic_with_limits(&limits)?;
    let decoder = Decoder::new(&buffer);
    Value::decode_graph(&decoder, buffer.root)
}

impl GraphBuffer {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&MAGIC.to_le_bytes());
        out.extend_from_slice(&VERSION.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&(self.nodes.len() as u32).to_le_bytes());
        out.extend_from_slice(&self.root.to_le_bytes());

        for node in &self.nodes {
            let kind = node.kind as u8;
            out.push(kind);
            out.push(0u8);
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&(node.payload.len() as u32).to_le_bytes());
            out.extend_from_slice(&node.payload);
        }

        out
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, AbiError> {
        let limits = Limits::default();
        Self::from_bytes_with_limits(bytes, &limits)
    }

    pub fn from_bytes_with_limits(bytes: &[u8], limits: &Limits) -> Result<Self, AbiError> {
        if bytes.len() > limits.max_buffer_size {
            return Err(AbiError::InvalidEncoding("Buffer too large".to_string()));
        }

        let mut cursor = Cursor::new(bytes);
        let magic = cursor.read_u32()?;
        if magic != MAGIC {
            return Err(AbiError::InvalidEncoding("Invalid magic".to_string()));
        }

        let version = cursor.read_u16()?;
        if version != VERSION {
            return Err(AbiError::InvalidEncoding("Unsupported version".to_string()));
        }

        let _flags = cursor.read_u16()?;
        let node_count = cursor.read_u32()? as usize;
        if node_count > limits.max_node_count {
            return Err(AbiError::InvalidEncoding("Node count exceeds limit".to_string()));
        }
        let root = cursor.read_u32()?;

        let mut nodes = Vec::with_capacity(node_count);
        for _ in 0..node_count {
            let kind = node_kind_from_u8(cursor.read_u8()?)?;
            let _node_flags = cursor.read_u8()?;
            let _reserved = cursor.read_u16()?;
            let payload_len = cursor.read_u32()? as usize;
            if payload_len > limits.max_payload_size {
                return Err(AbiError::InvalidEncoding("Payload too large".to_string()));
            }
            let payload = cursor.read_bytes(payload_len)?.to_vec();
            nodes.push(Node { kind, payload });
        }

        if (root as usize) >= nodes.len() {
            return Err(AbiError::InvalidEncoding("Root index out of range".to_string()));
        }

        if !cursor.is_eof() {
            return Err(AbiError::InvalidEncoding("Trailing bytes".to_string()));
        }

        Ok(Self { nodes, root })
    }

    pub fn validate_basic(&self) -> Result<(), AbiError> {
        let limits = Limits::default();
        self.validate_basic_with_limits(&limits)
    }

    pub fn validate_basic_with_limits(&self, limits: &Limits) -> Result<(), AbiError> {
        let node_count = self.nodes.len();
        if (self.root as usize) >= node_count {
            return Err(AbiError::InvalidEncoding("Root index out of range".to_string()));
        }
        if node_count > limits.max_node_count {
            return Err(AbiError::InvalidEncoding("Node count exceeds limit".to_string()));
        }

        for (index, node) in self.nodes.iter().enumerate() {
            if node.payload.len() > limits.max_payload_size {
                return Err(AbiError::InvalidEncoding(format!(
                    "Payload too large at node {index}"
                )));
            }
            let mut cursor = Cursor::new(&node.payload);
            match node.kind {
                NodeKind::Bool => {
                    let value = cursor.read_u8()?;
                    if value > 1 {
                        return Err(AbiError::InvalidEncoding(format!(
                            "Invalid bool payload at node {index}"
                        )));
                    }
                }
                NodeKind::S32 | NodeKind::F32 | NodeKind::U32 => {
                    cursor.read_bytes(4)?;
                }
                NodeKind::S64 | NodeKind::F64 | NodeKind::U64 => {
                    cursor.read_bytes(8)?;
                }
                NodeKind::U8 | NodeKind::S8 => {
                    cursor.read_bytes(1)?;
                }
                NodeKind::U16 | NodeKind::S16 => {
                    cursor.read_bytes(2)?;
                }
                NodeKind::Char => {
                    let value = cursor.read_u32()?;
                    let ch = char::from_u32(value).ok_or_else(|| {
                        AbiError::InvalidEncoding(format!(
                            "Invalid char scalar at node {index}"
                        ))
                    })?;
                    let _ = ch;
                }
                NodeKind::String => {
                    let len = cursor.read_u32()? as usize;
                    let bytes = cursor.read_bytes(len)?;
                    std::str::from_utf8(bytes).map_err(|_| {
                        AbiError::InvalidEncoding(format!(
                            "Invalid UTF-8 string at node {index}"
                        ))
                    })?;
                }
                NodeKind::List | NodeKind::Record | NodeKind::Tuple => {
                    let count = cursor.read_u32()? as usize;
                    if count > limits.max_sequence_len {
                        return Err(AbiError::InvalidEncoding(format!(
                            "Sequence too large at node {index}"
                        )));
                    }
                    for _ in 0..count {
                        let child = cursor.read_u32()? as usize;
                        if child >= node_count {
                            return Err(AbiError::InvalidEncoding(format!(
                                "Child index out of range at node {index}"
                            )));
                        }
                    }
                }
                NodeKind::Option => {
                    let has_value = cursor.read_u8()?;
                    if has_value > 1 {
                        return Err(AbiError::InvalidEncoding(format!(
                            "Invalid option flag at node {index}"
                        )));
                    }
                    if has_value == 1 {
                        let child = cursor.read_u32()? as usize;
                        if child >= node_count {
                            return Err(AbiError::InvalidEncoding(format!(
                                "Child index out of range at node {index}"
                            )));
                        }
                    }
                }
                NodeKind::Variant => {
                    cursor.read_u32()?;
                    let has_payload = cursor.read_u8()?;
                    if has_payload > 1 {
                        return Err(AbiError::InvalidEncoding(format!(
                            "Invalid variant payload flag at node {index}"
                        )));
                    }
                    if has_payload == 1 {
                        let child = cursor.read_u32()? as usize;
                        if child >= node_count {
                            return Err(AbiError::InvalidEncoding(format!(
                                "Child index out of range at node {index}"
                            )));
                        }
                    }
                }
            }

            if !cursor.is_eof() {
                return Err(AbiError::InvalidEncoding(format!(
                    "Trailing payload bytes at node {index}"
                )));
            }
        }

        Ok(())
    }
}

struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn is_eof(&self) -> bool {
        self.pos >= self.bytes.len()
    }

    fn read_bytes(&mut self, len: usize) -> Result<&'a [u8], AbiError> {
        if self.pos + len > self.bytes.len() {
            return Err(AbiError::BufferTooSmall {
                need: self.pos + len,
                have: self.bytes.len(),
            });
        }
        let start = self.pos;
        self.pos += len;
        Ok(&self.bytes[start..self.pos])
    }

    fn read_u8(&mut self) -> Result<u8, AbiError> {
        let bytes = self.read_bytes(1)?;
        Ok(bytes[0])
    }

    fn read_u16(&mut self) -> Result<u16, AbiError> {
        let bytes = self.read_bytes(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    fn read_u32(&mut self) -> Result<u32, AbiError> {
        let bytes = self.read_bytes(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn read_u64(&mut self) -> Result<u64, AbiError> {
        let bytes = self.read_bytes(8)?;
        Ok(u64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]))
    }
}

fn node_kind_from_u8(value: u8) -> Result<NodeKind, AbiError> {
    match value {
        0x01 => Ok(NodeKind::Bool),
        0x02 => Ok(NodeKind::S32),
        0x03 => Ok(NodeKind::S64),
        0x04 => Ok(NodeKind::F32),
        0x05 => Ok(NodeKind::F64),
        0x06 => Ok(NodeKind::String),
        0x07 => Ok(NodeKind::List),
        0x08 => Ok(NodeKind::Variant),
        0x09 => Ok(NodeKind::Record),
        0x0A => Ok(NodeKind::Option),
        0x0B => Ok(NodeKind::Tuple),
        0x0C => Ok(NodeKind::U8),
        0x0D => Ok(NodeKind::U16),
        0x0E => Ok(NodeKind::U32),
        0x0F => Ok(NodeKind::U64),
        0x10 => Ok(NodeKind::S8),
        0x11 => Ok(NodeKind::S16),
        0x12 => Ok(NodeKind::Char),
        _ => Err(AbiError::InvalidTag(value)),
    }
}

impl GraphCodec for Value {
    fn encode_graph(&self, encoder: &mut Encoder) -> Result<u32, AbiError> {
        match self {
            Value::Bool(value) => Ok(encoder.push_node(Node {
                kind: NodeKind::Bool,
                payload: vec![u8::from(*value)],
            })),
            Value::U8(value) => Ok(encoder.push_node(Node {
                kind: NodeKind::U8,
                payload: vec![*value],
            })),
            Value::U16(value) => Ok(encoder.push_node(Node {
                kind: NodeKind::U16,
                payload: value.to_le_bytes().to_vec(),
            })),
            Value::U32(value) => Ok(encoder.push_node(Node {
                kind: NodeKind::U32,
                payload: value.to_le_bytes().to_vec(),
            })),
            Value::U64(value) => Ok(encoder.push_node(Node {
                kind: NodeKind::U64,
                payload: value.to_le_bytes().to_vec(),
            })),
            Value::S8(value) => Ok(encoder.push_node(Node {
                kind: NodeKind::S8,
                payload: value.to_le_bytes().to_vec(),
            })),
            Value::S16(value) => Ok(encoder.push_node(Node {
                kind: NodeKind::S16,
                payload: value.to_le_bytes().to_vec(),
            })),
            Value::S32(value) => Ok(encoder.push_node(Node {
                kind: NodeKind::S32,
                payload: value.to_le_bytes().to_vec(),
            })),
            Value::S64(value) => Ok(encoder.push_node(Node {
                kind: NodeKind::S64,
                payload: value.to_le_bytes().to_vec(),
            })),
            Value::F32(value) => Ok(encoder.push_node(Node {
                kind: NodeKind::F32,
                payload: value.to_le_bytes().to_vec(),
            })),
            Value::F64(value) => Ok(encoder.push_node(Node {
                kind: NodeKind::F64,
                payload: value.to_le_bytes().to_vec(),
            })),
            Value::Char(value) => Ok(encoder.push_node(Node {
                kind: NodeKind::Char,
                payload: (*value as u32).to_le_bytes().to_vec(),
            })),
            Value::String(value) => {
                let bytes = value.as_bytes();
                let mut payload = Vec::with_capacity(4 + bytes.len());
                payload.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
                payload.extend_from_slice(bytes);
                Ok(encoder.push_node(Node {
                    kind: NodeKind::String,
                    payload,
                }))
            }
            Value::List(items) => {
                let mut child_indices = Vec::with_capacity(items.len());
                for item in items {
                    child_indices.push(item.encode_graph(encoder)?);
                }
                let mut payload = Vec::with_capacity(4 + 4 * child_indices.len());
                payload.extend_from_slice(&(child_indices.len() as u32).to_le_bytes());
                for child in child_indices {
                    payload.extend_from_slice(&child.to_le_bytes());
                }
                Ok(encoder.push_node(Node {
                    kind: NodeKind::List,
                    payload,
                }))
            }
            Value::Option(value) => {
                let mut payload = Vec::new();
                if let Some(inner) = value {
                    payload.push(1);
                    let child = inner.encode_graph(encoder)?;
                    payload.extend_from_slice(&child.to_le_bytes());
                } else {
                    payload.push(0);
                }
                Ok(encoder.push_node(Node {
                    kind: NodeKind::Option,
                    payload,
                }))
            }
            Value::Record(fields) => {
                let mut child_indices = Vec::with_capacity(fields.len());
                for (_, value) in fields {
                    child_indices.push(value.encode_graph(encoder)?);
                }
                let mut payload = Vec::with_capacity(4 + 4 * child_indices.len());
                payload.extend_from_slice(&(child_indices.len() as u32).to_le_bytes());
                for child in child_indices {
                    payload.extend_from_slice(&child.to_le_bytes());
                }
                Ok(encoder.push_node(Node {
                    kind: NodeKind::Record,
                    payload,
                }))
            }
            Value::Variant { tag, payload } => {
                let tag = u32::try_from(*tag).map_err(|_| AbiError::InvalidEncoding(
                    "Variant tag exceeds u32".to_string(),
                ))?;
                let mut payload_bytes = Vec::new();
                payload_bytes.extend_from_slice(&tag.to_le_bytes());
                if let Some(inner) = payload {
                    payload_bytes.push(1);
                    let child = inner.encode_graph(encoder)?;
                    payload_bytes.extend_from_slice(&child.to_le_bytes());
                } else {
                    payload_bytes.push(0);
                }
                Ok(encoder.push_node(Node {
                    kind: NodeKind::Variant,
                    payload: payload_bytes,
                }))
            }
        }
    }

    fn decode_graph(decoder: &Decoder<'_>, root: u32) -> Result<Self, AbiError> {
        let mut cache = HashMap::new();
        let mut visiting = HashSet::new();
        decode_value(decoder, root, &mut cache, &mut visiting)
    }
}

fn decode_value(
    decoder: &Decoder<'_>,
    index: u32,
    cache: &mut HashMap<u32, Value>,
    visiting: &mut HashSet<u32>,
) -> Result<Value, AbiError> {
    if let Some(value) = cache.get(&index) {
        return Ok(value.clone());
    }

    if !visiting.insert(index) {
        return Err(AbiError::InvalidEncoding(
            "Cycle detected in graph buffer".to_string(),
        ));
    }

    let node = decoder.node(index).ok_or_else(|| {
        AbiError::InvalidEncoding(format!("Node index {index} out of range"))
    })?;
    let mut cursor = Cursor::new(&node.payload);
    let value = match node.kind {
        NodeKind::Bool => Value::Bool(cursor.read_u8()? == 1),
        NodeKind::U8 => Value::U8(cursor.read_u8()?),
        NodeKind::U16 => {
            let raw = cursor.read_u16()?;
            Value::U16(raw)
        }
        NodeKind::U32 => Value::U32(cursor.read_u32()?),
        NodeKind::U64 => Value::U64(cursor.read_u64()?),
        NodeKind::S8 => {
            let raw = cursor.read_u8()?;
            Value::S8(i8::from_le_bytes([raw]))
        }
        NodeKind::S16 => {
            let raw = cursor.read_u16()?;
            Value::S16(i16::from_le_bytes(raw.to_le_bytes()))
        }
        NodeKind::S32 => {
            let raw = cursor.read_u32()?;
            Value::S32(i32::from_le_bytes(raw.to_le_bytes()))
        }
        NodeKind::S64 => {
            let raw = cursor.read_u64()?;
            Value::S64(i64::from_le_bytes(raw.to_le_bytes()))
        }
        NodeKind::F32 => {
            let raw = cursor.read_u32()?;
            Value::F32(f32::from_le_bytes(raw.to_le_bytes()))
        }
        NodeKind::F64 => {
            let raw = cursor.read_u64()?;
            Value::F64(f64::from_le_bytes(raw.to_le_bytes()))
        }
        NodeKind::Char => {
            let raw = cursor.read_u32()?;
            let ch = char::from_u32(raw)
                .ok_or_else(|| AbiError::InvalidEncoding("Invalid char scalar".to_string()))?;
            Value::Char(ch)
        }
        NodeKind::String => {
            let len = cursor.read_u32()? as usize;
            let bytes = cursor.read_bytes(len)?;
            let value = std::str::from_utf8(bytes)
                .map_err(|_| AbiError::InvalidEncoding("Invalid UTF-8".to_string()))?;
            Value::String(value.to_string())
        }
        NodeKind::List => {
            let count = cursor.read_u32()? as usize;
            let mut items = Vec::with_capacity(count);
            for _ in 0..count {
                let child = cursor.read_u32()?;
                items.push(decode_value(decoder, child, cache, visiting)?);
            }
            Value::List(items)
        }
        NodeKind::Record => {
            let count = cursor.read_u32()? as usize;
            let mut fields = Vec::with_capacity(count);
            for idx in 0..count {
                let child = cursor.read_u32()?;
                let value = decode_value(decoder, child, cache, visiting)?;
                fields.push((format!("field{idx}"), value));
            }
            Value::Record(fields)
        }
        NodeKind::Tuple => {
            let count = cursor.read_u32()? as usize;
            let mut items = Vec::with_capacity(count);
            for _ in 0..count {
                let child = cursor.read_u32()?;
                items.push(decode_value(decoder, child, cache, visiting)?);
            }
            Value::List(items)
        }
        NodeKind::Option => {
            let has_value = cursor.read_u8()?;
            let payload = if has_value == 1 {
                let child = cursor.read_u32()?;
                Some(Box::new(decode_value(decoder, child, cache, visiting)?))
            } else {
                None
            };
            Value::Option(payload)
        }
        NodeKind::Variant => {
            let tag = cursor.read_u32()? as usize;
            let has_payload = cursor.read_u8()?;
            let payload = if has_payload == 1 {
                let child = cursor.read_u32()?;
                Some(Box::new(decode_value(decoder, child, cache, visiting)?))
            } else {
                None
            };
            Value::Variant { tag, payload }
        }
    };

    if !cursor.is_eof() {
        return Err(AbiError::InvalidEncoding(format!(
            "Trailing payload bytes at node {index}"
        )));
    }

    visiting.remove(&index);
    cache.insert(index, value.clone());
    Ok(value)
}
