//! Unified Type System
//!
//! This module provides a unified type representation used across Pack:
//! - Design-time (parsing, validation)
//! - Runtime metadata (embedded in WASM packages)
//! - ABI encoding/decoding
//!
//! Key design decisions:
//! - **Arena** as unified scoping structure
//! - **Lexical scoping** with qualified paths for cross-arena references
//! - **Nominal typing** - names are part of identity
//! - **`Unit` is explicit** - no more optional ok/err in Result
//! - **Everything derives `Hash`** - enables hash-based comparison
//! - **`Value` kept** as dynamic escape hatch

use serde::{Deserialize, Serialize};
use std::hash::{Hash, Hasher};

// ============================================================================
// Arena - Core Scoping Structure
// ============================================================================

/// An arena containing type definitions, functions, and child arenas.
///
/// Arenas replace the Package/Interface split with a unified scoping structure.
/// They can be nested to represent hierarchical namespaces.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Arena {
    /// Name of this arena (e.g., "math", "wasi:cli/stdout")
    pub name: String,
    /// Type definitions in this arena
    pub types: Vec<TypeDef>,
    /// Functions in this arena
    pub functions: Vec<Function>,
    /// Child arenas (for hierarchical namespaces)
    pub children: Vec<Arena>,
}

impl Arena {
    /// Create a new empty arena with the given name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            types: Vec::new(),
            functions: Vec::new(),
            children: Vec::new(),
        }
    }

    /// Add a type definition to this arena.
    pub fn add_type(&mut self, typedef: TypeDef) {
        self.types.push(typedef);
    }

    /// Add a function to this arena.
    pub fn add_function(&mut self, func: Function) {
        self.functions.push(func);
    }

    /// Add a child arena.
    pub fn add_child(&mut self, child: Arena) {
        self.children.push(child);
    }

    /// Find a type definition by name in this arena.
    pub fn find_type(&self, name: &str) -> Option<&TypeDef> {
        self.types.iter().find(|t| t.name() == name)
    }

    /// Find a function by name in this arena.
    pub fn find_function(&self, name: &str) -> Option<&Function> {
        self.functions.iter().find(|f| f.name == name)
    }
}

// ============================================================================
// Function - Function Signatures
// ============================================================================

/// A function signature with parameters and results.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Function {
    /// Function name
    pub name: String,
    /// Local type definitions (scoped to this function)
    pub types: Vec<TypeDef>,
    /// Function parameters
    pub params: Vec<Param>,
    /// Return types
    pub results: Vec<Type>,
}

impl Function {
    /// Create a new function with the given name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            types: Vec::new(),
            params: Vec::new(),
            results: Vec::new(),
        }
    }

    /// Create a function with parameters and results.
    pub fn with_signature(
        name: impl Into<String>,
        params: Vec<Param>,
        results: Vec<Type>,
    ) -> Self {
        Self {
            name: name.into(),
            types: Vec::new(),
            params,
            results,
        }
    }
}

/// A function parameter.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Param {
    /// Parameter name
    pub name: String,
    /// Parameter type
    pub ty: Type,
}

impl Param {
    /// Create a new parameter.
    pub fn new(name: impl Into<String>, ty: Type) -> Self {
        Self {
            name: name.into(),
            ty,
        }
    }
}

// ============================================================================
// TypeDef - Type Definitions
// ============================================================================

/// A type definition (named type).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TypeDef {
    /// Type alias: `type foo = bar`
    Alias { name: String, ty: Type },

    /// Record type: `record foo { field: type, ... }`
    Record { name: String, fields: Vec<Field> },

    /// Variant type: `variant foo { case(payload), ... }`
    Variant { name: String, cases: Vec<Case> },

    /// Enum type: `enum foo { case1, case2, ... }`
    Enum { name: String, cases: Vec<String> },

    /// Flags type: `flags foo { flag1, flag2, ... }`
    Flags { name: String, flags: Vec<String> },
}

