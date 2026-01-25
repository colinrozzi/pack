//! Runtime values

use serde::{Deserialize, Serialize};

/// Runtime type representation for CGRF v2
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ValueType {
    Bool,
    U8,
    U16,
    U32,
    U64,
    S8,
    S16,
    S32,
    S64,
    F32,
    F64,
    Char,
    String,
    List(Box<ValueType>),
    Option(Box<ValueType>),
    Result { ok: Box<ValueType>, err: Box<ValueType> },
    Record(String),   // type name
    Variant(String),  // type name
    Tuple(Vec<ValueType>),
    Flags,
}

/// A runtime value that can be passed across package boundaries
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

    // Compound types WITH type info
    List { elem_type: ValueType, items: Vec<Value> },
    Option { inner_type: ValueType, value: Option<Box<Value>> },
    Result { ok_type: ValueType, err_type: ValueType, value: std::result::Result<Box<Value>, Box<Value>> },
    Record { type_name: String, fields: Vec<(String, Value)> },
    Variant { type_name: String, case_name: String, tag: usize, payload: Vec<Value> },

    // Keep Tuple as-is (no type info needed - positional)
    Tuple(Vec<Value>),
    Flags(u64),
}

impl Value {
    /// Helper to create a symbol (variant tag 0 with string payload)
    pub fn sym(s: impl Into<String>) -> Self {
        Value::Variant {
            type_name: "expr".to_string(),
            case_name: "sym".to_string(),
            tag: 0,
            payload: vec![Value::String(s.into())],
        }
    }

    /// Helper to create a number (variant tag 1 with s64 payload)
    pub fn num(n: i64) -> Self {
        Value::Variant {
            type_name: "expr".to_string(),
            case_name: "num".to_string(),
            tag: 1,
            payload: vec![Value::S64(n)],
        }
    }

    /// Helper to create a list (variant tag 4 with list payload)
    pub fn lst(items: Vec<Value>) -> Self {
        Value::Variant {
            type_name: "expr".to_string(),
            case_name: "lst".to_string(),
            tag: 4,
            payload: vec![Value::List {
                elem_type: ValueType::Variant("expr".to_string()),
                items,
            }],
        }
    }

    /// Infer the ValueType from this Value
    pub fn infer_type(&self) -> ValueType {
        match self {
            Value::Bool(_) => ValueType::Bool,
            Value::U8(_) => ValueType::U8,
            Value::U16(_) => ValueType::U16,
            Value::U32(_) => ValueType::U32,
            Value::U64(_) => ValueType::U64,
            Value::S8(_) => ValueType::S8,
            Value::S16(_) => ValueType::S16,
            Value::S32(_) => ValueType::S32,
            Value::S64(_) => ValueType::S64,
            Value::F32(_) => ValueType::F32,
            Value::F64(_) => ValueType::F64,
            Value::Char(_) => ValueType::Char,
            Value::String(_) => ValueType::String,
            Value::List { elem_type, .. } => ValueType::List(Box::new(elem_type.clone())),
            Value::Option { inner_type, .. } => ValueType::Option(Box::new(inner_type.clone())),
            Value::Result { ok_type, err_type, .. } => ValueType::Result {
                ok: Box::new(ok_type.clone()),
                err: Box::new(err_type.clone()),
            },
            Value::Record { type_name, .. } => ValueType::Record(type_name.clone()),
            Value::Variant { type_name, .. } => ValueType::Variant(type_name.clone()),
            Value::Tuple(items) => ValueType::Tuple(items.iter().map(|v| v.infer_type()).collect()),
            Value::Flags(_) => ValueType::Flags,
        }
    }
}
