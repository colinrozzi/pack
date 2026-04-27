//! Interface Transformation System
//!
//! This module provides a system for transforming interfaces before use.
//! The primary use case is the `rpc` transform which wraps all return types
//! in `result<T, rpc-error>` to handle RPC failure modes.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────┐
//! │           Interface Transform               │
//! │                                             │
//! │  Original Interface    Transformed Interface│
//! │  ┌───────────────┐    ┌───────────────────┐ │
//! │  │ func() -> T   │ => │ func() ->         │ │
//! │  │               │    │   result<T, err>  │ │
//! │  └───────────────┘    └───────────────────┘ │
//! └─────────────────────────────────────────────┘
//! ```
//!
//! # Design Principles
//!
//! - **Asymmetric**: Actors implement the original interface, callers use transformed
//! - **Extensible**: User-defined transformations via the trait
//! - **Composable**: Transforms can be chained: `traced(rpc(calculator))`
//!
//! # Example
//!
//! ```pact
//! interface calculator {
//!     exports {
//!         add: func(a: s32, b: s32) -> s32
//!     }
//! }
//!
//! interface caller {
//!     use rpc(calculator)
//!     // Now has: add: func(a: s32, b: s32) -> result<s32, rpc-error>
//! }
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use crate::parser::{PactExport, PactInterface};
use crate::types::{Case, Type, TypeDef};

// ============================================================================
// Interface Transform Trait
// ============================================================================

/// A transformation that can be applied to an interface.
///
/// Transforms modify the interface definition, typically by wrapping
/// return types or adding error handling.
pub trait InterfaceTransform: Send + Sync {
    /// The name of this transform (e.g., "rpc", "traced").
    fn name(&self) -> &str;

    /// Transform an interface, returning a modified copy.
    ///
    /// The base interface is not modified; a new interface is returned.
    fn transform(&self, base: &PactInterface) -> PactInterface;
}

// ============================================================================
// Transform Registry
// ============================================================================

/// Registry of available interface transforms.
///
/// The registry maintains a collection of named transforms that can be
/// looked up and applied to interfaces.
#[derive(Clone, Default)]
pub struct TransformRegistry {
    transforms: HashMap<String, Arc<dyn InterfaceTransform>>,
}

impl TransformRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a registry with the built-in transforms registered.
    pub fn with_builtins() -> Self {
        let mut registry = Self::new();
        registry.register(Arc::new(RpcTransform));
        registry
    }

    /// Register a transform.
    pub fn register(&mut self, transform: Arc<dyn InterfaceTransform>) {
        self.transforms
            .insert(transform.name().to_string(), transform);
    }

    /// Look up a transform by name.
    pub fn get(&self, name: &str) -> Option<&Arc<dyn InterfaceTransform>> {
        self.transforms.get(name)
    }

    /// Check if a transform exists.
    pub fn contains(&self, name: &str) -> bool {
        self.transforms.contains_key(name)
    }

    /// List all registered transform names.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.transforms.keys().map(|s| s.as_str())
    }
}

impl std::fmt::Debug for TransformRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TransformRegistry")
            .field("transforms", &self.transforms.keys().collect::<Vec<_>>())
            .finish()
    }
}

// ============================================================================
// RPC Transform
// ============================================================================

/// The RPC transform wraps all export return types in `result<T, rpc-error>`.
///
/// This transform is used when calling an interface over an RPC boundary,
/// where calls can fail due to network issues, actor lifecycle events, etc.
///
/// # Transformation
///
/// For each exported function:
/// - `func() -> T` becomes `func() -> result<T, rpc-error>`
/// - `func()` (no return) becomes `func() -> result<_, rpc-error>`
/// - `func() -> result<T, E>` becomes `func() -> result<result<T, E>, rpc-error>`
///
/// The transform also adds the `rpc-error` variant type to the interface.
#[derive(Debug, Clone, Copy)]
pub struct RpcTransform;

impl RpcTransform {
    /// Create the rpc-error variant type definition.
    pub fn rpc_error_typedef() -> TypeDef {
        TypeDef::variant(
            "rpc-error",
            vec![
                Case::unit("timeout"),
                Case::new("actor-not-found", Type::String),
                Case::new("function-not-found", Type::String),
                Case::unit("shutting-down"),
                Case::unit("channel-closed"),
                Case::new("call-failed", Type::String),
            ],
        )
    }

    /// Wrap a return type in result<T, rpc-error>.
    fn wrap_return_type(ty: &Type) -> Type {
        Type::result(ty.clone(), Type::named("rpc-error"))
    }

    /// Wrap a function's return types.
    fn wrap_function_returns(results: &[Type]) -> Vec<Type> {
        if results.is_empty() {
            // No return -> result<_, rpc-error> (using unit type for ok)
            vec![Type::result(Type::Unit, Type::named("rpc-error"))]
        } else {
            // Wrap each return type
            results.iter().map(Self::wrap_return_type).collect()
        }
    }
}

impl InterfaceTransform for RpcTransform {
    fn name(&self) -> &str {
        "rpc"
    }

