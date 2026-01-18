//! WIT+ Parser and Type System
//!
//! Defines a WIT+ dialect with recursion allowed by default.

mod types;
mod parser;
mod validation;

pub use types::*;
pub use parser::{parse_interface, parse_world};
pub use validation::{
    decode_with_schema, encode_with_schema, validate_graph_against_type, ValidationError,
};

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
    pub imports: Vec<InterfaceImport>,
    pub exports: Vec<InterfaceExport>,
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
            imports: Vec::new(),
            exports: Vec::new(),
        }
    }

    pub fn add_type(&mut self, typedef: TypeDef) {
        self.types.push(typedef);
    }

    pub fn add_function(&mut self, func: Function) {
        self.functions.push(func);
    }

    pub fn add_import(&mut self, import: InterfaceImport) {
        self.imports.push(import);
    }

    pub fn add_export(&mut self, export: InterfaceExport) {
        self.exports.push(export);
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

        for import in &self.imports {
            for func in &import.functions {
                for (_, ty) in &func.params {
                    validate_type_ref(ty, &names, false)?;
                }
                for ty in &func.results {
                    validate_type_ref(ty, &names, false)?;
                }
            }
        }

        for export in &self.exports {
            for func in &export.functions {
                for (_, ty) in &func.params {
                    validate_type_ref(ty, &names, false)?;
                }
                for ty in &func.results {
                    validate_type_ref(ty, &names, false)?;
                }
            }
        }

        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct InterfaceImport {
    pub name: String,
    pub functions: Vec<Function>,
}

#[derive(Debug, Clone)]
pub struct InterfaceExport {
    pub name: String,
    pub functions: Vec<Function>,
}

// ============================================================================
// World Definitions
// ============================================================================

/// A parsed WIT+ world definition.
///
/// Worlds define the imports and exports for a component, specifying which
/// interfaces it requires (imports) and provides (exports).
///
/// # Example
///
/// ```wit
/// world my-component {
///     import wasi:cli/stdin
///     import wasi:cli/stdout
///     import log: func(msg: string)
///
///     export run: func() -> result<_, string>
/// }
/// ```
#[derive(Debug, Clone)]
pub struct World {
    pub name: String,
    pub imports: Vec<WorldItem>,
    pub exports: Vec<WorldItem>,
}

impl World {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            imports: Vec::new(),
            exports: Vec::new(),
        }
    }

    pub fn add_import(&mut self, item: WorldItem) {
        self.imports.push(item);
    }

    pub fn add_export(&mut self, item: WorldItem) {
        self.exports.push(item);
    }

    /// Validate the world definition.
    ///
    /// Currently performs basic validation. Interface references are not
    /// resolved (that would require access to other parsed interfaces).
    pub fn validate(&self) -> Result<(), ParseError> {
        // Validate standalone functions don't use SelfRef
        for item in self.imports.iter().chain(self.exports.iter()) {
            if let WorldItem::Function(func) = item {
                for (_, ty) in &func.params {
                    validate_type_ref_no_self(ty)?;
                }
                for ty in &func.results {
                    validate_type_ref_no_self(ty)?;
                }
            }
        }
        Ok(())
    }
}

/// An item in a world's import or export list.
#[derive(Debug, Clone)]
pub enum WorldItem {
    /// A reference to an interface by path (e.g., `wasi:cli/stdin`)
    InterfacePath(InterfacePath),

    /// A standalone function (e.g., `run: func() -> result<_, string>`)
    Function(Function),

    /// An inline interface definition with a name
    /// e.g., `export api { process: func(...) }`
    InlineInterface {
        name: String,
        functions: Vec<Function>,
    },
}

/// A path to an interface (e.g., `wasi:cli/stdin` or just `logging`)
#[derive(Debug, Clone, PartialEq)]
pub struct InterfacePath {
    /// Optional namespace (e.g., "wasi" in "wasi:cli/stdin")
    pub namespace: Option<String>,
    /// Optional package (e.g., "cli" in "wasi:cli/stdin")
    pub package: Option<String>,
    /// Interface name (e.g., "stdin" in "wasi:cli/stdin", or "logging" for simple refs)
    pub interface: String,
}

impl InterfacePath {
    /// Create a simple interface path (just the name)
    pub fn simple(name: impl Into<String>) -> Self {
        Self {
            namespace: None,
            package: None,
            interface: name.into(),
        }
    }

    /// Create a fully qualified interface path
    pub fn qualified(
        namespace: impl Into<String>,
        package: impl Into<String>,
        interface: impl Into<String>,
    ) -> Self {
        Self {
            namespace: Some(namespace.into()),
            package: Some(package.into()),
            interface: interface.into(),
        }
    }
}

impl std::fmt::Display for InterfacePath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (&self.namespace, &self.package) {
            (Some(ns), Some(pkg)) => write!(f, "{}:{}/{}", ns, pkg, self.interface),
            (None, Some(pkg)) => write!(f, "{}/{}", pkg, self.interface),
            _ => write!(f, "{}", self.interface),
        }
    }
}

/// Helper to validate that a type doesn't use SelfRef (for standalone functions)
fn validate_type_ref_no_self(ty: &Type) -> Result<(), ParseError> {
    match ty {
        Type::SelfRef => Err(ParseError::SelfRefOutsideType),
        Type::List(inner) | Type::Option(inner) => validate_type_ref_no_self(inner),
        Type::Result { ok, err } => {
            if let Some(inner) = ok {
                validate_type_ref_no_self(inner)?;
            }
            if let Some(inner) = err {
                validate_type_ref_no_self(inner)?;
            }
            Ok(())
        }
        Type::Tuple(items) => {
            for item in items {
                validate_type_ref_no_self(item)?;
            }
            Ok(())
        }
        _ => Ok(()),
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
