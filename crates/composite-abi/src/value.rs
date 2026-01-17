//! Runtime values

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

/// A runtime value that can be passed across component boundaries
#[derive(Debug, Clone, PartialEq)]
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
    Flags(u64),
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

// ============================================================================
// From implementations for primitives
// ============================================================================

impl From<bool> for Value {
    fn from(v: bool) -> Self { Value::Bool(v) }
}

impl From<u8> for Value {
    fn from(v: u8) -> Self { Value::U8(v) }
}

impl From<u16> for Value {
    fn from(v: u16) -> Self { Value::U16(v) }
}

impl From<u32> for Value {
    fn from(v: u32) -> Self { Value::U32(v) }
}

impl From<u64> for Value {
    fn from(v: u64) -> Self { Value::U64(v) }
}

impl From<i8> for Value {
    fn from(v: i8) -> Self { Value::S8(v) }
}

impl From<i16> for Value {
    fn from(v: i16) -> Self { Value::S16(v) }
}

impl From<i32> for Value {
    fn from(v: i32) -> Self { Value::S32(v) }
}

impl From<i64> for Value {
    fn from(v: i64) -> Self { Value::S64(v) }
}

impl From<f32> for Value {
    fn from(v: f32) -> Self { Value::F32(v) }
}

impl From<f64> for Value {
    fn from(v: f64) -> Self { Value::F64(v) }
}

impl From<char> for Value {
    fn from(v: char) -> Self { Value::Char(v) }
}

impl From<String> for Value {
    fn from(v: String) -> Self { Value::String(v) }
}

impl From<&str> for Value {
    fn from(v: &str) -> Self { Value::String(String::from(v)) }
}

impl<T: Into<Value>> From<Vec<T>> for Value {
    fn from(v: Vec<T>) -> Self {
        Value::List(v.into_iter().map(Into::into).collect())
    }
}

impl<T: Into<Value>> From<Option<T>> for Value {
    fn from(v: Option<T>) -> Self {
        Value::Option(v.map(|x| Box::new(x.into())))
    }
}

impl<T: Into<Value>> From<Box<T>> for Value {
    fn from(v: Box<T>) -> Self {
        (*v).into()
    }
}

// ============================================================================
// TryFrom implementations for primitives
// ============================================================================

use crate::ConversionError;

impl TryFrom<Value> for bool {
    type Error = ConversionError;
    fn try_from(v: Value) -> Result<Self, Self::Error> {
        match v {
            Value::Bool(x) => Ok(x),
            other => Err(ConversionError::TypeMismatch {
                expected: String::from("bool"),
                got: format!("{:?}", other),
            }),
        }
    }
}

impl TryFrom<Value> for u8 {
    type Error = ConversionError;
    fn try_from(v: Value) -> Result<Self, Self::Error> {
        match v {
            Value::U8(x) => Ok(x),
            other => Err(ConversionError::TypeMismatch {
                expected: String::from("u8"),
                got: format!("{:?}", other),
            }),
        }
    }
}

impl TryFrom<Value> for u16 {
    type Error = ConversionError;
    fn try_from(v: Value) -> Result<Self, Self::Error> {
        match v {
            Value::U16(x) => Ok(x),
            other => Err(ConversionError::TypeMismatch {
                expected: String::from("u16"),
                got: format!("{:?}", other),
            }),
        }
    }
}

impl TryFrom<Value> for u32 {
    type Error = ConversionError;
    fn try_from(v: Value) -> Result<Self, Self::Error> {
        match v {
            Value::U32(x) => Ok(x),
            other => Err(ConversionError::TypeMismatch {
                expected: String::from("u32"),
                got: format!("{:?}", other),
            }),
        }
    }
}

impl TryFrom<Value> for u64 {
    type Error = ConversionError;
    fn try_from(v: Value) -> Result<Self, Self::Error> {
        match v {
            Value::U64(x) => Ok(x),
            other => Err(ConversionError::TypeMismatch {
                expected: String::from("u64"),
                got: format!("{:?}", other),
            }),
        }
    }
}

impl TryFrom<Value> for i8 {
    type Error = ConversionError;
    fn try_from(v: Value) -> Result<Self, Self::Error> {
        match v {
            Value::S8(x) => Ok(x),
            other => Err(ConversionError::TypeMismatch {
                expected: String::from("i8"),
                got: format!("{:?}", other),
            }),
        }
    }
}

impl TryFrom<Value> for i16 {
    type Error = ConversionError;
    fn try_from(v: Value) -> Result<Self, Self::Error> {
        match v {
            Value::S16(x) => Ok(x),
            other => Err(ConversionError::TypeMismatch {
                expected: String::from("i16"),
                got: format!("{:?}", other),
            }),
        }
    }
}

impl TryFrom<Value> for i32 {
    type Error = ConversionError;
    fn try_from(v: Value) -> Result<Self, Self::Error> {
        match v {
            Value::S32(x) => Ok(x),
            other => Err(ConversionError::TypeMismatch {
                expected: String::from("i32"),
                got: format!("{:?}", other),
            }),
        }
    }
}

impl TryFrom<Value> for i64 {
    type Error = ConversionError;
    fn try_from(v: Value) -> Result<Self, Self::Error> {
        match v {
            Value::S64(x) => Ok(x),
            other => Err(ConversionError::TypeMismatch {
                expected: String::from("i64"),
                got: format!("{:?}", other),
            }),
        }
    }
}

impl TryFrom<Value> for f32 {
    type Error = ConversionError;
    fn try_from(v: Value) -> Result<Self, Self::Error> {
        match v {
            Value::F32(x) => Ok(x),
            other => Err(ConversionError::TypeMismatch {
                expected: String::from("f32"),
                got: format!("{:?}", other),
            }),
        }
    }
}

impl TryFrom<Value> for f64 {
    type Error = ConversionError;
    fn try_from(v: Value) -> Result<Self, Self::Error> {
        match v {
            Value::F64(x) => Ok(x),
            other => Err(ConversionError::TypeMismatch {
                expected: String::from("f64"),
                got: format!("{:?}", other),
            }),
        }
    }
}

impl TryFrom<Value> for char {
    type Error = ConversionError;
    fn try_from(v: Value) -> Result<Self, Self::Error> {
        match v {
            Value::Char(x) => Ok(x),
            other => Err(ConversionError::TypeMismatch {
                expected: String::from("char"),
                got: format!("{:?}", other),
            }),
        }
    }
}

impl TryFrom<Value> for String {
    type Error = ConversionError;
    fn try_from(v: Value) -> Result<Self, Self::Error> {
        match v {
            Value::String(x) => Ok(x),
            other => Err(ConversionError::TypeMismatch {
                expected: String::from("String"),
                got: format!("{:?}", other),
            }),
        }
    }
}

impl<T: TryFrom<Value, Error = ConversionError>> TryFrom<Value> for Vec<T> {
    type Error = ConversionError;
    fn try_from(v: Value) -> Result<Self, Self::Error> {
        match v {
            Value::List(items) => items
                .into_iter()
                .enumerate()
                .map(|(i, item)| {
                    T::try_from(item).map_err(|e| ConversionError::IndexError(i, Box::new(e)))
                })
                .collect(),
            other => Err(ConversionError::ExpectedList(format!("{:?}", other))),
        }
    }
}

use alloc::format;
