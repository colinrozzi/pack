//! Interface Implementation Builder
//!
//! Provides a unified way to declare and implement host interfaces.
//! The interface declaration and implementation are combined, ensuring
//! they can never drift apart.
//!
//! # Example
//!
//! ```ignore
//! let interface = InterfaceImpl::new("theater:simple/runtime")
//!     .func("log", |ctx: &mut Ctx<State>, msg: String| {
//!         println!("{}", msg);
//!     })
//!     .func("get-state", |ctx: &mut Ctx<State>| -> String {
//!         ctx.data().state.clone()
//!     });
//!
//! // Get the interface hash (computed from function signatures)
//! let hash = interface.hash();
//!
//! // Register with a linker
//! builder.register_interface(&interface)?;
//! ```

use crate::metadata::TypeHash;
use crate::parser::{MetadataValue, PactExport, PactInterface};
use crate::types::{Type, TypeDef};

// ============================================================================
// PackType Trait - Maps Rust types to Pack types
// ============================================================================

/// Trait for types that can be represented in Pack's type system.
///
/// This enables automatic signature extraction from Rust closures.
pub trait PackType {
    /// The Pack type representation of this Rust type.
    fn pack_type() -> Type;
}

// Primitive implementations
impl PackType for () {
    fn pack_type() -> Type {
        Type::Unit
    }
}

impl PackType for bool {
    fn pack_type() -> Type {
        Type::Bool
    }
}

impl PackType for u8 {
    fn pack_type() -> Type {
        Type::U8
    }
}

impl PackType for u16 {
    fn pack_type() -> Type {
        Type::U16
    }
}

impl PackType for u32 {
    fn pack_type() -> Type {
        Type::U32
    }
}

impl PackType for u64 {
    fn pack_type() -> Type {
        Type::U64
    }
}

impl PackType for i8 {
    fn pack_type() -> Type {
        Type::S8
    }
}

impl PackType for i16 {
    fn pack_type() -> Type {
        Type::S16
    }
}

impl PackType for i32 {
    fn pack_type() -> Type {
        Type::S32
    }
}

impl PackType for i64 {
    fn pack_type() -> Type {
        Type::S64
    }
}

impl PackType for f32 {
    fn pack_type() -> Type {
        Type::F32
    }
}

impl PackType for f64 {
    fn pack_type() -> Type {
        Type::F64
    }
}

impl PackType for char {
    fn pack_type() -> Type {
        Type::Char
    }
}

impl PackType for String {
    fn pack_type() -> Type {
        Type::String
    }
}

impl PackType for crate::abi::Value {
    fn pack_type() -> Type {
        Type::Value
    }
}

impl<T: PackType> PackType for Vec<T> {
    fn pack_type() -> Type {
        Type::List(Box::new(T::pack_type()))
    }
}

impl<T: PackType> PackType for Option<T> {
    fn pack_type() -> Type {
        Type::Option(Box::new(T::pack_type()))
    }
}

impl<T: PackType, E: PackType> PackType for Result<T, E> {
    fn pack_type() -> Type {
        Type::Result {
            ok: Box::new(T::pack_type()),
            err: Box::new(E::pack_type()),
        }
    }
}

// Tuple implementations
impl<A: PackType> PackType for (A,) {
    fn pack_type() -> Type {
        Type::Tuple(vec![A::pack_type()])
    }
}

impl<A: PackType, B: PackType> PackType for (A, B) {
    fn pack_type() -> Type {
        Type::Tuple(vec![A::pack_type(), B::pack_type()])
    }
}

impl<A: PackType, B: PackType, C: PackType> PackType for (A, B, C) {
    fn pack_type() -> Type {
        Type::Tuple(vec![A::pack_type(), B::pack_type(), C::pack_type()])
    }
}

impl<A: PackType, B: PackType, C: PackType, D: PackType> PackType for (A, B, C, D) {
    fn pack_type() -> Type {
        Type::Tuple(vec![
            A::pack_type(),
            B::pack_type(),
            C::pack_type(),
            D::pack_type(),
        ])
    }
}

// ============================================================================
// Function Signature
// ============================================================================

/// A function signature extracted from Rust types.
#[derive(Debug, Clone)]
pub struct FuncSignature {
    pub name: String,
    pub params: Vec<Type>,
    pub results: Vec<Type>,
}

impl FuncSignature {
    /// Compute the hash of this function signature without resolving named refs.
    ///
    /// Equivalent to `hash_in(&[])`. Suitable for `InterfaceImpl::func`-built
    /// signatures where types come from `PackType` (no `Type::Ref`s appear).
    pub fn hash(&self) -> TypeHash {
        self.hash_in(&[])
    }