impl TypeDef {
    /// Get the name of this type definition.
    pub fn name(&self) -> &str {
        match self {
            TypeDef::Alias { name, .. } => name,
            TypeDef::Record { name, .. } => name,
            TypeDef::Variant { name, .. } => name,
            TypeDef::Enum { name, .. } => name,
            TypeDef::Flags { name, .. } => name,
        }
    }

    /// Create an alias type definition.
    pub fn alias(name: impl Into<String>, ty: Type) -> Self {
        TypeDef::Alias {
            name: name.into(),
            ty,
        }
    }

    /// Create a record type definition.
    pub fn record(name: impl Into<String>, fields: Vec<Field>) -> Self {
        TypeDef::Record {
            name: name.into(),
            fields,
        }
    }

    /// Create a variant type definition.
    pub fn variant(name: impl Into<String>, cases: Vec<Case>) -> Self {
        TypeDef::Variant {
            name: name.into(),
            cases,
        }
    }

    /// Create an enum type definition.
    pub fn enumeration(name: impl Into<String>, cases: Vec<String>) -> Self {
        TypeDef::Enum {
            name: name.into(),
            cases,
        }
    }

    /// Create a flags type definition.
    pub fn flags(name: impl Into<String>, flags: Vec<String>) -> Self {
        TypeDef::Flags {
            name: name.into(),
            flags,
        }
    }
}

/// A record field.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Field {
    /// Field name
    pub name: String,
    /// Field type
    pub ty: Type,
}

impl Field {
    /// Create a new field.
    pub fn new(name: impl Into<String>, ty: Type) -> Self {
        Self {
            name: name.into(),
            ty,
        }
    }
}

/// A variant case.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Case {
    /// Case name
    pub name: String,
    /// Optional payload type (Unit if no payload)
    pub payload: Type,
}

impl Case {
    /// Create a new case with a payload.
    pub fn new(name: impl Into<String>, payload: Type) -> Self {
        Self {
            name: name.into(),
            payload,
        }
    }

    /// Create a new case without a payload (Unit payload).
    pub fn unit(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            payload: Type::Unit,
        }
    }
}

// ============================================================================
// Type - Type References
// ============================================================================

/// A type reference.
///
/// This enum represents all possible types in the Pack type system.
/// Types can be primitive, compound, or references to named types.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Type {
    // Unit type (explicit, no value)
    Unit,

    // Primitive types
    Bool,
    U8,
    U16,
    U32,
    U64,
    S8,
    S16,
    S32,
    S64,
    F32,
    F64,
    Char,
    String,

    // Compound types
    List(Box<Type>),
    Option(Box<Type>),
    Result { ok: Box<Type>, err: Box<Type> },
    Tuple(Vec<Type>),

    // Named type reference (with qualified path)
    Ref(TypePath),

    // Dynamic value (escape hatch for untyped data)
    Value,
}

impl Type {
    /// Create a list type.
    pub fn list(inner: Type) -> Self {
        Type::List(Box::new(inner))
    }

    /// Create an option type.
    pub fn option(inner: Type) -> Self {
        Type::Option(Box::new(inner))
    }

    /// Create a result type.
    pub fn result(ok: Type, err: Type) -> Self {
        Type::Result {
            ok: Box::new(ok),
            err: Box::new(err),
        }
    }

    /// Create a tuple type.
    pub fn tuple(types: Vec<Type>) -> Self {
        Type::Tuple(types)
    }

    /// Create a reference to a named type by simple name.
    pub fn named(name: impl Into<String>) -> Self {
        Type::Ref(TypePath::simple(name))
    }

    /// Create a self-reference (reference to the containing type).
    /// This is syntactic sugar for a relative path with no segments.
    pub fn self_ref() -> Self {
        Type::Ref(TypePath::self_ref())
    }

    /// Check if this type is Unit.
    pub fn is_unit(&self) -> bool {
        matches!(self, Type::Unit)
    }

