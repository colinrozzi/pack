//! Runtime values

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

/// A runtime value that can be passed across package boundaries
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

// ============================================================================
// FromValue trait - avoids coherence issues with TryFrom for Option<T>
// ============================================================================

/// Trait for converting from a Value.
///
/// This trait exists to avoid coherence issues with Rust's blanket
/// `impl<T, U> TryFrom<U> for T where U: Into<T>` when implementing
/// conversions for generic types like `Option<T>`.
pub trait FromValue: Sized {
    fn from_value(v: Value) -> Result<Self, ConversionError>;
}

/// Blanket implementation for all types that implement TryFrom<Value>
impl<T: TryFrom<Value, Error = ConversionError>> FromValue for T {
    fn from_value(v: Value) -> Result<Self, ConversionError> {
        T::try_from(v)
    }
}

/// FromValue implementation for Option<T> - uses FromValue bound to avoid coherence issues
impl<T: FromValue> FromValue for Option<T> {
    fn from_value(v: Value) -> Result<Self, ConversionError> {
        match v {
            Value::Option(None) => Ok(None),
            Value::Option(Some(inner)) => {
                let value = T::from_value(*inner)?;
                Ok(Some(value))
            }
            other => Err(ConversionError::ExpectedOption(format!("{:?}", other))),
        }
    }
}

// ============================================================================
// Tuple conversions (for common sizes)
// ============================================================================

// Empty tuple / unit
impl From<()> for Value {
    fn from(_: ()) -> Self {
        Value::Tuple(Vec::new())
    }
}

impl TryFrom<Value> for () {
    type Error = ConversionError;
    fn try_from(v: Value) -> Result<Self, Self::Error> {
        match v {
            Value::Tuple(items) if items.is_empty() => Ok(()),
            other => Err(ConversionError::ExpectedTuple(format!("{:?}", other))),
        }
    }
}

// 1-tuple
impl<A: Into<Value>> From<(A,)> for Value {
    fn from((a,): (A,)) -> Self {
        Value::Tuple(alloc::vec![a.into()])
    }
}

impl<A: TryFrom<Value, Error = ConversionError>> TryFrom<Value> for (A,) {
    type Error = ConversionError;
    fn try_from(v: Value) -> Result<Self, Self::Error> {
        match v {
            Value::Tuple(mut items) if items.len() == 1 => {
                let a = A::try_from(items.remove(0))
                    .map_err(|e| ConversionError::IndexError(0, Box::new(e)))?;
                Ok((a,))
            }
            other => Err(ConversionError::ExpectedTuple(format!("{:?}", other))),
        }
    }
}

// 2-tuple
impl<A: Into<Value>, B: Into<Value>> From<(A, B)> for Value {
    fn from((a, b): (A, B)) -> Self {
        Value::Tuple(alloc::vec![a.into(), b.into()])
    }
}

impl<A: TryFrom<Value, Error = ConversionError>, B: TryFrom<Value, Error = ConversionError>>
    TryFrom<Value> for (A, B)
{
    type Error = ConversionError;
    fn try_from(v: Value) -> Result<Self, Self::Error> {
        match v {
            Value::Tuple(mut items) if items.len() == 2 => {
                let b = B::try_from(items.remove(1))
                    .map_err(|e| ConversionError::IndexError(1, Box::new(e)))?;
                let a = A::try_from(items.remove(0))
                    .map_err(|e| ConversionError::IndexError(0, Box::new(e)))?;
                Ok((a, b))
            }
            other => Err(ConversionError::ExpectedTuple(format!("{:?}", other))),
        }
    }
}

// 3-tuple
impl<A: Into<Value>, B: Into<Value>, C: Into<Value>> From<(A, B, C)> for Value {
    fn from((a, b, c): (A, B, C)) -> Self {
        Value::Tuple(alloc::vec![a.into(), b.into(), c.into()])
    }
}

