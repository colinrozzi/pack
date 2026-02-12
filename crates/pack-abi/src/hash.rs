//! Merkle-tree hashing for Pack types.
//!
//! Types are hashed structurally - field names are included, type names are not.
//! Names are bindings at the interface level, not part of the type's identity.
//!
//! # Design
//!
//! - Primitives have fixed hashes (constants)
//! - Compound types hash their components: `hash(list<T>) = hash("list", hash(T))`
//! - Records hash their fields: `hash({ x: s32, y: s32 }) = hash("record", [("x", hash(s32)), ("y", hash(s32))])`
//! - Type names are NOT included - `Point` and `Vec2` with same structure have same hash
//! - Interface bindings include names: `("Point", type_hash)`
//!
//! This creates a Merkle tree where:
//! - Structural sharing is natural (same structure = same hash)
//! - Type compatibility is O(1) hash comparison
//! - Interfaces can bind different names to the same type hash

use sha2::{Digest, Sha256};

#[cfg(feature = "std")]
use alloc::string::String;

/// A 256-bit type hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TypeHash([u8; 32]);

impl TypeHash {
    /// Create a TypeHash from raw bytes.
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Get the raw bytes.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Convert to a tuple of 4 u64s (for WASM-friendly representation).
    pub fn to_u64s(&self) -> (u64, u64, u64, u64) {
        let b = &self.0;
        (
            u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]),
            u64::from_le_bytes([b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]]),
            u64::from_le_bytes([b[16], b[17], b[18], b[19], b[20], b[21], b[22], b[23]]),
            u64::from_le_bytes([b[24], b[25], b[26], b[27], b[28], b[29], b[30], b[31]]),
        )
    }

    /// Create from a tuple of 4 u64s.
    pub fn from_u64s(a: u64, b: u64, c: u64, d: u64) -> Self {
        let mut bytes = [0u8; 32];
        bytes[0..8].copy_from_slice(&a.to_le_bytes());
        bytes[8..16].copy_from_slice(&b.to_le_bytes());
        bytes[16..24].copy_from_slice(&c.to_le_bytes());
        bytes[24..32].copy_from_slice(&d.to_le_bytes());
        Self(bytes)
    }

    /// Format as hex string (for debugging).
    #[cfg(feature = "std")]
    pub fn to_hex(&self) -> String {
        use alloc::format;
        self.0.iter().map(|b| format!("{:02x}", b)).collect()
    }
}

// ============================================================================
// Primitive Type Hashes (fixed constants)
// ============================================================================

// These are computed as SHA-256 of the type name, but we store them as constants
// for efficiency. The actual values don't matter as long as they're unique.

/// Hash for the `bool` type.
pub const HASH_BOOL: TypeHash = TypeHash::from_bytes([
    0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
]);

/// Hash for the `u8` type.
pub const HASH_U8: TypeHash = TypeHash::from_bytes([
    0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
]);

/// Hash for the `u16` type.
pub const HASH_U16: TypeHash = TypeHash::from_bytes([
    0x00, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
]);

/// Hash for the `u32` type.
pub const HASH_U32: TypeHash = TypeHash::from_bytes([
    0x00, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
]);

/// Hash for the `u64` type.
pub const HASH_U64: TypeHash = TypeHash::from_bytes([
    0x00, 0x05, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
]);

/// Hash for the `s8` type.
pub const HASH_S8: TypeHash = TypeHash::from_bytes([
    0x00, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
]);

/// Hash for the `s16` type.
pub const HASH_S16: TypeHash = TypeHash::from_bytes([
    0x00, 0x07, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
]);

/// Hash for the `s32` type.
pub const HASH_S32: TypeHash = TypeHash::from_bytes([
    0x00, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
]);

/// Hash for the `s64` type.
pub const HASH_S64: TypeHash = TypeHash::from_bytes([
    0x00, 0x09, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
]);

/// Hash for the `f32` type.
pub const HASH_F32: TypeHash = TypeHash::from_bytes([
    0x00, 0x0a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
]);