    /// Check if this type is a self-reference.
    pub fn is_self_ref(&self) -> bool {
        matches!(self, Type::Ref(path) if path.is_self_ref())
    }

    /// Check if this type contains any recursive references.
    pub fn contains_recursion(&self) -> bool {
        match self {
            Type::Ref(path) if path.is_self_ref() => true,
            Type::List(inner) | Type::Option(inner) => inner.contains_recursion(),
            Type::Result { ok, err } => ok.contains_recursion() || err.contains_recursion(),
            Type::Tuple(types) => types.iter().any(|t| t.contains_recursion()),
            _ => false,
        }
    }
}

// ============================================================================
// TypePath - Qualified Type Paths
// ============================================================================

/// A qualified path to a type.
///
/// Paths can be:
/// - Simple: just a name like "expr"
/// - Qualified: segments like ["wasi", "cli", "stdin"]
/// - Self-reference: empty segments with relative=true
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TypePath {
    /// Path segments (empty for self-reference)
    pub segments: Vec<String>,
    /// Whether this is an absolute or relative path
    pub absolute: bool,
}

impl TypePath {
    /// Create a simple path with just a name.
    pub fn simple(name: impl Into<String>) -> Self {
        Self {
            segments: vec![name.into()],
            absolute: false,
        }
    }

    /// Create an absolute qualified path.
    pub fn absolute(segments: Vec<String>) -> Self {
        Self {
            segments,
            absolute: true,
        }
    }

    /// Create a relative qualified path.
    pub fn relative(segments: Vec<String>) -> Self {
        Self {
            segments,
            absolute: false,
        }
    }

    /// Create a self-reference path.
    pub fn self_ref() -> Self {
        Self {
            segments: Vec::new(),
            absolute: false,
        }
    }

    /// Check if this is a self-reference.
    pub fn is_self_ref(&self) -> bool {
        self.segments.is_empty() && !self.absolute
    }

    /// Check if this is a simple (single-segment) path.
    pub fn is_simple(&self) -> bool {
        self.segments.len() == 1 && !self.absolute
    }

    /// Get the simple name if this is a simple path.
    pub fn as_simple(&self) -> Option<&str> {
        if self.is_simple() {
            self.segments.first().map(|s| s.as_str())
        } else {
            None
        }
    }

    /// Get the last segment (the actual type name).
    pub fn name(&self) -> Option<&str> {
        self.segments.last().map(|s| s.as_str())
    }
}

impl std::fmt::Display for TypePath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.is_self_ref() {
            write!(f, "self")
        } else if self.absolute {
            write!(f, "::{}", self.segments.join("::"))
        } else {
            write!(f, "{}", self.segments.join("::"))
        }
    }
}

// ============================================================================
// Convenience Helpers
// ============================================================================

/// Helper to build an sexpr type (common use case).
pub fn sexpr_type() -> TypeDef {
    TypeDef::Variant {
        name: "sexpr".to_string(),
        cases: vec![
            Case::new("sym", Type::String),
            Case::new("num", Type::S64),
            Case::new("flt", Type::F64),
            Case::new("str", Type::String),
            Case::new("lst", Type::list(Type::self_ref())),
        ],
    }
}

// ============================================================================
// Floating-point Hash implementations
// ============================================================================

