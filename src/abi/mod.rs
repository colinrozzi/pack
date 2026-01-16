//! ABI: Type Encoding and Decoding
//!
//! Handles marshaling data between host and WASM components.
//!
//! - Standard types use canonical ABI (fixed layout)
//! - Recursive types use serialization ABI (length-prefixed bytes)

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

/// Encode a value to bytes (for recursive types)
pub fn encode(_value: &Value) -> Vec<u8> {
    todo!("Serialization encoding")
}

/// Decode bytes to a value (for recursive types)
pub fn decode(_bytes: &[u8]) -> Result<Value, AbiError> {
    todo!("Serialization decoding")
}