/// Hash for the `f64` type.
pub const HASH_F64: TypeHash = TypeHash::from_bytes([
    0x00, 0x0b, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
]);

/// Hash for the `char` type.
pub const HASH_CHAR: TypeHash = TypeHash::from_bytes([
    0x00, 0x0c, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
]);

/// Hash for the `string` type.
pub const HASH_STRING: TypeHash = TypeHash::from_bytes([
    0x00, 0x0d, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
]);

/// Hash for the `flags` type.
pub const HASH_FLAGS: TypeHash = TypeHash::from_bytes([
    0x00, 0x0e, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
]);

// ============================================================================
// Hash Builder
// ============================================================================

/// Builder for computing type hashes.
pub struct TypeHasher {
    hasher: Sha256,
}

impl TypeHasher {
    /// Create a new hasher.
    pub fn new() -> Self {
        Self {
            hasher: Sha256::new(),
        }
    }

    /// Add a tag byte (identifies the kind of construct being hashed).
    pub fn tag(mut self, tag: u8) -> Self {
        self.hasher.update([tag]);
        self
    }

    /// Add a string (length-prefixed).
    pub fn string(mut self, s: &str) -> Self {
        self.hasher.update((s.len() as u32).to_le_bytes());
        self.hasher.update(s.as_bytes());
        self
    }

    /// Add a child type hash.
    pub fn child(mut self, hash: &TypeHash) -> Self {
        self.hasher.update(hash.as_bytes());
        self
    }

    /// Add a count (for lists of children).
    pub fn count(mut self, n: usize) -> Self {
        self.hasher.update((n as u32).to_le_bytes());
        self
    }

    /// Finalize and return the hash.
    pub fn finish(self) -> TypeHash {
        let result = self.hasher.finalize();
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&result);
        TypeHash(bytes)
    }
}

