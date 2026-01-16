//! WIT+ Parser and Type System
//!
//! Defines a WIT+ dialect with recursion allowed by default.

mod types;

pub use types::*;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum ParseError {
    #[error("Unexpected token: {0}")]
    UnexpectedToken(String),
    #[error("Unexpected end of input")]
    UnexpectedEof,
    #[error("Invalid type reference: {0}")]
    InvalidTypeRef(String),
}

/// A parsed WIT+ interface
#[derive(Debug, Clone)]
pub struct Interface {
    pub name: String,
    pub types: Vec<TypeDef>,
    pub functions: Vec<Function>,
}

/// A function signature
#[derive(Debug, Clone)]
pub struct Function {
    pub name: String,
    pub params: Vec<(String, Type)>,
    pub results: Vec<Type>,
}

impl Interface {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            types: Vec::new(),
            functions: Vec::new(),
        }
    }

    pub fn add_type(&mut self, typedef: TypeDef) {
        self.types.push(typedef);
    }

    pub fn add_function(&mut self, func: Function) {
        self.functions.push(func);
    }
}