impl<
        A: TryFrom<Value, Error = ConversionError>,
        B: TryFrom<Value, Error = ConversionError>,
        C: TryFrom<Value, Error = ConversionError>,
    > TryFrom<Value> for (A, B, C)
{
    type Error = ConversionError;
    fn try_from(v: Value) -> Result<Self, Self::Error> {
        match v {
            Value::Tuple(mut items) if items.len() == 3 => {
                let c = C::try_from(items.remove(2))
                    .map_err(|e| ConversionError::IndexError(2, Box::new(e)))?;
                let b = B::try_from(items.remove(1))
                    .map_err(|e| ConversionError::IndexError(1, Box::new(e)))?;
                let a = A::try_from(items.remove(0))
                    .map_err(|e| ConversionError::IndexError(0, Box::new(e)))?;
                Ok((a, b, c))
            }
            other => Err(ConversionError::ExpectedTuple(format!("{:?}", other))),
        }
    }
}

// 4-tuple
impl<A: Into<Value>, B: Into<Value>, C: Into<Value>, D: Into<Value>> From<(A, B, C, D)> for Value {
    fn from((a, b, c, d): (A, B, C, D)) -> Self {
        Value::Tuple(alloc::vec![a.into(), b.into(), c.into(), d.into()])
    }
}

impl<
        A: TryFrom<Value, Error = ConversionError>,
        B: TryFrom<Value, Error = ConversionError>,
        C: TryFrom<Value, Error = ConversionError>,
        D: TryFrom<Value, Error = ConversionError>,
    > TryFrom<Value> for (A, B, C, D)
{
    type Error = ConversionError;
    fn try_from(v: Value) -> Result<Self, Self::Error> {
        match v {
            Value::Tuple(mut items) if items.len() == 4 => {
                let d = D::try_from(items.remove(3))
                    .map_err(|e| ConversionError::IndexError(3, Box::new(e)))?;
                let c = C::try_from(items.remove(2))
                    .map_err(|e| ConversionError::IndexError(2, Box::new(e)))?;
                let b = B::try_from(items.remove(1))
                    .map_err(|e| ConversionError::IndexError(1, Box::new(e)))?;
                let a = A::try_from(items.remove(0))
                    .map_err(|e| ConversionError::IndexError(0, Box::new(e)))?;
                Ok((a, b, c, d))
            }
            other => Err(ConversionError::ExpectedTuple(format!("{:?}", other))),
        }
    }
}

// ============================================================================
// Result conversions (as WIT result type - variant with tag 0=ok, 1=err)
// ============================================================================

impl<T: Into<Value>, E: Into<Value>> From<Result<T, E>> for Value {
    fn from(r: Result<T, E>) -> Self {
        match r {
            Ok(v) => Value::Variant {
                tag: 0,
                payload: Some(Box::new(v.into())),
            },
            Err(e) => Value::Variant {
                tag: 1,
                payload: Some(Box::new(e.into())),
            },
        }
    }
}

impl<T: TryFrom<Value, Error = ConversionError>, E: TryFrom<Value, Error = ConversionError>>
    TryFrom<Value> for Result<T, E>
{
    type Error = ConversionError;
    fn try_from(v: Value) -> Result<Self, Self::Error> {
        match v {
            Value::Variant { tag: 0, payload } => {
                let payload = payload.ok_or(ConversionError::MissingPayload)?;
                let value = T::try_from(*payload)
                    .map_err(|e| ConversionError::PayloadError(Box::new(e)))?;
                Ok(Ok(value))
            }
            Value::Variant { tag: 1, payload } => {
                let payload = payload.ok_or(ConversionError::MissingPayload)?;
                let value = E::try_from(*payload)
                    .map_err(|e| ConversionError::PayloadError(Box::new(e)))?;
                Ok(Err(value))
            }
            Value::Variant { tag, .. } => Err(ConversionError::UnknownTag { tag, max: 1 }),
            other => Err(ConversionError::ExpectedVariant(format!("{:?}", other))),
        }
    }
}

use alloc::format;
