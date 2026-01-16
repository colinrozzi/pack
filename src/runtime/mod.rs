//! Component Runtime
//!
//! Handles component instantiation, linking, and execution.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum RuntimeError {
    #[error("Module not found: {0}")]
    ModuleNotFound(String),

    #[error("Function not found: {0}")]
    FunctionNotFound(String),

    #[error("Type mismatch: {0}")]
    TypeMismatch(String),

    #[error("WASM execution error: {0}")]
    WasmError(String),
}

/// The component runtime
pub struct Runtime {
    // TODO: Implementation
}

impl Runtime {
    pub fn new() -> Self {
        Self {}
    }
}

impl Default for Runtime {
    fn default() -> Self {
        Self::new()
    }
}
