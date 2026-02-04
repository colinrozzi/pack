//! Error types for static composition.

use thiserror::Error;

/// Errors that can occur during static composition.
#[derive(Debug, Error)]
pub enum ComposeError {
    /// Failed to parse a WASM module.
    #[error("parse error in module '{module}': {message}")]
    ParseError { module: String, message: String },

    /// A module with the given name was not found.
    #[error("module not found: {0}")]
    ModuleNotFound(String),

    /// A function was not found in a module.
    #[error("function '{function}' not found in module '{module}'")]
    FunctionNotFound { module: String, function: String },

    /// An import could not be resolved.
    #[error("unresolved import in '{consumer}': {import_module}::{import_fn}")]
    UnresolvedImport {
        consumer: String,
        import_module: String,
        import_fn: String,
    },

    /// Type mismatch when wiring imports to exports.
    #[error("type mismatch wiring {consumer}::{import_fn} to {provider}::{export_fn}: {message}")]
    TypeMismatch {
        consumer: String,
        import_fn: String,
        provider: String,
        export_fn: String,
        message: String,
    },

    /// Multiple modules export the same internal function (e.g., __pack_alloc).
    #[error("duplicate internal function '{function}' in modules: {modules:?}")]
    DuplicateInternal {
        function: String,
        modules: Vec<String>,
    },

    /// Circular dependency detected between modules.
    #[error("circular dependency detected: {cycle:?}")]
    CircularDependency { cycle: Vec<String> },

    /// Failed to encode the merged WASM module.
    #[error("encoding error: {0}")]
    EncodingError(String),

    /// Memory merge error.
    #[error("memory error: {0}")]
    MemoryError(String),

    /// No modules were added to the composer.
    #[error("no modules added")]
    NoModules,

    /// Invalid WASM module.
    #[error("invalid WASM module '{module}': {message}")]
    InvalidModule { module: String, message: String },
}