impl Default for TypeHasher {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Hash Tags (identifies type constructors)
// ============================================================================

const TAG_LIST: u8 = 0x10;
const TAG_OPTION: u8 = 0x11;
const TAG_RESULT: u8 = 0x12;
const TAG_TUPLE: u8 = 0x13;
const TAG_RECORD: u8 = 0x14;
const TAG_VARIANT: u8 = 0x15;
const TAG_FUNCTION: u8 = 0x16;
const TAG_INTERFACE: u8 = 0x17;

// ============================================================================
// Compound Type Hashing
// ============================================================================

/// Hash a list type: `list<T>`.
pub fn hash_list(element: &TypeHash) -> TypeHash {
    TypeHasher::new()
        .tag(TAG_LIST)
        .child(element)
        .finish()
}

/// Hash an option type: `option<T>`.
pub fn hash_option(inner: &TypeHash) -> TypeHash {
    TypeHasher::new()
        .tag(TAG_OPTION)
        .child(inner)
        .finish()
}

/// Hash a result type: `result<T, E>`.
pub fn hash_result(ok: &TypeHash, err: &TypeHash) -> TypeHash {
    TypeHasher::new()
        .tag(TAG_RESULT)
        .child(ok)
        .child(err)
        .finish()
}

/// Hash a tuple type: `tuple<T1, T2, ...>`.
pub fn hash_tuple(elements: &[TypeHash]) -> TypeHash {
    let mut hasher = TypeHasher::new()
        .tag(TAG_TUPLE)
        .count(elements.len());

    for elem in elements {
        hasher = hasher.child(elem);
    }

    hasher.finish()
}

/// Hash a record type (structural - name NOT included).
/// Fields should be in canonical order (sorted by name).
pub fn hash_record(fields: &[(&str, TypeHash)]) -> TypeHash {
    let mut hasher = TypeHasher::new()
        .tag(TAG_RECORD)
        .count(fields.len());

    for (name, type_hash) in fields {
        hasher = hasher.string(name).child(type_hash);
    }

    hasher.finish()
}

/// Hash a variant type (structural - name NOT included).
/// Cases should be in canonical order (sorted by name).
pub fn hash_variant(cases: &[(&str, Option<TypeHash>)]) -> TypeHash {
    let mut hasher = TypeHasher::new()
        .tag(TAG_VARIANT)
        .count(cases.len());

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
/// Param names are NOT included (just types in order).
/// Result types are included in order.
pub fn hash_function(params: &[TypeHash], results: &[TypeHash]) -> TypeHash {
    let mut hasher = TypeHasher::new()
        .tag(TAG_FUNCTION)
        .count(params.len());

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
/// Bindings should be in canonical order (sorted by name).
pub fn hash_interface(
    name: &str,
    type_bindings: &[Binding<'_>],
    func_bindings: &[Binding<'_>],
) -> TypeHash {
    let mut hasher = TypeHasher::new()
        .tag(TAG_INTERFACE)
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

// ============================================================================
// Recursive Type Hashing
// ============================================================================

/// Placeholder for self-references in recursive types.
///
/// When hashing `variant sexpr { lst(list<sexpr>) }`:
/// 1. First pass: use HASH_SELF_REF for the recursive reference
/// 2. This produces a "template hash"
/// 3. The template hash IS the final hash (self-reference is structural)
pub const HASH_SELF_REF: TypeHash = TypeHash::from_bytes([
    0xff, 0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
]);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_primitive_hashes_are_unique() {
        let primitives = [
            HASH_BOOL, HASH_U8, HASH_U16, HASH_U32, HASH_U64,
            HASH_S8, HASH_S16, HASH_S32, HASH_S64,
            HASH_F32, HASH_F64, HASH_CHAR, HASH_STRING, HASH_FLAGS,
        ];

        for (i, a) in primitives.iter().enumerate() {
            for (j, b) in primitives.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b, "primitive hashes must be unique");
                }
            }
        }
    }

    #[test]
    fn test_list_hash() {
        let list_s32 = hash_list(&HASH_S32);
        let list_s64 = hash_list(&HASH_S64);

        assert_ne!(list_s32, list_s64);
        assert_ne!(list_s32, HASH_S32); // list<s32> != s32
    }

    #[test]
    fn test_record_hash_is_structural() {
        // Same structure, would have different names in source
        let point = hash_record(&[("x", HASH_S32), ("y", HASH_S32)]);
        let vec2 = hash_record(&[("x", HASH_S32), ("y", HASH_S32)]);

        assert_eq!(point, vec2, "same structure = same hash");
    }

    #[test]
    fn test_record_hash_includes_field_names() {
        let xy = hash_record(&[("x", HASH_S32), ("y", HASH_S32)]);
        let ab = hash_record(&[("a", HASH_S32), ("b", HASH_S32)]);

        assert_ne!(xy, ab, "different field names = different hash");
    }

    #[test]
    fn test_tuple_vs_record() {
        let tuple = hash_tuple(&[HASH_S32, HASH_S32]);
        let record = hash_record(&[("x", HASH_S32), ("y", HASH_S32)]);

        assert_ne!(tuple, record, "tuple != record");
    }

    #[test]
    fn test_function_hash() {
        let add = hash_function(&[HASH_S32, HASH_S32], &[HASH_S32]);
        let sub = hash_function(&[HASH_S32, HASH_S32], &[HASH_S32]);

        // Same signature = same hash (function names are bindings, not part of hash)
        assert_eq!(add, sub);
    }

    #[test]
    fn test_interface_includes_names() {
        let iface_a = hash_interface(
            "math",
            &[],
            &[Binding { name: "add", hash: hash_function(&[HASH_S32], &[HASH_S32]) }],
        );

        let iface_b = hash_interface(
            "math",
            &[],
            &[Binding { name: "inc", hash: hash_function(&[HASH_S32], &[HASH_S32]) }],
        );

        // Different binding names = different interface hash
        assert_ne!(iface_a, iface_b);
    }

    #[test]
    fn test_u64_roundtrip() {
        let hash = hash_list(&HASH_S32);
        let (a, b, c, d) = hash.to_u64s();
        let back = TypeHash::from_u64s(a, b, c, d);

        assert_eq!(hash, back);
    }
}
