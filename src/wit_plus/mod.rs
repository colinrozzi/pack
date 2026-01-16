//! WIT+ Parser and Type System
//!
//! Defines a WIT+ dialect with recursion allowed by default.

mod types;
mod parser;
mod validation;

pub use types::*;
pub use parser::parse_interface;
pub use validation::{validate_graph_against_type, ValidationError};

use std::collections::HashSet;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum ParseError {
    #[error("Unexpected token: {0}")]
    UnexpectedToken(String),
    #[error("Unexpected end of input")]
    UnexpectedEof,
    #[error("Invalid type reference: {0}")]
    InvalidTypeRef(String),
    #[error("Undefined type: {0}")]
    UndefinedType(String),
    #[error("Self reference used outside of a type definition")]
    SelfRefOutsideType,
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

    pub fn validate(&self) -> Result<(), ParseError> {
        let names: HashSet<&str> = self.types.iter().map(|def| def.name()).collect();

        for def in &self.types {
            match def {
                TypeDef::Alias(_, ty) => {
                    validate_type_ref(ty, &names, true)?;
                }
                TypeDef::Record(record) => {
                    for (_, ty) in &record.fields {
                        validate_type_ref(ty, &names, true)?;
                    }
                }
                TypeDef::Variant(variant) => {
                    for case in &variant.cases {
                        if let Some(ty) = &case.payload {
                            validate_type_ref(ty, &names, true)?;
                        }
                    }
                }
                TypeDef::Enum(_) | TypeDef::Flags(_) => {}
            }
        }

        for func in &self.functions {
            for (_, ty) in &func.params {
                validate_type_ref(ty, &names, false)?;
            }
            for ty in &func.results {
                validate_type_ref(ty, &names, false)?;
            }
        }

        Ok(())
    }
}

fn validate_type_ref(
    ty: &Type,
    names: &HashSet<&str>,
    allow_self_ref: bool,
) -> Result<(), ParseError> {
    match ty {
        Type::List(inner) | Type::Option(inner) => {
            validate_type_ref(inner, names, allow_self_ref)
        }
        Type::Result { ok, err } => {
            if let Some(inner) = ok {
                validate_type_ref(inner, names, allow_self_ref)?;
            }
            if let Some(inner) = err {
                validate_type_ref(inner, names, allow_self_ref)?;
            }
            Ok(())
        }
        Type::Tuple(items) => {
            for item in items {
                validate_type_ref(item, names, allow_self_ref)?;
            }
            Ok(())
        }
        Type::Named(name) => {
            if names.contains(name.as_str()) {
                Ok(())
            } else {
                Err(ParseError::UndefinedType(name.clone()))
            }
        }
        Type::SelfRef => {
            if allow_self_ref {
                Ok(())
            } else {
                Err(ParseError::SelfRefOutsideType)
            }
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_allows_mutual_recursion() {
        let mut interface = Interface::new("math");

        interface.add_type(TypeDef::Variant(VariantDef {
            name: "expr".to_string(),
            cases: vec![VariantCase {
                name: "literal".to_string(),
                payload: Some(Type::Named("lit".to_string())),
            }],
        }));

        interface.add_type(TypeDef::Variant(VariantDef {
            name: "lit".to_string(),
            cases: vec![VariantCase {
                name: "quoted".to_string(),
                payload: Some(Type::Named("expr".to_string())),
            }],
        }));

        assert!(interface.validate().is_ok());
    }

    #[test]
    fn validate_rejects_unknown_type() {
        let mut interface = Interface::new("bad");

        interface.add_type(TypeDef::Record(RecordDef {
            name: "config".to_string(),
            fields: vec![("value".to_string(), Type::Named("missing".to_string()))],
        }));

        assert!(matches!(
            interface.validate(),
            Err(ParseError::UndefinedType(name)) if name == "missing"
        ));
    }

    #[test]
    fn validate_rejects_self_ref_outside_type() {
        let mut interface = Interface::new("bad");

        interface.add_function(Function {
            name: "f".to_string(),
            params: vec![("x".to_string(), Type::SelfRef)],
            results: Vec::new(),
        });

        assert!(matches!(
            interface.validate(),
            Err(ParseError::SelfRefOutsideType)
        ));
    }
}
