//! ABI: Type Encoding and Decoding
//!
//! Handles marshaling data between host and WASM components.
//!
//! - All types use a graph-encoded ABI (schema-aware)

mod value;

pub use value::Value;

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
pub fn encode(_value: &Value) -> Vec<u8> {
    todo!("Serialization encoding")
}

/// Decode bytes to a value (graph-encoded ABI)
pub fn decode(_bytes: &[u8]) -> Result<Value, AbiError> {
    todo!("Serialization decoding")
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

    pub fn from_bytes(_bytes: &[u8]) -> Result<Self, AbiError> {
        let mut cursor = Cursor::new(_bytes);
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
        let root = cursor.read_u32()?;

        let mut nodes = Vec::with_capacity(node_count);
        for _ in 0..node_count {
            let kind = node_kind_from_u8(cursor.read_u8()?)?;
            let _node_flags = cursor.read_u8()?;
            let _reserved = cursor.read_u16()?;
            let payload_len = cursor.read_u32()? as usize;
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
        _ => Err(AbiError::InvalidTag(value)),
    }
}
