//! Pack type trait for compile-time type information.
//!
//! `PackType` is the host-side trait used by `func_typed_result` and
//! similar wrappers to obtain a `ValueType` at compile time. It's a thin
//! wrapper over `KnownValueType` (which lives in `packr-abi` and is also
//! used by guests). Anything that's `Into<Value> + KnownValueType` is
//! automatically a `PackType`.

use super::{Value, ValueType};
pub use packr_abi::KnownValueType;

/// Trait for types that can provide their `ValueType` at compile time.
///
/// This is used by `func_typed_result` and `func_async_result` to
/// determine the correct type tags for `Result` encoding, even when we
/// only have a value for one variant (Ok or Err).
pub trait PackType: Into<Value> + KnownValueType {
    /// Returns the `ValueType` for this type. Defaults to
    /// `KnownValueType::known_value_type`.
    fn value_type() -> ValueType {
        <Self as KnownValueType>::known_value_type()
    }
}

impl<T: Into<Value> + KnownValueType> PackType for T {}