    /// Compute the hash, resolving named refs against `types`.
    ///
    /// Used by `from_pact`-built interfaces where function signatures may
    /// reference records/variants declared at the pact-interface level.
    pub fn hash_in(&self, types: &[TypeDef]) -> TypeHash {
        let param_hashes: Vec<_> = self
            .params
            .iter()
            .map(|t| crate::metadata::hash_type_in(t, types))
            .collect();
        let result_hashes: Vec<_> = self
            .results
            .iter()
            .map(|t| crate::metadata::hash_type_in(t, types))
            .collect();
        crate::metadata::hash_function(&param_hashes, &result_hashes)
    }
}

// ============================================================================
// Interface Implementation
// ============================================================================

/// A declared and implemented interface.
///
/// This combines the interface signature with its implementation,
/// ensuring they can never drift apart.
#[derive(Debug)]
pub struct InterfaceImpl {
    /// The interface name (e.g., "theater:simple/runtime")
    pub name: String,
    /// Type definitions in scope for ref resolution when hashing.
    ///
    /// Populated by `from_pact` from `pact.types`. Manually-constructed
    /// interfaces (via `new` + `func`) leave this empty — their signatures
    /// come from `PackType` and don't contain `Type::Ref`s.
    pub types: Vec<TypeDef>,
    /// Function signatures (extracted from Rust types)
    pub functions: Vec<FuncSignature>,
}

impl InterfaceImpl {
    /// Create a new interface implementation.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            types: Vec::new(),
            functions: Vec::new(),
        }
    }

    /// Declare and implement a function.
    ///
    /// The signature is automatically extracted from the closure's types.
    pub fn func<F, Args, Ret>(mut self, name: &str, _f: F) -> Self
    where
        F: HostFunc<Args, Ret>,
        Args: PackParams,
        Ret: PackType + 'static,
    {
        let params = Args::pack_types();
        let results = if std::any::TypeId::of::<Ret>() == std::any::TypeId::of::<()>() {
            vec![]
        } else {
            vec![Ret::pack_type()]
        };

        self.functions.push(FuncSignature {
            name: name.to_string(),
            params,
            results,
        });

        self
    }

    /// Compute the interface hash from all function signatures.
    pub fn hash(&self) -> TypeHash {
        use crate::metadata::Binding;

        // Create bindings for each function (sorted by name for determinism)
        let mut bindings: Vec<_> = self
            .functions
            .iter()
            .map(|f| Binding {
                name: &f.name,
                hash: f.hash_in(&self.types),
            })
            .collect();
        bindings.sort_by(|a, b| a.name.cmp(b.name));

        crate::metadata::hash_interface(
            &self.name,
            &[], // No type bindings for now
            &bindings,
        )
    }

    /// Compute the interface hash for a subset of functions.
    ///
    /// This enables partial interface matching - an actor that imports only
    /// some functions from an interface can still verify compatibility with
    /// a handler that exports the full interface.
    ///
    /// Returns None if any requested function is not found in this interface.
    pub fn hash_subset(&self, function_names: &[&str]) -> Option<TypeHash> {
        use crate::metadata::Binding;

        // Find the requested functions and compute their hashes
        let mut bindings = Vec::with_capacity(function_names.len());
        for name in function_names {
            let func = self.functions.iter().find(|f| f.name == *name)?;
            bindings.push(Binding {
                name: &func.name,
                hash: func.hash_in(&self.types),
            });
        }

        // Sort by name for deterministic hashing
        bindings.sort_by(|a, b| a.name.cmp(b.name));

        Some(crate::metadata::hash_interface(
            &self.name,
            &[], // No type bindings for now
            &bindings,
        ))
    }

    /// Get the hash for a specific function by name.
    ///
    /// Useful for per-function verification.
    pub fn function_hash(&self, name: &str) -> Option<TypeHash> {
        self.functions
            .iter()
            .find(|f| f.name == name)
            .map(|f| f.hash_in(&self.types))
    }

    /// Get the interface name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Get the function signatures.
    pub fn signatures(&self) -> &[FuncSignature] {
        &self.functions
    }

    /// Create an InterfaceImpl from a parsed PactInterface.
    ///
    /// This constructs the full interface name from the `@package` metadata and
    /// the interface name, then extracts all exported function signatures.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let pact = parse_pact(include_str!("../../../pact/runtime.pact"))
    ///     .expect("embedded runtime.pact should be valid");
    /// let interface = InterfaceImpl::from_pact(&pact);
    /// ```
    pub fn from_pact(pact: &PactInterface) -> Self {
        // Get package from metadata (e.g., "theater:simple")
        let package = pact
            .metadata
            .iter()
            .find(|m| m.name == "package")
            .and_then(|m| match &m.value {
                MetadataValue::String(s) => Some(s.as_str()),
                _ => None,
            })
            .unwrap_or("");

        // Construct full interface name: "package/interface-name"
        let full_name = if package.is_empty() {
            pact.name.clone()
        } else {
            format!("{}/{}", package, pact.name)
        };

        let mut interface = InterfaceImpl::new(full_name);

        // Capture interface-level typedefs so refs in function signatures
        // (e.g. `func run(spec: container-spec) -> ...`) resolve structurally
        // when the interface hash is computed.
        interface.types = pact.types.clone();
        // Type-style exports (`PactExport::Type`) also contribute to the
        // resolution scope alongside the inline `record/variant/...` defs.
        for export in &pact.exports {
            if let PactExport::Type(td) = export {
                interface.types.push(td.clone());
            }
        }

        // Extract function signatures from exports
        for export in &pact.exports {
            if let PactExport::Function(func) = export {
                let params: Vec<Type> = func.params.iter().map(|p| p.ty.clone()).collect();
                let results: Vec<Type> = func.results.clone();

                interface.functions.push(FuncSignature {
                    name: func.name.clone(),
                    params,
                    results,
                });
            }
        }

        interface
    }
}

