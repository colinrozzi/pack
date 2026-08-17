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
use std::hash::Hash;

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
    /// Interface-level generic type parameters (e.g. `type t: serializable`).
    /// Empty for non-generic interfaces. Carried through into embedded
    /// metadata so composition can identify which signature type-references are
    /// generic parameters (see [`TypeParam`]).
    #[serde(default)]
    pub type_params: Vec<TypeParam>,
}

/// An interface-level generic type parameter, e.g. `type t: serializable`.
///
/// The `constraint` is the name of an interface the bound type must satisfy
/// (currently carried but not yet enforced).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TypeParam {
    /// Parameter name (e.g. "t").
    pub name: String,
    /// Optional constraint — an interface name the concrete type must satisfy.
    #[serde(default)]
    pub constraint: Option<String>,
}

impl TypeParam {
    /// Create a type parameter.
    pub fn new(name: impl Into<String>, constraint: Option<String>) -> Self {
        Self {
            name: name.into(),
            constraint,
        }
    }
}

impl Arena {
    /// Create a new empty arena with the given name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            types: Vec::new(),
            functions: Vec::new(),
            children: Vec::new(),
            type_params: Vec::new(),
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

    /// Get all imported functions from this package arena.
    ///
    /// Returns functions from the "imports" child arena, flattened across interfaces.
    /// Each function includes its interface name in the `interface` field.
    pub fn imports(&self) -> Vec<Function> {
        self.children
            .iter()
            .find(|c| c.name == "imports")
            .map(|imports_arena| {
                imports_arena
                    .children
                    .iter()
                    .flat_map(|interface| {
                        interface.functions.iter().map(|f| {
                            let mut func = f.clone();
                            func.interface = interface.name.clone();
                            func
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Get all exported functions from this package arena.
    ///
    /// Returns functions from the "exports" child arena, flattened across interfaces.
    /// Each function includes its interface name in the `interface` field.
    pub fn exports(&self) -> Vec<Function> {
        self.children
            .iter()
            .find(|c| c.name == "exports")
            .map(|exports_arena| {
                exports_arena
                    .children
                    .iter()
                    .flat_map(|interface| {
                        interface.functions.iter().map(|f| {
                            let mut func = f.clone();
                            func.interface = interface.name.clone();
                            func
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Get imported function names for a specific interface.
    ///
    /// Useful for subset hash verification - given an interface name,
    /// returns the names of functions the actor imports from it.
    pub fn imported_function_names(&self, interface_name: &str) -> Vec<String> {
        self.imports()
            .into_iter()
            .filter(|f| f.interface == interface_name)
            .map(|f| f.name)
            .collect()
    }

    /// Get exported function names for a specific interface.
    pub fn exported_function_names(&self, interface_name: &str) -> Vec<String> {
        self.exports()
            .into_iter()
            .filter(|f| f.interface == interface_name)
            .map(|f| f.name)
            .collect()
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
    /// Interface this function belongs to (for metadata)
    #[serde(default)]
    pub interface: String,
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
            interface: String::new(),
            types: Vec::new(),
            params: Vec::new(),
            results: Vec::new(),
        }
    }

    /// Create a function with parameters and results.
    pub fn with_signature(name: impl Into<String>, params: Vec<Param>, results: Vec<Type>) -> Self {
        Self {
            name: name.into(),
            interface: String::new(),
            types: Vec::new(),
            params,
            results,
        }
    }

    /// Create a function with interface and signature.
    pub fn with_interface(
        name: impl Into<String>,
        interface: impl Into<String>,
        params: Vec<Param>,
        results: Vec<Type>,
    ) -> Self {
        Self {
            name: name.into(),
            interface: interface.into(),
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
    /// Type alias: `type foo = bar` (optionally generic: `type foo<A> = ...`)
    Alias {
        name: String,
        #[serde(default)]
        type_params: Vec<String>,
        ty: Type,
    },

    /// Record type: `record foo { field: type, ... }`
    /// (optionally generic: `record foo<A, B> { ... }`)
    Record {
        name: String,
        #[serde(default)]
        type_params: Vec<String>,
        fields: Vec<Field>,
    },

    /// Variant type: `variant foo { case(payload), ... }`
    /// (optionally generic: `variant foo<T> { ... }`)
    Variant {
        name: String,
        #[serde(default)]
        type_params: Vec<String>,
        cases: Vec<Case>,
    },

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

    /// Get the type parameters of this definition (empty for non-generic
    /// definitions and for enum/flags, which cannot be generic).
    pub fn type_params(&self) -> &[String] {
        match self {
            TypeDef::Alias { type_params, .. }
            | TypeDef::Record { type_params, .. }
            | TypeDef::Variant { type_params, .. } => type_params,
            TypeDef::Enum { .. } | TypeDef::Flags { .. } => &[],
        }
    }

    /// Create an alias type definition.
    pub fn alias(name: impl Into<String>, ty: Type) -> Self {
        TypeDef::Alias {
            name: name.into(),
            type_params: Vec::new(),
            ty,
        }
    }

    /// Create a generic alias type definition.
    pub fn alias_generic(name: impl Into<String>, type_params: Vec<String>, ty: Type) -> Self {
        TypeDef::Alias {
            name: name.into(),
            type_params,
            ty,
        }
    }

    /// Create a record type definition.
    pub fn record(name: impl Into<String>, fields: Vec<Field>) -> Self {
        TypeDef::Record {
            name: name.into(),
            type_params: Vec::new(),
            fields,
        }
    }

    /// Create a generic record type definition.
    pub fn record_generic(
        name: impl Into<String>,
        type_params: Vec<String>,
        fields: Vec<Field>,
    ) -> Self {
        TypeDef::Record {
            name: name.into(),
            type_params,
            fields,
        }
    }

    /// Create a variant type definition.
    pub fn variant(name: impl Into<String>, cases: Vec<Case>) -> Self {
        TypeDef::Variant {
            name: name.into(),
            type_params: Vec::new(),
            cases,
        }
    }

    /// Create a generic variant type definition.
    pub fn variant_generic(
        name: impl Into<String>,
        type_params: Vec<String>,
        cases: Vec<Case>,
    ) -> Self {
        TypeDef::Variant {
            name: name.into(),
            type_params,
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

    /// Instantiate a generic type definition by binding its type parameters to
    /// concrete `args`, producing a monomorphic (non-generic) definition with
    /// every parameter reference substituted away.
    ///
    /// For non-generic definitions (including enum/flags) this is a clone.
    /// Callers are responsible for checking arity (`type_params().len() ==
    /// args.len()`) before calling.
    pub fn instantiate(&self, args: &[Type]) -> TypeDef {
        let env: std::collections::HashMap<String, Type> = self
            .type_params()
            .iter()
            .cloned()
            .zip(args.iter().cloned())
            .collect();
        match self {
            TypeDef::Alias { name, ty, .. } => TypeDef::Alias {
                name: name.clone(),
                type_params: Vec::new(),
                ty: ty.substitute(&env),
            },
            TypeDef::Record { name, fields, .. } => TypeDef::Record {
                name: name.clone(),
                type_params: Vec::new(),
                fields: fields
                    .iter()
                    .map(|f| Field::new(f.name.clone(), f.ty.substitute(&env)))
                    .collect(),
            },
            TypeDef::Variant { name, cases, .. } => TypeDef::Variant {
                name: name.clone(),
                type_params: Vec::new(),
                cases: cases
                    .iter()
                    .map(|c| Case::new(c.name.clone(), c.payload.substitute(&env)))
                    .collect(),
            },
            TypeDef::Enum { .. } | TypeDef::Flags { .. } => self.clone(),
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

    // Map type `map<K, V>`. A front-end convenience that lowers to a
    // `BTreeMap<K, V>` in Rust and marshals as a `list<tuple<K, V>>` on the wire
    // (canonical, key-sorted). Distinct only at the source/codegen level; for
    // hashing, metadata, and validation it is treated as its desugared list form.
    Map { key: Box<Type>, value: Box<Type> },

    // Set type `set<T>`. A front-end convenience that lowers to a `BTreeSet<T>`
    // in Rust and marshals as a `list<T>` on the wire (canonical, key-sorted).
    // Distinct only at the source/codegen level; for hashing, metadata, and
    // validation it is treated as its desugared list form.
    Set(Box<Type>),

    // Named type reference (with qualified path).
    //
    // Also used for references to an in-scope generic type parameter: a bare
    // `Ref(simple("T"))` inside a generic definition's body resolves to the
    // parameter `T` when the surrounding definition declares it, and to a
    // nominal type otherwise. The distinction is made by the resolver's
    // binding environment, not by a dedicated variant.
    Ref(TypePath),

    // Generic type application: a named generic type applied to type
    // arguments, e.g. `pair<u32, string>` or the recursive `tree<T>`.
    App { path: TypePath, args: Vec<Type> },

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

    /// Create a generic type application by simple name, e.g. `pair<u32, string>`.
    pub fn app(name: impl Into<String>, args: Vec<Type>) -> Self {
        Type::App {
            path: TypePath::simple(name),
            args,
        }
    }

    /// Create a map type `map<key, value>`.
    pub fn map(key: Type, value: Type) -> Self {
        Type::Map {
            key: Box::new(key),
            value: Box::new(value),
        }
    }

    /// The desugared wire form of a `map<K, V>`: `list<tuple<K, V>>`. A map
    /// marshals, hashes, and validates exactly as this list of key/value pairs,
    /// so the erased paths (metadata, hashing, validation) delegate to it.
    /// Returns `self` unchanged for a non-map type.
    pub fn desugar_map(&self) -> Type {
        match self {
            Type::Map { key, value } => Type::List(Box::new(Type::Tuple(vec![
                (**key).clone(),
                (**value).clone(),
            ]))),
            other => other.clone(),
        }
    }

    /// Create a set type `set<elem>`.
    pub fn set(elem: Type) -> Self {
        Type::Set(Box::new(elem))
    }

    /// The desugared wire form of a `set<T>`: `list<T>`. A set marshals, hashes,
    /// and validates exactly as this list (canonical, key-sorted), so the erased
    /// paths (metadata, hashing, validation) delegate to it. Returns `self`
    /// unchanged for a non-set type.
    pub fn desugar_set(&self) -> Type {
        match self {
            Type::Set(elem) => Type::List(elem.clone()),
            other => other.clone(),
        }
    }

    /// Substitute in-scope type parameters with concrete types.
    ///
    /// A `Ref` whose simple name is bound in `env` is replaced by the bound
    /// type; every other type is rebuilt with substitution applied to its
    /// component types. This is the core operation behind generic
    /// instantiation (see [`TypeDef::instantiate`]).
    pub fn substitute(&self, env: &std::collections::HashMap<String, Type>) -> Type {
        match self {
            Type::Ref(path) => {
                if let Some(name) = path.as_simple() {
                    if let Some(bound) = env.get(name) {
                        return bound.clone();
                    }
                }
                self.clone()
            }
            Type::List(inner) => Type::List(Box::new(inner.substitute(env))),
            Type::Option(inner) => Type::Option(Box::new(inner.substitute(env))),
            Type::Result { ok, err } => Type::Result {
                ok: Box::new(ok.substitute(env)),
                err: Box::new(err.substitute(env)),
            },
            Type::Tuple(items) => Type::Tuple(items.iter().map(|t| t.substitute(env)).collect()),
            Type::App { path, args } => Type::App {
                path: path.clone(),
                args: args.iter().map(|t| t.substitute(env)).collect(),
            },
            Type::Map { key, value } => Type::Map {
                key: Box::new(key.substitute(env)),
                value: Box::new(value.substitute(env)),
            },
            Type::Set(elem) => Type::Set(Box::new(elem.substitute(env))),
            _ => self.clone(),
        }
    }

    /// Structurally unify this (generic) type against a `concrete` type,
    /// recording bindings for any in-scope generic parameters.
    ///
    /// `params` is the set of generic parameter names in scope; a `Ref` whose
    /// simple name is in `params` binds to whatever concrete type sits opposite
    /// it. Every other construct must match structurally (same constructor,
    /// same arity), recursing into children. Returns `Err` on a structural
    /// mismatch or an inconsistent binding (a parameter forced to two different
    /// types) — this is one-directional unification (the generic side has the
    /// variables, the concrete side is ground), which is all compose-time
    /// interface binding needs.
    pub fn unify(
        &self,
        concrete: &Type,
        params: &std::collections::HashSet<String>,
        bindings: &mut std::collections::HashMap<String, Type>,
    ) -> Result<(), String> {
        // A reference to an in-scope generic parameter binds to `concrete`.
        if let Type::Ref(path) = self {
            if let Some(name) = path.as_simple() {
                if params.contains(name) {
                    if let Some(existing) = bindings.get(name) {
                        if existing != concrete {
                            return Err(format!(
                                "generic parameter `{name}` bound to conflicting types: \
                                 {existing:?} vs {concrete:?}"
                            ));
                        }
                    } else {
                        bindings.insert(name.to_string(), concrete.clone());
                    }
                    return Ok(());
                }
            }
        }

        match (self, concrete) {
            (Type::List(a), Type::List(b)) | (Type::Option(a), Type::Option(b)) => {
                a.unify(b, params, bindings)
            }
            (Type::Result { ok: a, err: c }, Type::Result { ok: b, err: d }) => {
                a.unify(b, params, bindings)?;
                c.unify(d, params, bindings)
            }
            (Type::Tuple(xs), Type::Tuple(ys)) if xs.len() == ys.len() => {
                for (x, y) in xs.iter().zip(ys) {
                    x.unify(y, params, bindings)?;
                }
                Ok(())
            }
            (Type::App { path: p1, args: a1 }, Type::App { path: p2, args: a2 })
                if p1 == p2 && a1.len() == a2.len() =>
            {
                for (x, y) in a1.iter().zip(a2) {
                    x.unify(y, params, bindings)?;
                }
                Ok(())
            }
            (Type::Map { key: k1, value: v1 }, Type::Map { key: k2, value: v2 }) => {
                k1.unify(k2, params, bindings)?;
                v1.unify(v2, params, bindings)
            }
            (Type::Set(e1), Type::Set(e2)) => e1.unify(e2, params, bindings),
            // Non-parameter constructs (primitives, nominal refs) must be equal.
            (a, b) if a == b => Ok(()),
            (a, b) => Err(format!("cannot unify {a:?} with {b:?}")),
        }
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
            Type::App { args, .. } => args.iter().any(|t| t.contains_recursion()),
            Type::Map { key, value } => key.contains_recursion() || value.contains_recursion(),
            Type::Set(elem) => elem.contains_recursion(),
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
        type_params: Vec::new(),
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

    // ========================================================================
    // Generic unification (M4b): infer parameter bindings, then substitute
    // ========================================================================

    fn params(names: &[&str]) -> HashSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn unify_binds_bare_param() {
        let p = params(&["s"]);
        let mut b = std::collections::HashMap::new();
        Type::named("s").unify(&Type::U32, &p, &mut b).unwrap();
        assert_eq!(b.get("s"), Some(&Type::U32));
    }

    #[test]
    fn unify_recurses_into_containers_and_apps() {
        let p = params(&["s"]);

        let mut b = std::collections::HashMap::new();
        Type::list(Type::named("s"))
            .unify(&Type::list(Type::U32), &p, &mut b)
            .unwrap();
        assert_eq!(b.get("s"), Some(&Type::U32));

        // The mesh shape: state<s> unifies with a concrete state<chat-state>.
        let mut b = std::collections::HashMap::new();
        Type::app("state", vec![Type::named("s")])
            .unify(
                &Type::app("state", vec![Type::named("chat-state")]),
                &p,
                &mut b,
            )
            .unwrap();
        assert_eq!(b.get("s"), Some(&Type::named("chat-state")));

        let mut b = std::collections::HashMap::new();
        Type::tuple(vec![Type::named("s"), Type::String])
            .unify(&Type::tuple(vec![Type::U32, Type::String]), &p, &mut b)
            .unwrap();
        assert_eq!(b.get("s"), Some(&Type::U32));
    }

    #[test]
    fn unify_rejects_inconsistent_binding() {
        let p = params(&["s"]);
        let mut b = std::collections::HashMap::new();
        // tuple<s, s> cannot unify with tuple<u32, string>.
        assert!(Type::tuple(vec![Type::named("s"), Type::named("s")])
            .unify(&Type::tuple(vec![Type::U32, Type::String]), &p, &mut b)
            .is_err());
    }

    #[test]
    fn unify_rejects_structural_mismatch() {
        let p = params(&[]);
        let mut b = std::collections::HashMap::new();
        assert!(Type::U32.unify(&Type::String, &p, &mut b).is_err());
        let mut b = std::collections::HashMap::new();
        assert!(Type::app("a", vec![Type::U32])
            .unify(&Type::app("b", vec![Type::U32]), &p, &mut b)
            .is_err());
    }

    #[test]
    fn unify_then_substitute_reproduces_concrete() {
        // The compose contract: infer by unifying, monomorphize by substituting,
        // and the result equals the concrete side (so their hashes agree).
        let p = params(&["s"]);
        let generic = Type::tuple(vec![
            Type::app("state", vec![Type::named("s")]),
            Type::String,
        ]);
        let concrete = Type::tuple(vec![
            Type::app("state", vec![Type::named("chat-state")]),
            Type::String,
        ]);
        let mut b = std::collections::HashMap::new();
        generic.unify(&concrete, &p, &mut b).unwrap();
        assert_eq!(generic.substitute(&b), concrete);
    }
}
