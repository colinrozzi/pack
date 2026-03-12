//! Pack type trait for compile-time type information

use super::{Value, ValueType};

/// Trait for types that can provide their ValueType at compile time.
///
/// This is used by `func_typed_result` and `func_async_result` to determine
/// the correct type tags for Result encoding, even when we only have a value
/// for one variant (Ok or Err).
pub trait PackType: Into<Value> {
    /// Returns the ValueType for this type.
    fn value_type() -> ValueType;
}

// Primitive type implementations for PackType
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