// ============================================================================
// Host Function Traits
// ============================================================================

/// Trait for extracting parameter types from a tuple.
pub trait PackParams {
    fn pack_types() -> Vec<Type>;
}

impl PackParams for () {
    fn pack_types() -> Vec<Type> {
        vec![]
    }
}

impl<A: PackType> PackParams for (A,) {
    fn pack_types() -> Vec<Type> {
        vec![A::pack_type()]
    }
}

impl<A: PackType, B: PackType> PackParams for (A, B) {
    fn pack_types() -> Vec<Type> {
        vec![A::pack_type(), B::pack_type()]
    }
}

impl<A: PackType, B: PackType, C: PackType> PackParams for (A, B, C) {
    fn pack_types() -> Vec<Type> {
        vec![A::pack_type(), B::pack_type(), C::pack_type()]
    }
}

impl<A: PackType, B: PackType, C: PackType, D: PackType> PackParams for (A, B, C, D) {
    fn pack_types() -> Vec<Type> {
        vec![
            A::pack_type(),
            B::pack_type(),
            C::pack_type(),
            D::pack_type(),
        ]
    }
}

/// Marker trait for host functions.
///
/// This trait is implemented for closures that can be used as host functions.
/// It allows us to extract the parameter and return types at compile time.
pub trait HostFunc<Args, Ret> {}

// Implement for various closure signatures
// Note: In practice, these would need to match the actual host function signatures
// that include Ctx<T> as the first parameter.

impl<F, Ret> HostFunc<(), Ret> for F
where
    F: Fn() -> Ret,
    Ret: PackType,
{
}

impl<F, A, Ret> HostFunc<(A,), Ret> for F
where
    F: Fn(A) -> Ret,
    A: PackType,
    Ret: PackType,
{
}

impl<F, A, B, Ret> HostFunc<(A, B), Ret> for F
where
    F: Fn(A, B) -> Ret,
    A: PackType,
    B: PackType,
    Ret: PackType,
{
}

impl<F, A, B, C, Ret> HostFunc<(A, B, C), Ret> for F
where
    F: Fn(A, B, C) -> Ret,
    A: PackType,
    B: PackType,
    C: PackType,
    Ret: PackType,
{
}

