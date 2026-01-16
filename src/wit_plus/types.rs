//! WIT+ Type Definitions
//!
//! Supports all standard WIT types plus recursive types.

use serde::{Deserialize, Serialize};

/// A type definition (named type)
#[derive(Debug, Clone)]
pub enum TypeDef {
    /// type foo = bar
    Alias(String, Type),

    /// record foo { ... }
    Record(RecordDef),

    /// variant foo { ... }
    Variant(VariantDef),

    /// enum foo { ... }
    Enum(EnumDef),

    /// flags foo { ... }
    Flags(FlagsDef),

    /// rec variant foo { ... }
    /// Recursive types - the key extension!
    Recursive(RecursiveDef),
}

impl TypeDef {
    pub fn name(&self) -> &str {
        match self {
            TypeDef::Alias(name, _) => name,
            TypeDef::Record(r) => &r.name,
            TypeDef::Variant(v) => &v.name,
            TypeDef::Enum(e) => &e.name,
            TypeDef::Flags(f) => &f.name,
            TypeDef::Recursive(r) => &r.name,
        }
    }

    pub fn is_recursive(&self) -> bool {
        matches!(self, TypeDef::Recursive(_))
    }
}

/// A type reference
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Type {
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
    Result {
        ok: Option<Box<Type>>,
        err: Option<Box<Type>>,
    },
    Tuple(Vec<Type>),

    // Named type reference
    Named(String),

    // Self-reference within a recursive type
    SelfRef,
}

impl Type {
    pub fn list(inner: Type) -> Self {
        Type::List(Box::new(inner))
    }

    pub fn option(inner: Type) -> Self {
        Type::Option(Box::new(inner))
    }

    pub fn result(ok: Option<Type>, err: Option<Type>) -> Self {
        Type::Result {
            ok: ok.map(Box::new),
            err: err.map(Box::new),
        }
    }

    /// Check if this type contains any recursive references
    pub fn contains_recursion(&self) -> bool {
        match self {
            Type::SelfRef => true,
            Type::List(inner) => inner.contains_recursion(),
            Type::Option(inner) => inner.contains_recursion(),
            Type::Result { ok, err } => {
                ok.as_ref().map_or(false, |t| t.contains_recursion())
                    || err.as_ref().map_or(false, |t| t.contains_recursion())
            }
            Type::Tuple(types) => types.iter().any(|t| t.contains_recursion()),
            _ => false,
        }
    }
}

/// record name { field: type, ... }
#[derive(Debug, Clone)]
pub struct RecordDef {
    pub name: String,
    pub fields: Vec<(String, Type)>,
}

/// variant name { case(payload), ... }
#[derive(Debug, Clone)]
pub struct VariantDef {
    pub name: String,
    pub cases: Vec<VariantCase>,
}

#[derive(Debug, Clone)]
pub struct VariantCase {
    pub name: String,
    pub payload: Option<Type>,
}

/// enum name { case1, case2, ... }
#[derive(Debug, Clone)]
pub struct EnumDef {
    pub name: String,
    pub cases: Vec<String>,
}

/// flags name { flag1, flag2, ... }
#[derive(Debug, Clone)]
pub struct FlagsDef {
    pub name: String,
    pub flags: Vec<String>,
}

/// rec variant name { ... }
/// A recursive type definition - uses serialization ABI instead of fixed layout
#[derive(Debug, Clone)]
pub struct RecursiveDef {
    pub name: String,
    pub cases: Vec<VariantCase>,
}

impl RecursiveDef {
    /// Create a new recursive variant definition
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            cases: Vec::new(),
        }
    }

    /// Add a case with no payload
    pub fn add_case(&mut self, name: impl Into<String>) {
        self.cases.push(VariantCase {
            name: name.into(),
            payload: None,
        });
    }

    /// Add a case with a payload type
    pub fn add_case_with_payload(&mut self, name: impl Into<String>, payload: Type) {
        self.cases.push(VariantCase {
            name: name.into(),
            payload: Some(payload),
        });
    }
}

/// Helper to build an sexpr type (common use case)
pub fn sexpr_type() -> RecursiveDef {
    let mut def = RecursiveDef::new("sexpr");
    def.add_case_with_payload("sym", Type::String);
    def.add_case_with_payload("num", Type::S64);
    def.add_case_with_payload("flt", Type::F64);
    def.add_case_with_payload("str", Type::String);
    def.add_case_with_payload("lst", Type::list(Type::SelfRef));
    def
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sexpr_type() {
        let sexpr = sexpr_type();
        assert_eq!(sexpr.name, "sexpr");
        assert_eq!(sexpr.cases.len(), 5);

        // Check that lst case references self
        let lst_case = &sexpr.cases[4];
        assert_eq!(lst_case.name, "lst");
        if let Some(Type::List(inner)) = &lst_case.payload {
            assert_eq!(**inner, Type::SelfRef);
        } else {
            panic!("Expected list type");
        }
    }

    #[test]
    fn test_contains_recursion() {
        assert!(Type::SelfRef.contains_recursion());
        assert!(Type::list(Type::SelfRef).contains_recursion());
        assert!(!Type::list(Type::S32).contains_recursion());
        assert!(!Type::String.contains_recursion());
    }
}
