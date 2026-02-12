//! WIT+ Parser
//!
//! Parses a WIT+ dialect with recursion allowed by default.
//! The module focuses on parsing; types are defined in `crate::types`.

mod wit;
mod validation;

pub use wit::{parse_interface, parse_world};
pub use validation::{
    decode_with_schema, encode_with_schema, validate_graph_against_type, ValidationError,
};

// Re-export types from crate::types for convenience
pub use crate::types::{
    Arena, Case, Field, Function, Param, Type, TypeDef, TypePath,
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

// ============================================================================
// World Definitions (kept here as they're parser-specific)
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
                for param in &func.params {
                    validate_type_ref_no_self(&param.ty)?;
                }
                for ty in &func.results {
                    validate_type_ref_no_self(ty)?;
                }
            }
        }
        Ok(())
    }

    /// Convert this World to an Arena representation.
    pub fn to_arena(&self) -> Arena {
        let mut arena = Arena::new(&self.name);

        // Create child arenas for imports and exports
        let mut import_arena = Arena::new("imports");
        let mut export_arena = Arena::new("exports");

        for item in &self.imports {
            match item {
                WorldItem::InterfacePath(path) => {
                    let child = Arena::new(path.to_string());
                    import_arena.add_child(child);
                }
                WorldItem::Function(func) => {
                    // Standalone functions go in a "standalone" interface
                    let standalone = import_arena.children.iter_mut()
                        .find(|c| c.name == "standalone");
                    if let Some(child) = standalone {
                        child.add_function(func.clone());
                    } else {
                        let mut child = Arena::new("standalone");
                        child.add_function(func.clone());
                        import_arena.add_child(child);
                    }
                }
                WorldItem::InlineInterface { name, functions } => {
                    let mut child = Arena::new(name.clone());
                    for func in functions {
                        child.add_function(func.clone());
                    }
                    import_arena.add_child(child);
                }
            }
        }

        for item in &self.exports {
            match item {
                WorldItem::InterfacePath(path) => {
                    let child = Arena::new(path.to_string());
                    export_arena.add_child(child);
                }
                WorldItem::Function(func) => {
                    let standalone = export_arena.children.iter_mut()
                        .find(|c| c.name == "standalone");
                    if let Some(child) = standalone {
                        child.add_function(func.clone());
                    } else {
                        let mut child = Arena::new("standalone");
                        child.add_function(func.clone());
                        export_arena.add_child(child);
                    }
                }
                WorldItem::InlineInterface { name, functions } => {
                    let mut child = Arena::new(name.clone());
                    for func in functions {
                        child.add_function(func.clone());
                    }
                    export_arena.add_child(child);
                }
            }
        }

        if !import_arena.children.is_empty() {
            arena.add_child(import_arena);
        }
        if !export_arena.children.is_empty() {
            arena.add_child(export_arena);
        }

        arena
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

// ============================================================================
// Legacy Interface type (for backward compatibility)
// ============================================================================

/// A parsed WIT+ interface (legacy type).
///
/// This type is kept for backward compatibility during migration.
/// New code should use `Arena` directly.
#[derive(Debug, Clone)]
pub struct Interface {
    pub name: String,
    pub types: Vec<TypeDef>,
    pub functions: Vec<Function>,
    pub imports: Vec<InterfaceImport>,
    pub exports: Vec<InterfaceExport>,
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
                TypeDef::Alias { ty, .. } => {
                    validate_type_ref(ty, &names, true)?;
                }
                TypeDef::Record { fields, .. } => {
                    for field in fields {
                        validate_type_ref(&field.ty, &names, true)?;
                    }
                }
                TypeDef::Variant { cases, .. } => {
                    for case in cases {
                        if !case.payload.is_unit() {
                            validate_type_ref(&case.payload, &names, true)?;
                        }
                    }
                }
                TypeDef::Enum { .. } | TypeDef::Flags { .. } => {}
            }
        }

        for func in &self.functions {
            for param in &func.params {
                validate_type_ref(&param.ty, &names, false)?;
            }
            for ty in &func.results {
                validate_type_ref(ty, &names, false)?;
            }
        }

        for import in &self.imports {
            for func in &import.functions {
                for param in &func.params {
                    validate_type_ref(&param.ty, &names, false)?;
                }
                for ty in &func.results {
                    validate_type_ref(ty, &names, false)?;
                }
            }
        }

        for export in &self.exports {
            for func in &export.functions {
                for param in &func.params {
                    validate_type_ref(&param.ty, &names, false)?;
                }
                for ty in &func.results {
                    validate_type_ref(ty, &names, false)?;
                }
            }
        }

        Ok(())
    }

    /// Convert this Interface to an Arena representation.
    pub fn to_arena(&self) -> Arena {
        let mut arena = Arena::new(&self.name);

        for typedef in &self.types {
            arena.add_type(typedef.clone());
        }

        for func in &self.functions {
            arena.add_function(func.clone());
        }

        // Add imports as child arenas
        for import in &self.imports {
            let mut child = Arena::new(&import.name);
            for func in &import.functions {
                child.add_function(func.clone());
            }
            arena.add_child(child);
        }

        // Add exports as child arenas
        for export in &self.exports {
            let mut child = Arena::new(&export.name);
            for func in &export.functions {
                child.add_function(func.clone());
            }
            arena.add_child(child);
        }

        arena
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
// Validation Helpers
// ============================================================================

/// Helper to validate that a type doesn't use SelfRef (for standalone functions)
fn validate_type_ref_no_self(ty: &Type) -> Result<(), ParseError> {
    match ty {
        Type::Ref(path) if path.is_self_ref() => Err(ParseError::SelfRefOutsideType),
        Type::List(inner) | Type::Option(inner) => validate_type_ref_no_self(inner),
        Type::Result { ok, err } => {
            validate_type_ref_no_self(ok)?;
            validate_type_ref_no_self(err)?;
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
            validate_type_ref(ok, names, allow_self_ref)?;
            validate_type_ref(err, names, allow_self_ref)?;
            Ok(())
        }
        Type::Tuple(items) => {
            for item in items {
                validate_type_ref(item, names, allow_self_ref)?;
            }
            Ok(())
        }
        Type::Ref(path) => {
            if path.is_self_ref() {
                if allow_self_ref {
                    Ok(())
                } else {
                    Err(ParseError::SelfRefOutsideType)
                }
            } else if let Some(name) = path.as_simple() {
                if names.contains(name) {
                    Ok(())
                } else {
                    Err(ParseError::UndefinedType(name.to_string()))
                }
            } else {
                // Qualified paths are assumed valid (external references)
                Ok(())
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

        interface.add_type(TypeDef::variant(
            "expr",
            vec![Case::new("literal", Type::named("lit"))],
        ));

        interface.add_type(TypeDef::variant(
            "lit",
            vec![Case::new("quoted", Type::named("expr"))],
        ));

        assert!(interface.validate().is_ok());
    }

    #[test]
    fn validate_rejects_unknown_type() {
        let mut interface = Interface::new("bad");

        interface.add_type(TypeDef::record(
            "config",
            vec![Field::new("value", Type::named("missing"))],
        ));

        assert!(matches!(
            interface.validate(),
            Err(ParseError::UndefinedType(name)) if name == "missing"
        ));
    }

    #[test]
    fn validate_rejects_self_ref_outside_type() {
        let mut interface = Interface::new("bad");

        interface.add_function(Function::with_signature(
            "f",
            vec![Param::new("x", Type::self_ref())],
            Vec::new(),
        ));

        assert!(matches!(
            interface.validate(),
            Err(ParseError::SelfRefOutsideType)
        ));
    }

    #[test]
    fn interface_to_arena_conversion() {
        let mut interface = Interface::new("test");
        interface.add_type(TypeDef::alias("count", Type::U32));
        interface.add_function(Function::with_signature(
            "add",
            vec![Param::new("a", Type::S32), Param::new("b", Type::S32)],
            vec![Type::S32],
        ));

        let arena = interface.to_arena();
        assert_eq!(arena.name, "test");
        assert_eq!(arena.types.len(), 1);
        assert_eq!(arena.functions.len(), 1);
    }
}