impl<F, A, B, C, D, Ret> HostFunc<(A, B, C, D), Ret> for F
where
    F: Fn(A, B, C, D) -> Ret,
    A: PackType,
    B: PackType,
    C: PackType,
    D: PackType,
    Ret: PackType,
{
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pack_type_primitives() {
        assert!(matches!(String::pack_type(), Type::String));
        assert!(matches!(i32::pack_type(), Type::S32));
        assert!(matches!(bool::pack_type(), Type::Bool));
    }

    #[test]
    fn test_pack_type_compound() {
        let list_type = Vec::<u8>::pack_type();
        assert!(matches!(list_type, Type::List(_)));

        let option_type = Option::<String>::pack_type();
        assert!(matches!(option_type, Type::Option(_)));

        let result_type = Result::<String, String>::pack_type();
        assert!(matches!(result_type, Type::Result { .. }));
    }

    #[test]
    fn test_interface_impl_basic() {
        let interface = InterfaceImpl::new("test:example/api")
            .func("greet", |name: String| -> String {
                format!("Hello, {}!", name)
            })
            .func("add", |a: i32, b: i32| -> i32 { a + b });

        assert_eq!(interface.name(), "test:example/api");
        assert_eq!(interface.functions.len(), 2);

        // Check signatures
        let greet = &interface.functions[0];
        assert_eq!(greet.name, "greet");
        assert_eq!(greet.params.len(), 1);
        assert!(matches!(greet.params[0], Type::String));
        assert_eq!(greet.results.len(), 1);
        assert!(matches!(greet.results[0], Type::String));

        let add = &interface.functions[1];
        assert_eq!(add.name, "add");
        assert_eq!(add.params.len(), 2);
    }

    #[test]
    fn test_interface_hash_deterministic() {
        let interface1 = InterfaceImpl::new("test:api")
            .func("foo", |x: i32| -> i32 { x })
            .func("bar", |s: String| -> String { s });

        let interface2 = InterfaceImpl::new("test:api")
            .func("bar", |s: String| -> String { s }) // Different order
            .func("foo", |x: i32| -> i32 { x });

        // Same interface, different declaration order -> same hash
        assert_eq!(interface1.hash(), interface2.hash());
    }

    #[test]
    fn test_interface_hash_differs_on_signature() {
        let interface1 = InterfaceImpl::new("test:api").func("foo", |x: i32| -> i32 { x });

        let interface2 = InterfaceImpl::new("test:api").func("foo", |x: i64| -> i64 { x }); // Different type!

        assert_ne!(interface1.hash(), interface2.hash());
    }

    #[test]
    fn test_from_pact_basic() {
        use crate::parser::parse_pact;

        let src = r#"
            interface runtime {
                @package: string = "theater:simple"

                exports {
                    log: func(msg: string)
                    get-chain: func() -> list<u8>
                    shutdown: func(data: option<list<u8>>) -> result<_, string>
                }
            }
        "#;

        let pact = parse_pact(src).expect("should parse");
        let interface = InterfaceImpl::from_pact(&pact);

        assert_eq!(interface.name(), "theater:simple/runtime");
        assert_eq!(interface.functions.len(), 3);

        // Check function names
        let names: Vec<&str> = interface
            .functions
            .iter()
            .map(|f| f.name.as_str())
            .collect();
        assert!(names.contains(&"log"));
        assert!(names.contains(&"get-chain"));
        assert!(names.contains(&"shutdown"));

        // Check log function signature
        let log_fn = interface
            .functions
            .iter()
            .find(|f| f.name == "log")
            .unwrap();
        assert_eq!(log_fn.params.len(), 1);
        assert!(matches!(log_fn.params[0], Type::String));
        assert!(log_fn.results.is_empty());

        // Check get-chain function signature
        let get_chain_fn = interface
            .functions
            .iter()
            .find(|f| f.name == "get-chain")
            .unwrap();
        assert!(get_chain_fn.params.is_empty());
        assert_eq!(get_chain_fn.results.len(), 1);
        assert!(matches!(get_chain_fn.results[0], Type::List(_)));
    }

    #[test]
    fn test_from_pact_hash_determinism() {
        use crate::parser::parse_pact;

        let src = r#"
            interface test {
                @package: string = "my:package"

                exports {
                    add: func(a: s32, b: s32) -> s32
                    subtract: func(a: s32, b: s32) -> s32
                }
            }
        "#;

        let pact1 = parse_pact(src).expect("should parse");
        let pact2 = parse_pact(src).expect("should parse");

        let interface1 = InterfaceImpl::from_pact(&pact1);
        let interface2 = InterfaceImpl::from_pact(&pact2);

        assert_eq!(interface1.hash(), interface2.hash());
    }

    #[test]
    fn test_from_pact_matches_hand_coded() {
        use crate::parser::parse_pact;

        // Hand-coded interface
        let hand_coded = InterfaceImpl::new("test:example/calculator")
            .func("add", |_a: i32, _b: i32| -> i32 { 0 })
            .func("subtract", |_a: i32, _b: i32| -> i32 { 0 });

        // Pact-based interface
        let src = r#"
            interface calculator {
                @package: string = "test:example"

                exports {
                    add: func(a: s32, b: s32) -> s32
                    subtract: func(a: s32, b: s32) -> s32
                }
            }
        "#;

        let pact = parse_pact(src).expect("should parse");
        let from_pact = InterfaceImpl::from_pact(&pact);

        // Names should match
        assert_eq!(hand_coded.name(), from_pact.name());

        // Hashes should match
        assert_eq!(
            hand_coded.hash(),
            from_pact.hash(),
            "Hand-coded interface hash should match pact-based interface hash"
        );
    }
}
