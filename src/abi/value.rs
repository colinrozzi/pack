//! Runtime values

use serde::{Deserialize, Serialize};

/// A runtime value that can be passed across component boundaries
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Value {
    // Primitives
    Bool(bool),
    U8(u8),
    U16(u16),
    U32(u32),
    U64(u64),
    S8(i8),
    S16(i16),
    S32(i32),
    S64(i64),
    F32(f32),
    F64(f64),
    Char(char),
    String(String),

    // Compound
    List(Vec<Value>),
    Option(Option<Box<Value>>),
    Tuple(Vec<Value>),
    Record(Vec<(String, Value)>),
    Variant { tag: usize, payload: Option<Box<Value>> },
}

impl Value {
    /// Helper to create a symbol (variant tag 0 with string payload)
    pub fn sym(s: impl Into<String>) -> Self {
        Value::Variant {
            tag: 0,
            payload: Some(Box::new(Value::String(s.into()))),
        }
    }

    /// Helper to create a number (variant tag 1 with s64 payload)
    pub fn num(n: i64) -> Self {
        Value::Variant {
            tag: 1,
            payload: Some(Box::new(Value::S64(n))),
        }
    }

    /// Helper to create a list (variant tag 4 with list payload)
    pub fn lst(items: Vec<Value>) -> Self {
        Value::Variant {
            tag: 4,
            payload: Some(Box::new(Value::List(items))),
        }
    }
}
