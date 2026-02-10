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

// From implementations for primitive types
impl From<bool> for Value {
    fn from(v: bool) -> Self {
        Value::Bool(v)
    }
}

impl From<u8> for Value {
    fn from(v: u8) -> Self {
        Value::U8(v)
    }
}

impl From<u16> for Value {
    fn from(v: u16) -> Self {
        Value::U16(v)
    }
}

impl From<u32> for Value {
    fn from(v: u32) -> Self {
        Value::U32(v)
    }
}

impl From<u64> for Value {
    fn from(v: u64) -> Self {
        Value::U64(v)
    }
}

impl From<i8> for Value {
    fn from(v: i8) -> Self {
        Value::S8(v)
    }
}

impl From<i16> for Value {
    fn from(v: i16) -> Self {
        Value::S16(v)
    }
}

impl From<i32> for Value {
    fn from(v: i32) -> Self {
        Value::S32(v)
    }
}

impl From<i64> for Value {
    fn from(v: i64) -> Self {
        Value::S64(v)
    }
}

impl From<f32> for Value {
    fn from(v: f32) -> Self {
        Value::F32(v)
    }
}

impl From<f64> for Value {
    fn from(v: f64) -> Self {
        Value::F64(v)
    }
}

impl From<char> for Value {
    fn from(v: char) -> Self {
        Value::Char(v)
    }
}

impl From<String> for Value {
    fn from(v: String) -> Self {
        Value::String(v)
    }
}

impl<T: PackType> From<Vec<T>> for Value {
    fn from(v: Vec<T>) -> Self {
        Value::List {
            elem_type: T::value_type(),
            items: v.into_iter().map(|x| x.into()).collect(),
        }
    }
}

impl<T: PackType> From<Option<T>> for Value {
    fn from(v: Option<T>) -> Self {
        Value::Option {
            inner_type: T::value_type(),
            value: v.map(|x| Box::new(x.into())),
        }
    }
}

/// Trait for types that can provide their ValueType at compile time.
///
/// This is used by `func_typed_result` and `func_async_result` to determine
/// the correct type tags for Result encoding, even when we only have a value
/// for one variant (Ok or Err).
pub trait PackType: Into<Value> {
    /// Returns the ValueType for this type.
    fn value_type() -> ValueType;
}

// Primitive type implementations
impl PackType for bool {
    fn value_type() -> ValueType {
        ValueType::Bool
    }
}

impl PackType for u8 {
    fn value_type() -> ValueType {
        ValueType::U8
    }
}

impl PackType for u16 {
    fn value_type() -> ValueType {
        ValueType::U16
    }
}

impl PackType for u32 {
    fn value_type() -> ValueType {
        ValueType::U32
    }
}

impl PackType for u64 {
    fn value_type() -> ValueType {
        ValueType::U64
    }
}

impl PackType for i8 {
    fn value_type() -> ValueType {
        ValueType::S8
    }
}

impl PackType for i16 {
    fn value_type() -> ValueType {
        ValueType::S16
    }
}

impl PackType for i32 {
    fn value_type() -> ValueType {
        ValueType::S32
    }
}

impl PackType for i64 {
    fn value_type() -> ValueType {
        ValueType::S64
    }
}

impl PackType for f32 {
    fn value_type() -> ValueType {
        ValueType::F32
    }
}

impl PackType for f64 {
    fn value_type() -> ValueType {
        ValueType::F64
    }
}

impl PackType for char {
    fn value_type() -> ValueType {
        ValueType::Char
    }
}

impl PackType for String {
    fn value_type() -> ValueType {
        ValueType::String
    }
}

// Vec<T> for list types
impl<T: PackType> PackType for Vec<T> {
    fn value_type() -> ValueType {
        ValueType::List(Box::new(T::value_type()))
    }
}

// Option<T> for option types
impl<T: PackType> PackType for Option<T> {
    fn value_type() -> ValueType {
        ValueType::Option(Box::new(T::value_type()))
    }
}

// Value itself - infers type at runtime (fallback for dynamic typing)
// Note: This defaults to String when we can't know the type statically.
// For accurate type encoding, use concrete types instead of Value.
impl PackType for Value {
    fn value_type() -> ValueType {
        // When using Value directly, we can't know the type statically.
        // Default to String as a fallback - this matches the previous behavior.
        ValueType::String
    }
}
