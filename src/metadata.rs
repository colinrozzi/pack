//! Host-side metadata types for package type information.
//!
//! Packages embed CGRF-encoded metadata accessible via `__pack_types`.
//! This module provides types and a decoder for that metadata.
//!
//! Uses the unified type system from `crate::types`.
//!
//! ## Interface Hashes (Merkle Tree)
//!
//! Each interface has a content-addressed hash computed from its structure:
//! - Types are hashed structurally (field names included, type names excluded)
//! - Functions are hashed by their signature (param types + result types)
//! - Interfaces include bindings (name → hash pairs)
//!
//! This enables O(1) compatibility checking: if hashes match, interfaces are compatible.

use crate::abi::{decode, encode, Value};
use crate::types::{Arena, Case, Field, Function, Param, Type, TypePath};

// ============================================================================
// Interface Hashes
// ============================================================================

/// A 256-bit type hash for Merkle tree-based type compatibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TypeHash([u8; 32]);

impl TypeHash {
    /// Create from raw bytes.
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Get the raw bytes.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Create from a tuple of 4 u64s (WASM representation).
    pub fn from_u64s(a: u64, b: u64, c: u64, d: u64) -> Self {
        let mut bytes = [0u8; 32];
        bytes[0..8].copy_from_slice(&a.to_le_bytes());
        bytes[8..16].copy_from_slice(&b.to_le_bytes());
        bytes[16..24].copy_from_slice(&c.to_le_bytes());
        bytes[24..32].copy_from_slice(&d.to_le_bytes());
        Self(bytes)
    }

    /// Convert to a tuple of 4 u64s.
    pub fn to_u64s(&self) -> (u64, u64, u64, u64) {
        let b = &self.0;
        (
            u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]),
            u64::from_le_bytes([b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]]),
            u64::from_le_bytes([b[16], b[17], b[18], b[19], b[20], b[21], b[22], b[23]]),
            u64::from_le_bytes([b[24], b[25], b[26], b[27], b[28], b[29], b[30], b[31]]),
        )
    }

    /// Format as hex string (for display).
    pub fn to_hex(&self) -> String {
        self.0.iter().map(|b| format!("{:02x}", b)).collect()
    }

    /// Format as short hex (first 8 chars).
    pub fn to_short_hex(&self) -> String {
        self.0.iter().take(4).map(|b| format!("{:02x}", b)).collect()
    }

    /// Const function to create from bytes (for compile-time constants).
    pub const fn from_bytes_const(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

/// Placeholder for self-references and dynamic value types.
///
/// Used for:
/// - Recursive types where a type references itself
/// - The `value` type which represents any Pack value dynamically
///
/// This matches the HASH_SELF_REF constant in pack-abi for consistency.
pub const HASH_SELF_REF: TypeHash = TypeHash::from_bytes_const([
    0xff, 0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
]);

impl std::fmt::Display for TypeHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_short_hex())
    }
}

// ============================================================================
// Primitive Type Hash Constants
// ============================================================================

/// Hash for the `bool` type.
pub const HASH_BOOL: TypeHash = TypeHash([
    0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
]);

/// Hash for the `u8` type.
pub const HASH_U8: TypeHash = TypeHash([
    0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
]);

/// Hash for the `u16` type.
pub const HASH_U16: TypeHash = TypeHash([
    0x00, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
]);

/// Hash for the `u32` type.
pub const HASH_U32: TypeHash = TypeHash([
    0x00, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
]);

/// Hash for the `u64` type.
pub const HASH_U64: TypeHash = TypeHash([
    0x00, 0x05, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
]);

/// Hash for the `s8` type.
pub const HASH_S8: TypeHash = TypeHash([
    0x00, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
]);

/// Hash for the `s16` type.
pub const HASH_S16: TypeHash = TypeHash([
    0x00, 0x07, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
]);

/// Hash for the `s32` type.
pub const HASH_S32: TypeHash = TypeHash([
    0x00, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
]);

/// Hash for the `s64` type.
pub const HASH_S64: TypeHash = TypeHash([
    0x00, 0x09, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
]);

/// Hash for the `f32` type.
pub const HASH_F32: TypeHash = TypeHash([
    0x00, 0x0a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
]);

/// Hash for the `f64` type.
pub const HASH_F64: TypeHash = TypeHash([
    0x00, 0x0b, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
]);

/// Hash for the `char` type.
pub const HASH_CHAR: TypeHash = TypeHash([
    0x00, 0x0c, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
]);

/// Hash for the `string` type.
pub const HASH_STRING: TypeHash = TypeHash([
    0x00, 0x0d, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
]);

/// Hash for the `flags` type.
pub const HASH_FLAGS: TypeHash = TypeHash([
    0x00, 0x0e, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
]);

// ============================================================================
// Hash Builder and Functions
// ============================================================================

use sha2::{Digest, Sha256};

// Hash construction tags (different from CGRF wire format tags)
const HASH_TAG_LIST: u8 = 0x10;
const HASH_TAG_OPTION: u8 = 0x11;
const HASH_TAG_RESULT: u8 = 0x12;
const HASH_TAG_TUPLE: u8 = 0x13;
const HASH_TAG_RECORD: u8 = 0x14;
const HASH_TAG_VARIANT: u8 = 0x15;
const HASH_TAG_FUNCTION: u8 = 0x16;
const HASH_TAG_INTERFACE: u8 = 0x17;