// Note: f32 and f64 are included in Type but don't implement Hash by default.
// The Hash derive above uses a custom implementation through the Serialize/Deserialize
// path which handles this correctly for our use case (comparing type structures).
// For actual floating point value comparison, we'd need special handling.

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn test_arena_creation() {
        let mut arena = Arena::new("test");
        arena.add_type(TypeDef::alias("count", Type::U32));
        arena.add_function(Function::with_signature(
            "add",
            vec![Param::new("a", Type::S32), Param::new("b", Type::S32)],
            vec![Type::S32],
        ));

        assert_eq!(arena.name, "test");
        assert_eq!(arena.types.len(), 1);
        assert_eq!(arena.functions.len(), 1);
        assert!(arena.find_type("count").is_some());
        assert!(arena.find_function("add").is_some());
    }

    #[test]
    fn test_type_path() {
        let simple = TypePath::simple("expr");
        assert!(simple.is_simple());
        assert_eq!(simple.as_simple(), Some("expr"));
        assert!(!simple.is_self_ref());

        let self_ref = TypePath::self_ref();
        assert!(self_ref.is_self_ref());
        assert!(!self_ref.is_simple());

        let absolute = TypePath::absolute(vec!["wasi".into(), "cli".into(), "stdout".into()]);
        assert!(absolute.absolute);
        assert_eq!(absolute.name(), Some("stdout"));
    }

    #[test]
    fn test_sexpr_type() {
        let sexpr = sexpr_type();
        assert_eq!(sexpr.name(), "sexpr");
        if let TypeDef::Variant { cases, .. } = &sexpr {
            assert_eq!(cases.len(), 5);
            assert_eq!(cases[0].name, "sym");
            assert_eq!(cases[4].name, "lst");
            // Check that lst case references self
            if let Type::List(inner) = &cases[4].payload {
                assert!(inner.is_self_ref());
            } else {
                panic!("Expected list type");
            }
        } else {
            panic!("Expected variant");
        }
    }

    #[test]
    fn test_contains_recursion() {
        assert!(Type::self_ref().contains_recursion());
        assert!(Type::list(Type::self_ref()).contains_recursion());
        assert!(!Type::list(Type::S32).contains_recursion());
        assert!(!Type::String.contains_recursion());
        assert!(Type::result(Type::self_ref(), Type::String).contains_recursion());
    }

    #[test]
    fn test_type_hashing() {
        let mut set = HashSet::new();

        // Same types should produce same hash
        set.insert(Type::S32);
        assert!(!set.insert(Type::S32)); // Should return false (already exists)

        // Different types should produce different hashes
        assert!(set.insert(Type::S64));
        assert!(set.insert(Type::String));
        assert!(set.insert(Type::list(Type::S32)));
    }

    #[test]
    fn test_arena_hashing() {
        let mut set = HashSet::new();

        let arena1 = Arena::new("test");
        let arena2 = Arena::new("test");
        let arena3 = Arena::new("other");

        set.insert(arena1.clone());
        assert!(!set.insert(arena2)); // Same name, should already exist
        assert!(set.insert(arena3)); // Different name, should be new
    }

    #[test]
    fn test_typedef_name() {
        assert_eq!(TypeDef::alias("foo", Type::S32).name(), "foo");
        assert_eq!(TypeDef::record("bar", vec![]).name(), "bar");
        assert_eq!(TypeDef::variant("baz", vec![]).name(), "baz");
        assert_eq!(TypeDef::enumeration("qux", vec![]).name(), "qux");
        assert_eq!(TypeDef::flags("quux", vec![]).name(), "quux");
    }

    #[test]
    fn test_case_constructors() {
        let with_payload = Case::new("data", Type::String);
        assert_eq!(with_payload.name, "data");
        assert_eq!(with_payload.payload, Type::String);

        let without_payload = Case::unit("empty");
        assert_eq!(without_payload.name, "empty");
        assert_eq!(without_payload.payload, Type::Unit);
    }

    #[test]
    fn test_unit_type() {
        assert!(Type::Unit.is_unit());
        assert!(!Type::S32.is_unit());
        assert!(!Type::String.is_unit());
    }

    #[test]
    fn test_type_display() {
        assert_eq!(TypePath::self_ref().to_string(), "self");
        assert_eq!(TypePath::simple("expr").to_string(), "expr");
        assert_eq!(
            TypePath::absolute(vec!["wasi".into(), "cli".into()]).to_string(),
            "::wasi::cli"
        );
    }
}
