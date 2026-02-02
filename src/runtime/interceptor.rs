//! Call Interceptor
//!
//! Provides a trait for intercepting import (host function) and export (WASM function)
//! calls at the Pack runtime level. This enables automatic recording and replay of
//! all calls without handlers needing manual recording code.
//!
//! # Recording
//!
//! A recording interceptor returns `None` from `before_import`/`before_export`
//! (allowing normal execution) and records the input/output in `after_import`/`after_export`.
//!
//! # Replay
//!
//! A replay interceptor returns `Some(recorded_output)` from `before_import`/`before_export`,
//! short-circuiting the actual call and returning the previously recorded value.

use crate::abi::Value;

/// Trait for intercepting calls at the Pack runtime level.
///
/// Implementations can record calls (for audit/replay) or short-circuit
/// them with previously recorded values (for replay).
///
/// All methods are synchronous since recording (push to list) and replay
/// (read from list) are simple lookups that don't need async.
pub trait CallInterceptor: Send + Sync {
    /// Called before a host function (import) executes.
    ///
    /// Return `Some(Value)` to short-circuit with a recorded value (replay).
    /// Return `None` to proceed with normal execution (recording/passthrough).
    fn before_import(&self, interface: &str, function: &str, input: &Value) -> Option<Value>;

    /// Called after a host function (import) returns normally.
    fn after_import(&self, interface: &str, function: &str, input: &Value, output: &Value);

    /// Called before an export function is called.
    ///
    /// Return `Some(Value)` to skip the actual WASM call (replay).
    /// Return `None` to proceed normally.
    fn before_export(&self, function: &str, input: &Value) -> Option<Value>;

    /// Called after an export function returns.
    fn after_export(&self, function: &str, input: &Value, output: &Value);
}