/// Builder for computing type hashes.
struct TypeHasher {
    hasher: Sha256,
}

impl TypeHasher {
    fn new() -> Self {
        Self { hasher: Sha256::new() }
    }

    fn tag(mut self, tag: u8) -> Self {
        self.hasher.update([tag]);
        self
    }

    fn string(mut self, s: &str) -> Self {
        self.hasher.update((s.len() as u32).to_le_bytes());
        self.hasher.update(s.as_bytes());
        self
    }

    fn child(mut self, hash: &TypeHash) -> Self {
        self.hasher.update(hash.as_bytes());
        self
    }

    fn count(mut self, n: usize) -> Self {
        self.hasher.update((n as u32).to_le_bytes());
        self
    }

    fn finish(self) -> TypeHash {
        let result = self.hasher.finalize();
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&result);
        TypeHash(bytes)
    }
}

/// Hash a list type.
pub fn hash_list(element: &TypeHash) -> TypeHash {
    TypeHasher::new().tag(HASH_TAG_LIST).child(element).finish()
}

/// Hash an option type.
pub fn hash_option(inner: &TypeHash) -> TypeHash {
    TypeHasher::new().tag(HASH_TAG_OPTION).child(inner).finish()
}

/// Hash a result type.
pub fn hash_result(ok: &TypeHash, err: &TypeHash) -> TypeHash {
    TypeHasher::new().tag(HASH_TAG_RESULT).child(ok).child(err).finish()
}

/// Hash a tuple type.
pub fn hash_tuple(elements: &[TypeHash]) -> TypeHash {
    let mut hasher = TypeHasher::new().tag(HASH_TAG_TUPLE).count(elements.len());
    for elem in elements {
        hasher = hasher.child(elem);
    }
    hasher.finish()
}

/// Hash a record type (structural - name NOT included).
pub fn hash_record(fields: &[(&str, TypeHash)]) -> TypeHash {
    let mut hasher = TypeHasher::new().tag(HASH_TAG_RECORD).count(fields.len());
    for (name, type_hash) in fields {
        hasher = hasher.string(name).child(type_hash);
    }
    hasher.finish()
}

/// Hash a variant type (structural - name NOT included).
pub fn hash_variant(cases: &[(&str, Option<TypeHash>)]) -> TypeHash {
    let mut hasher = TypeHasher::new().tag(HASH_TAG_VARIANT).count(cases.len());
    for (name, payload) in cases {
        hasher = hasher.string(name);
        if let Some(type_hash) = payload {
            hasher = hasher.tag(1).child(type_hash);
        } else {
            hasher = hasher.tag(0);
        }
    }
    hasher.finish()
}

/// Hash a function signature.
pub fn hash_function(params: &[TypeHash], results: &[TypeHash]) -> TypeHash {
    let mut hasher = TypeHasher::new().tag(HASH_TAG_FUNCTION).count(params.len());
    for param in params {
        hasher = hasher.child(param);
    }
    hasher = hasher.count(results.len());
    for result in results {
        hasher = hasher.child(result);
    }
    hasher.finish()
}

/// An interface binding: name -> hash.
pub struct Binding<'a> {
    pub name: &'a str,
    pub hash: TypeHash,
}