    fn transform(&self, base: &PactInterface) -> PactInterface {
        let mut result = base.clone();

        // Update the name to indicate it's transformed
        result.name = format!("rpc({})", base.name);

        // Add the rpc-error type
        result.types.push(Self::rpc_error_typedef());

        // Wrap all export function returns
        for export in &mut result.exports {
            if let PactExport::Function(func) = export {
                func.results = Self::wrap_function_returns(&func.results);
            }
        }

        result
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_pact;

    #[test]
    fn test_rpc_error_typedef() {
        let typedef = RpcTransform::rpc_error_typedef();
        assert_eq!(typedef.name(), "rpc-error");

        if let TypeDef::Variant { cases, .. } = &typedef {
            assert_eq!(cases.len(), 6);
            assert_eq!(cases[0].name, "timeout");
            assert_eq!(cases[1].name, "actor-not-found");
            assert_eq!(cases[2].name, "function-not-found");
            assert_eq!(cases[3].name, "shutting-down");
            assert_eq!(cases[4].name, "channel-closed");
            assert_eq!(cases[5].name, "call-failed");
        } else {
            panic!("Expected variant type");
        }
    }

    #[test]
    fn test_rpc_transform_simple() {
        let src = r#"
            interface calculator {
                exports {
                    add: func(a: s32, b: s32) -> s32
                }
            }
        "#;

        let interface = parse_pact(src).expect("parse");
        let transform = RpcTransform;
        let transformed = transform.transform(&interface);

        // Check name is updated
        assert_eq!(transformed.name, "rpc(calculator)");

        // Check rpc-error type is added
        assert!(transformed.types.iter().any(|t| t.name() == "rpc-error"));

        // Check the function return type is wrapped
        if let PactExport::Function(func) = &transformed.exports[0] {
            assert_eq!(func.results.len(), 1);
            match &func.results[0] {
                Type::Result { ok, err } => {
                    assert_eq!(**ok, Type::S32);
                    assert_eq!(**err, Type::named("rpc-error"));
                }
                _ => panic!("Expected result type"),
            }
        } else {
            panic!("Expected function export");
        }
    }

    #[test]
    fn test_rpc_transform_no_return() {
        let src = r#"
            interface logger {
                exports {
                    log: func(msg: string)
                }
            }
        "#;

        let interface = parse_pact(src).expect("parse");
        let transform = RpcTransform;
        let transformed = transform.transform(&interface);

        // Function with no return should get result<_, rpc-error>
        if let PactExport::Function(func) = &transformed.exports[0] {
            assert_eq!(func.results.len(), 1);
            match &func.results[0] {
                Type::Result { ok, err } => {
                    assert_eq!(**ok, Type::Unit);
                    assert_eq!(**err, Type::named("rpc-error"));
                }
                _ => panic!("Expected result type"),
            }
        }
    }

    #[test]
    fn test_rpc_transform_nested_result() {
        let src = r#"
            interface calculator {
                exports {
                    divide: func(a: s32, b: s32) -> result<s32, string>
                }
            }
        "#;

        let interface = parse_pact(src).expect("parse");
        let transform = RpcTransform;
        let transformed = transform.transform(&interface);

        // result<s32, string> should become result<result<s32, string>, rpc-error>
        if let PactExport::Function(func) = &transformed.exports[0] {
            match &func.results[0] {
                Type::Result { ok, err } => {
                    // ok should be result<s32, string>
                    match ok.as_ref() {
                        Type::Result {
                            ok: inner_ok,
                            err: inner_err,
                        } => {
                            assert_eq!(**inner_ok, Type::S32);
                            assert_eq!(**inner_err, Type::String);
                        }
                        _ => panic!("Expected nested result for ok type"),
                    }
                    // err should be rpc-error
                    assert_eq!(**err, Type::named("rpc-error"));
                }
                _ => panic!("Expected result type"),
            }
        }
    }

    #[test]
    fn test_transform_registry() {
        let registry = TransformRegistry::with_builtins();

        assert!(registry.contains("rpc"));
        assert!(!registry.contains("nonexistent"));

        let rpc = registry.get("rpc").expect("rpc transform");
        assert_eq!(rpc.name(), "rpc");
    }

    #[test]
    fn test_parse_transform_use() {
        let src = r#"
            interface caller {
                use rpc(calculator)
            }
        "#;

        let interface = parse_pact(src).expect("parse");
        assert_eq!(interface.uses.len(), 1);
        assert_eq!(interface.uses[0].interface, "rpc");
        assert_eq!(interface.uses[0].transform_args, vec!["calculator"]);
    }

    #[test]
    fn test_parse_interface_alias() {
        let src = r#"
            interface test {
                interface calc-client = rpc(calculator)
            }
        "#;

        let interface = parse_pact(src).expect("parse");
        assert_eq!(interface.aliases.len(), 1);
        assert_eq!(interface.aliases[0].name, "calc-client");
        assert_eq!(interface.aliases[0].transform, "rpc");
        assert_eq!(interface.aliases[0].args, vec!["calculator"]);
    }
}