/// Hash an interface (includes binding names).
pub fn hash_interface(
    name: &str,
    type_bindings: &[Binding<'_>],
    func_bindings: &[Binding<'_>],
) -> TypeHash {
    let mut hasher = TypeHasher::new()
        .tag(HASH_TAG_INTERFACE)
        .string(name)
        .count(type_bindings.len());

    for binding in type_bindings {
        hasher = hasher.string(binding.name).child(&binding.hash);
    }

    hasher = hasher.count(func_bindings.len());
    for binding in func_bindings {
        hasher = hasher.string(binding.name).child(&binding.hash);
    }

    hasher.finish()
}

/// An interface with its Merkle-tree hash.
#[derive(Debug, Clone)]
pub struct InterfaceHash {
    /// Interface name (e.g., "theater:simple/runtime").
    pub name: String,
    /// Content-addressed hash of the interface structure.
    pub hash: TypeHash,
}

// ============================================================================
// Type Hashing from Arena Types
// ============================================================================

/// Compute the TypeHash for a Type from the types module.
///
/// This is used when computing interface hashes from Arena metadata.
pub fn hash_type(ty: &Type) -> TypeHash {
    match ty {
        Type::Unit => hash_tuple(&[]), // Unit is empty tuple
        Type::Bool => HASH_BOOL,
        Type::U8 => HASH_U8,
        Type::U16 => HASH_U16,
        Type::U32 => HASH_U32,
        Type::U64 => HASH_U64,
        Type::S8 => HASH_S8,
        Type::S16 => HASH_S16,
        Type::S32 => HASH_S32,
        Type::S64 => HASH_S64,
        Type::F32 => HASH_F32,
        Type::F64 => HASH_F64,
        Type::Char => HASH_CHAR,
        Type::String => HASH_STRING,
        Type::List(inner) => hash_list(&hash_type(inner)),
        Type::Option(inner) => hash_option(&hash_type(inner)),
        Type::Result { ok, err } => hash_result(&hash_type(ok), &hash_type(err)),
        Type::Tuple(types) => {
            let hashes: Vec<_> = types.iter().map(hash_type).collect();
            hash_tuple(&hashes)
        }
        Type::Ref(path) => {
            // Named type reference - hash based on the path string
            let path_str = path.to_string();
            let mut hasher = sha2::Sha256::new();
            hasher.update(b"ref:");
            hasher.update(path_str.as_bytes());
            TypeHash::from_bytes(hasher.finalize().into())
        }
        Type::Value => {
            // Dynamic value type - use HASH_SELF_REF for consistency with pack-guest-macros
            HASH_SELF_REF
        }
    }
}

/// Compute the hash for a function from the types module.
pub fn hash_function_from_sig(func: &Function) -> TypeHash {
    let param_hashes: Vec<_> = func.params.iter().map(|p| hash_type(&p.ty)).collect();
    let result_hashes: Vec<_> = func.results.iter().map(hash_type).collect();
    hash_function(&param_hashes, &result_hashes)
}

/// Compute the interface hash for an Arena containing functions.
///
/// The Arena is treated as an interface - its name and function signatures
/// are hashed to produce a content-addressed interface hash.
pub fn compute_interface_hash(interface_arena: &Arena) -> TypeHash {
    // Create bindings for each function (sorted by name for determinism)
    let mut bindings: Vec<_> = interface_arena
        .functions
        .iter()
        .map(|f| Binding {
            name: &f.name,
            hash: hash_function_from_sig(f),
        })
        .collect();
    bindings.sort_by(|a, b| a.name.cmp(b.name));

    hash_interface(
        &interface_arena.name,
        &[], // No type bindings for now
        &bindings,
    )
}

/// Compute interface hashes for all interfaces in an Arena's imports or exports section.
///
/// Returns a list of (interface_name, interface_hash) pairs.
pub fn compute_interface_hashes(arena: &Arena, section: &str) -> Vec<InterfaceHash> {
    let mut result = Vec::new();

    for child in &arena.children {
        if child.name == section {
            // Each child of "imports" or "exports" is an interface
            for interface_arena in &child.children {
                result.push(InterfaceHash {
                    name: interface_arena.name.clone(),
                    hash: compute_interface_hash(interface_arena),
                });
            }
        }
    }

    result
}

/// Metadata with interface hashes for compatibility checking.
#[derive(Debug, Clone)]
pub struct MetadataWithHashes {
    /// The decoded arena (types, functions).
    pub arena: Arena,
    /// Hashes of imported interfaces.
    pub import_hashes: Vec<InterfaceHash>,
    /// Hashes of exported interfaces.
    pub export_hashes: Vec<InterfaceHash>,
}

/// Errors that can occur when reading metadata.
#[derive(Debug, thiserror::Error)]
pub enum MetadataError {
    #[error("package does not export __pack_types")]
    NotFound,

    #[error("metadata call failed: {0}")]
    CallFailed(String),

    #[error("failed to decode metadata: {0}")]
    DecodeFailed(String),

    #[error("invalid metadata structure: {0}")]
    InvalidStructure(String),

    #[error("failed to encode metadata: {0}")]
    EncodeFailed(String),
}

// ============================================================================
// CGRF Type Tags - Wire Format Compatibility
// ============================================================================

// These tag numbers MUST be preserved for backwards compatibility with existing
// WASM packages. The CGRF wire format uses these variant tags to encode types.
const TAG_BOOL: u32 = 0;
const TAG_U8: u32 = 1;
const TAG_U16: u32 = 2;
const TAG_U32: u32 = 3;
const TAG_U64: u32 = 4;
const TAG_S8: u32 = 5;
const TAG_S16: u32 = 6;
const TAG_S32: u32 = 7;
const TAG_S64: u32 = 8;
const TAG_F32: u32 = 9;
const TAG_F64: u32 = 10;
const TAG_CHAR: u32 = 11;
const TAG_STRING: u32 = 12;
const TAG_FLAGS: u32 = 13;
const TAG_LIST: u32 = 14;
const TAG_OPTION: u32 = 15;
const TAG_RESULT: u32 = 16;
const TAG_RECORD: u32 = 17;
const TAG_VARIANT: u32 = 18;
const TAG_TUPLE: u32 = 19;
const TAG_VALUE: u32 = 20;
const TAG_UNIT: u32 = 21;

// ============================================================================
// Metadata Decoding
// ============================================================================

/// Decode CGRF bytes into an Arena.
///
/// The metadata format is a record with "imports" and "exports" lists,
/// each containing function signatures. This is converted to an Arena
/// with two child arenas: one for imports, one for exports.
pub fn decode_metadata(bytes: &[u8]) -> Result<Arena, MetadataError> {
    let value = decode(bytes).map_err(|e| MetadataError::DecodeFailed(format!("{:?}", e)))?;

    match value {
        Value::Record { fields, .. } => {
            let mut imports = Vec::new();
            let mut exports = Vec::new();

            for (name, val) in fields {
                match name.as_str() {
                    "imports" => imports = decode_func_sig_list(val)?,
                    "exports" => exports = decode_func_sig_list(val)?,
                    _ => {}
                }
            }

            // Build an Arena with imports and exports as child arenas
            let mut arena = Arena::new("package");

            if !imports.is_empty() {
                let mut import_arena = Arena::new("imports");
                // Group functions by interface
                let mut by_interface: std::collections::HashMap<String, Vec<Function>> =
                    std::collections::HashMap::new();
                for (interface, func) in imports {
                    by_interface.entry(interface).or_default().push(func);
                }
                for (interface_name, funcs) in by_interface {
                    let mut interface_arena = Arena::new(interface_name);
                    for func in funcs {
                        interface_arena.add_function(func);
                    }
                    import_arena.add_child(interface_arena);
                }
                arena.add_child(import_arena);
            }

            if !exports.is_empty() {
                let mut export_arena = Arena::new("exports");
                // Group functions by interface
                let mut by_interface: std::collections::HashMap<String, Vec<Function>> =
                    std::collections::HashMap::new();
                for (interface, func) in exports {
                    by_interface.entry(interface).or_default().push(func);
                }
                for (interface_name, funcs) in by_interface {
                    let mut interface_arena = Arena::new(interface_name);
                    for func in funcs {
                        interface_arena.add_function(func);
                    }
                    export_arena.add_child(interface_arena);
                }
                arena.add_child(export_arena);
            }

            Ok(arena)
        }
        _ => Err(MetadataError::InvalidStructure(
            "expected record at top level".into(),
        )),
    }
}

/// Decode CGRF bytes into metadata with interface hashes.
///
/// This is the preferred decoding function as it includes Merkle-tree hashes
/// for O(1) interface compatibility checking.
pub fn decode_metadata_with_hashes(bytes: &[u8]) -> Result<MetadataWithHashes, MetadataError> {
    let value = decode(bytes).map_err(|e| MetadataError::DecodeFailed(format!("{:?}", e)))?;

    match value {
        Value::Record { fields, .. } => {
            let mut imports = Vec::new();
            let mut exports = Vec::new();
            let mut import_hashes = Vec::new();
            let mut export_hashes = Vec::new();

            for (name, val) in fields {
                match name.as_str() {
                    "imports" => imports = decode_func_sig_list(val)?,
                    "exports" => exports = decode_func_sig_list(val)?,
                    "import-hashes" => import_hashes = decode_interface_hash_list(val)?,
                    "export-hashes" => export_hashes = decode_interface_hash_list(val)?,
                    _ => {}
                }
            }

            // Build the arena (same as decode_metadata)
            let mut arena = Arena::new("package");

            if !imports.is_empty() {
                let mut import_arena = Arena::new("imports");
                let mut by_interface: std::collections::HashMap<String, Vec<Function>> =
                    std::collections::HashMap::new();
                for (interface, func) in imports {
                    by_interface.entry(interface).or_default().push(func);
                }
                for (interface_name, funcs) in by_interface {
                    let mut interface_arena = Arena::new(interface_name);
                    for func in funcs {
                        interface_arena.add_function(func);
                    }
                    import_arena.add_child(interface_arena);
                }
                arena.add_child(import_arena);
            }

            if !exports.is_empty() {
                let mut export_arena = Arena::new("exports");
                let mut by_interface: std::collections::HashMap<String, Vec<Function>> =
                    std::collections::HashMap::new();
                for (interface, func) in exports {
                    by_interface.entry(interface).or_default().push(func);
                }
                for (interface_name, funcs) in by_interface {
                    let mut interface_arena = Arena::new(interface_name);
                    for func in funcs {
                        interface_arena.add_function(func);
                    }
                    export_arena.add_child(interface_arena);
                }
                arena.add_child(export_arena);
            }

            Ok(MetadataWithHashes {
                arena,
                import_hashes,
                export_hashes,
            })
        }
        _ => Err(MetadataError::InvalidStructure(
            "expected record at top level".into(),
        )),
    }
}

/// Decode a list of interface hashes.
fn decode_interface_hash_list(value: Value) -> Result<Vec<InterfaceHash>, MetadataError> {
    match value {
        Value::List { items, .. } => items.into_iter().map(decode_interface_hash).collect(),
        _ => Err(MetadataError::InvalidStructure(
            "expected list of interface hashes".into(),
        )),
    }
}

/// Decode a single interface hash.
fn decode_interface_hash(value: Value) -> Result<InterfaceHash, MetadataError> {
    match value {
        Value::Record { fields, .. } => {
            let mut name = String::new();
            let mut hash = TypeHash::from_bytes([0u8; 32]);

            for (field_name, val) in fields {
                match field_name.as_str() {
                    "name" => {
                        if let Value::String(s) = val {
                            name = s;
                        }
                    }
                    "hash" => {
                        // Hash can be stored as list<u8> or tuple<u64, u64, u64, u64>
                        match val {
                            Value::List { items, .. } => {
                                // List of u8 bytes
                                let bytes: Vec<u8> = items
                                    .into_iter()
                                    .filter_map(|v| match v {
                                        Value::U8(b) => Some(b),
                                        _ => None,
                                    })
                                    .collect();
                                if bytes.len() == 32 {
                                    let mut arr = [0u8; 32];
                                    arr.copy_from_slice(&bytes);
                                    hash = TypeHash::from_bytes(arr);
                                }
                            }
                            Value::Tuple(parts) => {
                                // Legacy: tuple of 4 u64s
                                if parts.len() == 4 {
                                    let a = match &parts[0] { Value::U64(v) => *v, _ => 0 };
                                    let b = match &parts[1] { Value::U64(v) => *v, _ => 0 };
                                    let c = match &parts[2] { Value::U64(v) => *v, _ => 0 };
                                    let d = match &parts[3] { Value::U64(v) => *v, _ => 0 };
                                    hash = TypeHash::from_u64s(a, b, c, d);
                                }
                            }
                            _ => {}
                        }
                    }
                    _ => {}
                }
            }

            Ok(InterfaceHash { name, hash })
        }
        _ => Err(MetadataError::InvalidStructure(
            "expected record for interface hash".into(),
        )),
    }
}

/// Decode a list of function signatures.
/// Returns (interface_name, Function) pairs.
fn decode_func_sig_list(value: Value) -> Result<Vec<(String, Function)>, MetadataError> {
    match value {
        Value::List { items, .. } => items.into_iter().map(decode_func_sig).collect(),
        _ => Err(MetadataError::InvalidStructure(
            "expected list of function signatures".into(),
        )),
    }
}

/// Decode a function signature.
/// Returns (interface_name, Function).
fn decode_func_sig(value: Value) -> Result<(String, Function), MetadataError> {
    match value {
        Value::Record { fields, .. } => {
            let mut interface = String::new();
            let mut name = String::new();
            let mut params = Vec::new();
            let mut results = Vec::new();

            for (field_name, val) in fields {
                match field_name.as_str() {
                    "interface" => {
                        if let Value::String(s) = val {
                            interface = s;
                        }
                    }
                    "name" => {
                        if let Value::String(s) = val {
                            name = s;
                        }
                    }
                    "params" => {
                        params = decode_param_list(val)?;
                    }
                    "results" => {
                        results = decode_type_list(val)?;
                    }
                    _ => {}
                }
            }

            Ok((interface, Function::with_signature(name, params, results)))
        }
        _ => Err(MetadataError::InvalidStructure(
            "expected record for function signature".into(),
        )),
    }
}

fn decode_param_list(value: Value) -> Result<Vec<Param>, MetadataError> {
    match value {
        Value::List { items, .. } => items.into_iter().map(decode_param).collect(),
        _ => Err(MetadataError::InvalidStructure(
            "expected list of parameters".into(),
        )),
    }
}

fn decode_param(value: Value) -> Result<Param, MetadataError> {
    match value {
        Value::Record { fields, .. } => {
            let mut name = String::new();
            let mut ty = Type::Value;

            for (field_name, val) in fields {
                match field_name.as_str() {
                    "name" => {
                        if let Value::String(s) = val {
                            name = s;
                        }
                    }
                    "type" => {
                        ty = decode_type(val)?;
                    }
                    _ => {}
                }
            }

            Ok(Param::new(name, ty))
        }
        _ => Err(MetadataError::InvalidStructure(
            "expected record for parameter".into(),
        )),
    }
}

fn decode_type_list(value: Value) -> Result<Vec<Type>, MetadataError> {
    match value {
        Value::List { items, .. } => items.into_iter().map(decode_type).collect(),
        _ => Err(MetadataError::InvalidStructure(
            "expected list of types".into(),
        )),
    }
}

fn decode_type(value: Value) -> Result<Type, MetadataError> {
    match value {
        Value::Variant { tag, payload, .. } => {
            let tag = tag as u32;
            match tag {
                TAG_BOOL => Ok(Type::Bool),
                TAG_U8 => Ok(Type::U8),
                TAG_U16 => Ok(Type::U16),
                TAG_U32 => Ok(Type::U32),
                TAG_U64 => Ok(Type::U64),
                TAG_S8 => Ok(Type::S8),
                TAG_S16 => Ok(Type::S16),
                TAG_S32 => Ok(Type::S32),
                TAG_S64 => Ok(Type::S64),
                TAG_F32 => Ok(Type::F32),
                TAG_F64 => Ok(Type::F64),
                TAG_CHAR => Ok(Type::Char),
                TAG_STRING => Ok(Type::String),
                TAG_FLAGS => Ok(Type::Ref(TypePath::simple("flags"))),
                TAG_LIST => {
                    let inner = payload.into_iter().next().ok_or_else(|| {
                        MetadataError::InvalidStructure("list missing element type".into())
                    })?;
                    Ok(Type::list(decode_type(inner)?))
                }
                TAG_OPTION => {
                    let inner = payload.into_iter().next().ok_or_else(|| {
                        MetadataError::InvalidStructure("option missing inner type".into())
                    })?;
                    Ok(Type::option(decode_type(inner)?))
                }
                TAG_RESULT => {
                    let record = payload.into_iter().next().ok_or_else(|| {
                        MetadataError::InvalidStructure("result missing payload".into())
                    })?;
                    match record {
                        Value::Record { fields, .. } => {
                            let mut ok = Type::Unit;
                            let mut err = Type::Unit;
                            for (name, val) in fields {
                                match name.as_str() {
                                    "ok" => ok = decode_type(val)?,
                                    "err" => err = decode_type(val)?,
                                    _ => {}
                                }
                            }
                            Ok(Type::result(ok, err))
                        }
                        _ => Err(MetadataError::InvalidStructure(
                            "result payload not a record".into(),
                        )),
                    }
                }
                TAG_RECORD => {
                    let record = payload.into_iter().next().ok_or_else(|| {
                        MetadataError::InvalidStructure("record missing payload".into())
                    })?;
                    decode_record_type(record)
                }
                TAG_VARIANT => {
                    let record = payload.into_iter().next().ok_or_else(|| {
                        MetadataError::InvalidStructure("variant missing payload".into())
                    })?;
                    decode_variant_type(record)
                }
                TAG_TUPLE => {
                    let list = payload.into_iter().next().ok_or_else(|| {
                        MetadataError::InvalidStructure("tuple missing payload".into())
                    })?;
                    match list {
                        Value::List { items, .. } => {
                            let types: Result<Vec<_>, _> =
                                items.into_iter().map(decode_type).collect();
                            Ok(Type::tuple(types?))
                        }
                        _ => Err(MetadataError::InvalidStructure(
                            "tuple payload not a list".into(),
                        )),
                    }
                }
                TAG_VALUE => Ok(Type::Value),
                TAG_UNIT => Ok(Type::Unit),
                _ => Err(MetadataError::InvalidStructure(format!(
                    "unknown type tag: {}",
                    tag
                ))),
            }
        }
        _ => Err(MetadataError::InvalidStructure(
            "expected variant for type".into(),
        )),
    }
}

fn decode_record_type(value: Value) -> Result<Type, MetadataError> {
    match value {
        Value::Record {
            fields: rec_fields,
            ..
        } => {
            let mut name = String::new();
            for (fname, val) in rec_fields {
                if fname == "name" {
                    if let Value::String(s) = val {
                        name = s;
                    }
                }
            }
            // Records are represented as named type references
            Ok(Type::Ref(TypePath::simple(name)))
        }
        _ => Err(MetadataError::InvalidStructure(
            "record payload not a record".into(),
        )),
    }
}

fn decode_variant_type(value: Value) -> Result<Type, MetadataError> {
    match value {
        Value::Record {
            fields: rec_fields,
            ..
        } => {
            let mut name = String::new();
            for (fname, val) in rec_fields {
                if fname == "name" {
                    if let Value::String(s) = val {
                        name = s;
                    }
                }
            }
            // Variants are represented as named type references
            Ok(Type::Ref(TypePath::simple(name)))
        }
        _ => Err(MetadataError::InvalidStructure(
            "variant payload not a record".into(),
        )),
    }
}

// ============================================================================
// Metadata Encoding
// ============================================================================

/// Encode an Arena to CGRF bytes (metadata format).
///
/// The Arena is encoded as a record with "imports" and "exports" lists.
/// Child arenas named "imports" and "exports" are used, with their children
/// as interfaces containing functions.
pub fn encode_metadata(arena: &Arena) -> Result<Vec<u8>, MetadataError> {
    let mut imports = Vec::new();
    let mut exports = Vec::new();

    for child in &arena.children {
        match child.name.as_str() {
            "imports" => {
                for interface in &child.children {
                    for func in &interface.functions {
                        imports.push(encode_func_sig_value(&interface.name, func));
                    }
                }
            }
            "exports" => {
                for interface in &child.children {
                    for func in &interface.functions {
                        exports.push(encode_func_sig_value(&interface.name, func));
                    }
                }
            }
            _ => {}
        }
    }

    let record = Value::Record {
        type_name: "PackageMetadata".to_string(),
        fields: vec![
            (
                "imports".to_string(),
                Value::List {
                    elem_type: crate::abi::ValueType::Record("FunctionSignature".to_string()),
                    items: imports,
                },
            ),
            (
                "exports".to_string(),
                Value::List {
                    elem_type: crate::abi::ValueType::Record("FunctionSignature".to_string()),
                    items: exports,
                },
            ),
        ],
    };

    encode(&record).map_err(|e| MetadataError::EncodeFailed(format!("{:?}", e)))
}

fn encode_func_sig_value(interface: &str, func: &Function) -> Value {
    Value::Record {
        type_name: "FunctionSignature".to_string(),
        fields: vec![
            ("interface".to_string(), Value::String(interface.to_string())),
            ("name".to_string(), Value::String(func.name.clone())),
            (
                "params".to_string(),
                Value::List {
                    elem_type: crate::abi::ValueType::Record("ParamSignature".to_string()),
                    items: func.params.iter().map(encode_param_value).collect(),
                },
            ),
            (
                "results".to_string(),
                Value::List {
                    elem_type: crate::abi::ValueType::Variant("Type".to_string()),
                    items: func.results.iter().map(encode_type_value).collect(),
                },
            ),
        ],
    }
}

fn encode_param_value(param: &Param) -> Value {
    Value::Record {
        type_name: "ParamSignature".to_string(),
        fields: vec![
            ("name".to_string(), Value::String(param.name.clone())),
            ("type".to_string(), encode_type_value(&param.ty)),
        ],
    }
}

fn encode_type_value(ty: &Type) -> Value {
    let (tag, payload) = match ty {
        Type::Unit => (TAG_UNIT as usize, vec![]),
        Type::Bool => (TAG_BOOL as usize, vec![]),
        Type::U8 => (TAG_U8 as usize, vec![]),
        Type::U16 => (TAG_U16 as usize, vec![]),
        Type::U32 => (TAG_U32 as usize, vec![]),
        Type::U64 => (TAG_U64 as usize, vec![]),
        Type::S8 => (TAG_S8 as usize, vec![]),
        Type::S16 => (TAG_S16 as usize, vec![]),
        Type::S32 => (TAG_S32 as usize, vec![]),
        Type::S64 => (TAG_S64 as usize, vec![]),
        Type::F32 => (TAG_F32 as usize, vec![]),
        Type::F64 => (TAG_F64 as usize, vec![]),
        Type::Char => (TAG_CHAR as usize, vec![]),
        Type::String => (TAG_STRING as usize, vec![]),
        Type::List(inner) => (TAG_LIST as usize, vec![encode_type_value(inner)]),
        Type::Option(inner) => (TAG_OPTION as usize, vec![encode_type_value(inner)]),
        Type::Result { ok, err } => (
            TAG_RESULT as usize,
            vec![Value::Record {
                type_name: "ResultPayload".to_string(),
                fields: vec![
                    ("ok".to_string(), encode_type_value(ok)),
                    ("err".to_string(), encode_type_value(err)),
                ],
            }],
        ),
        Type::Tuple(types) => (
            TAG_TUPLE as usize,
            vec![Value::List {
                elem_type: crate::abi::ValueType::Variant("Type".to_string()),
                items: types.iter().map(encode_type_value).collect(),
            }],
        ),
        Type::Ref(path) => {
            let name = path.segments.join("::");
            (
                TAG_VARIANT as usize,
                vec![Value::Record {
                    type_name: "TypeRef".to_string(),
                    fields: vec![("name".to_string(), Value::String(name))],
                }],
            )
        }
        Type::Value => (TAG_VALUE as usize, vec![]),
    };

    Value::Variant {
        type_name: "Type".to_string(),
        case_name: format!("tag{}", tag),
        tag,
        payload,
    }
}

// ============================================================================
// Legacy Type Aliases (for backward compatibility during migration)
// ============================================================================

/// Legacy type alias for backward compatibility.
/// Use `crate::types::Type` instead.
pub type TypeDesc = Type;

/// Legacy type alias for backward compatibility.
/// Use `crate::types::Field` instead.
pub type FieldDesc = Field;

/// Legacy type alias for backward compatibility.
/// Use `crate::types::Case` instead.
pub type CaseDesc = Case;

/// Legacy type alias for backward compatibility.
/// Use `crate::types::Param` instead.
pub type ParamSignature = Param;

/// Legacy type alias for backward compatibility.
/// Use `crate::types::Function` instead.
pub type FunctionSignature = Function;

/// Legacy type alias for backward compatibility.
/// Use `crate::types::Arena` instead.
pub type PackageMetadata = Arena;

// ============================================================================
// Metadata Encoding with Hashes
// ============================================================================

/// Encode an Arena to CGRF bytes with interface hashes included.
///
/// This is the preferred encoding function as it includes Merkle-tree hashes
/// for O(1) interface compatibility checking at runtime.
///
/// The output format includes:
/// - `imports`: List of function signatures
/// - `exports`: List of function signatures
/// - `import-hashes`: List of (interface_name, hash) pairs
/// - `export-hashes`: List of (interface_name, hash) pairs
pub fn encode_metadata_with_hashes(arena: &Arena) -> Result<Vec<u8>, MetadataError> {
    let mut imports = Vec::new();
    let mut exports = Vec::new();

    // Collect function signatures (same as encode_metadata)
    for child in &arena.children {
        match child.name.as_str() {
            "imports" => {
                for interface in &child.children {
                    for func in &interface.functions {
                        imports.push(encode_func_sig_value(&interface.name, func));
                    }
                }
            }
            "exports" => {
                for interface in &child.children {
                    for func in &interface.functions {
                        exports.push(encode_func_sig_value(&interface.name, func));
                    }
                }
            }
            _ => {}
        }
    }

    // Compute interface hashes
    let import_hashes = compute_interface_hashes(arena, "imports");
    let export_hashes = compute_interface_hashes(arena, "exports");

    let record = Value::Record {
        type_name: "PackageMetadata".to_string(),
        fields: vec![
            (
                "imports".to_string(),
                Value::List {
                    elem_type: crate::abi::ValueType::Record("FunctionSignature".to_string()),
                    items: imports,
                },
            ),
            (
                "exports".to_string(),
                Value::List {
                    elem_type: crate::abi::ValueType::Record("FunctionSignature".to_string()),
                    items: exports,
                },
            ),
            (
                "import-hashes".to_string(),
                Value::List {
                    elem_type: crate::abi::ValueType::Record("InterfaceHash".to_string()),
                    items: import_hashes.iter().map(encode_interface_hash_value).collect(),
                },
            ),
            (
                "export-hashes".to_string(),
                Value::List {
                    elem_type: crate::abi::ValueType::Record("InterfaceHash".to_string()),
                    items: export_hashes.iter().map(encode_interface_hash_value).collect(),
                },
            ),
        ],
    };

    encode(&record).map_err(|e| MetadataError::EncodeFailed(format!("{:?}", e)))
}

/// Encode an InterfaceHash as a CGRF Value.
fn encode_interface_hash_value(ih: &InterfaceHash) -> Value {
    Value::Record {
        type_name: "InterfaceHash".to_string(),
        fields: vec![
            ("name".to_string(), Value::String(ih.name.clone())),
            (
                "hash".to_string(),
                Value::List {
                    elem_type: crate::abi::ValueType::U8,
                    items: ih.hash.as_bytes().iter().map(|&b| Value::U8(b)).collect(),
                },
            ),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_type_tags_preserved() {
        // Verify tag constants match expected values for wire format compatibility
        assert_eq!(TAG_BOOL, 0);
        assert_eq!(TAG_U8, 1);
        assert_eq!(TAG_STRING, 12);
        assert_eq!(TAG_VALUE, 20);
        assert_eq!(TAG_UNIT, 21);
    }

    #[test]
    fn test_hash_type_primitives() {
        assert_eq!(hash_type(&Type::Bool), HASH_BOOL);
        assert_eq!(hash_type(&Type::String), HASH_STRING);
        assert_eq!(hash_type(&Type::S32), HASH_S32);
        assert_eq!(hash_type(&Type::U8), HASH_U8);
    }

    #[test]
    fn test_hash_type_compound() {
        let list_hash = hash_type(&Type::List(Box::new(Type::U8)));
        let expected = hash_list(&HASH_U8);
        assert_eq!(list_hash, expected);

        let option_hash = hash_type(&Type::Option(Box::new(Type::String)));
        let expected = hash_option(&HASH_STRING);
        assert_eq!(option_hash, expected);

        let result_hash = hash_type(&Type::Result {
            ok: Box::new(Type::String),
            err: Box::new(Type::String),
        });
        let expected = hash_result(&HASH_STRING, &HASH_STRING);
        assert_eq!(result_hash, expected);
    }

    #[test]
    fn test_compute_interface_hash() {
        // Create a simple interface arena with two functions
        let mut interface = Arena::new("test:example/api");
        interface.add_function(Function::with_signature(
            "greet",
            vec![Param::new("name", Type::String)],
            vec![Type::String],
        ));
        interface.add_function(Function::with_signature(
            "add",
            vec![Param::new("a", Type::S32), Param::new("b", Type::S32)],
            vec![Type::S32],
        ));

        let hash1 = compute_interface_hash(&interface);

        // Same interface should produce same hash
        let hash2 = compute_interface_hash(&interface);
        assert_eq!(hash1, hash2);

        // Different function order should produce same hash (sorted by name)
        let mut interface_reordered = Arena::new("test:example/api");
        interface_reordered.add_function(Function::with_signature(
            "add",
            vec![Param::new("a", Type::S32), Param::new("b", Type::S32)],
            vec![Type::S32],
        ));
        interface_reordered.add_function(Function::with_signature(
            "greet",
            vec![Param::new("name", Type::String)],
            vec![Type::String],
        ));
        let hash3 = compute_interface_hash(&interface_reordered);
        assert_eq!(hash1, hash3);
    }

    #[test]
    fn test_compute_interface_hash_differs_on_signature() {
        let mut interface1 = Arena::new("test:api");
        interface1.add_function(Function::with_signature(
            "foo",
            vec![Param::new("x", Type::S32)],
            vec![Type::S32],
        ));

        let mut interface2 = Arena::new("test:api");
        interface2.add_function(Function::with_signature(
            "foo",
            vec![Param::new("x", Type::S64)], // Different type!
            vec![Type::S64],
        ));

        assert_ne!(
            compute_interface_hash(&interface1),
            compute_interface_hash(&interface2)
        );
    }

    #[test]
    fn test_encode_metadata_with_hashes_roundtrip() {
        // Build an arena with imports and exports
        let mut arena = Arena::new("package");

        let mut imports = Arena::new("imports");
        let mut runtime_interface = Arena::new("theater:simple/runtime");
        runtime_interface.add_function(Function::with_signature(
            "log",
            vec![Param::new("msg", Type::String)],
            vec![],
        ));
        imports.add_child(runtime_interface);
        arena.add_child(imports);

        let mut exports = Arena::new("exports");
        let mut actor_interface = Arena::new("theater:simple/actor");
        actor_interface.add_function(Function::with_signature(
            "init",
            vec![Param::new("data", Type::List(Box::new(Type::U8)))],
            vec![Type::List(Box::new(Type::U8))],
        ));
        exports.add_child(actor_interface);
        arena.add_child(exports);

        // Encode with hashes
        let bytes = encode_metadata_with_hashes(&arena).expect("encoding failed");

        // Decode and verify hashes are present
        let decoded = decode_metadata_with_hashes(&bytes).expect("decoding failed");

        assert_eq!(decoded.import_hashes.len(), 1);
        assert_eq!(decoded.import_hashes[0].name, "theater:simple/runtime");

        assert_eq!(decoded.export_hashes.len(), 1);
        assert_eq!(decoded.export_hashes[0].name, "theater:simple/actor");

        // Verify hashes are non-zero
        assert!(!decoded.import_hashes[0].hash.as_bytes().iter().all(|&b| b == 0));
        assert!(!decoded.export_hashes[0].hash.as_bytes().iter().all(|&b| b == 0));
    }
}
