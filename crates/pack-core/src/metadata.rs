//! Read-side package metadata: CGRF decoding + interface hashing.
//!
//! Packages embed CGRF-encoded metadata in a wasm data segment (prefixed with
//! [`CGRF_MAGIC`]). This module statically extracts and decodes that metadata
//! into the pact AST ([`packr_abi::types`]), and computes content-addressed
//! Merkle-tree interface hashes for O(1) compatibility checking.
//!
//! This is the **read side only** — the structural hash primitives themselves
//! (`hash_list`, `TypeHash`, the `HASH_*` constants, …) live in `packr-abi`
//! and are used here; the encoders and value/type-space validation from the
//! umbrella crate are intentionally NOT ported.

use packr_abi::types::{Arena, Case, Field, Function, Param, Type, TypeDef, TypeParam, TypePath};
use packr_abi::{
    decode_prefix, hash_function, hash_interface, hash_list, hash_map, hash_option, hash_record,
    hash_result, hash_set, hash_tuple, hash_variant, Binding, TypeHash, Value, HASH_BOOL,
    HASH_CHAR, HASH_F32, HASH_F64, HASH_FLAGS, HASH_S16, HASH_S32, HASH_S64, HASH_S8,
    HASH_SELF_REF, HASH_STRING, HASH_U16, HASH_U32, HASH_U64, HASH_U8,
};

use sha2::{Digest, Sha256};
use std::collections::HashMap;

// ============================================================================
// Interface Hashes
// ============================================================================

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
/// Named references (`Type::Ref`) are resolved structurally when this is called
/// via the in-scope variants. This bare entry point has no resolution context,
/// so it falls back to a path-based hash for refs — useful only for primitive
/// or fully-monomorphic types.
pub fn hash_type(ty: &Type) -> TypeHash {
    hash_type_in(ty, &[])
}

/// Compute the TypeHash for a Type, resolving named refs against `types`.
///
/// When a `Type::Ref` names a local typedef, the underlying record/variant/etc
/// is hashed structurally — matching the actor-side `pack_types!` macro which
/// inlines refs at metadata-emission time. Self-references and cycles return
/// `HASH_SELF_REF`.
pub fn hash_type_in(ty: &Type, types: &[TypeDef]) -> TypeHash {
    hash_type_inner(ty, types, &mut Vec::new())
}

fn hash_type_inner(ty: &Type, types: &[TypeDef], stack: &mut Vec<String>) -> TypeHash {
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
        Type::List(inner) => hash_list(&hash_type_inner(inner, types, stack)),
        Type::Option(inner) => hash_option(&hash_type_inner(inner, types, stack)),
        Type::Result { ok, err } => hash_result(
            &hash_type_inner(ok, types, stack),
            &hash_type_inner(err, types, stack),
        ),
        Type::Tuple(elems) => {
            let hashes: Vec<_> = elems
                .iter()
                .map(|t| hash_type_inner(t, types, stack))
                .collect();
            hash_tuple(&hashes)
        }
        Type::Map { key, value } => hash_map(
            &hash_type_inner(key, types, stack),
            &hash_type_inner(value, types, stack),
        ),
        Type::Set(elem) => hash_set(&hash_type_inner(elem, types, stack)),
        Type::Ref(path) => hash_ref(path, types, stack),
        Type::App { path, args } => hash_app(path, args, types, stack),
        Type::Value => HASH_SELF_REF,
    }
}

/// Hash a generic type application. Under type-parameter erasure a generic
/// instantiation hashes identically to the equivalent hand-written monomorphic
/// type: we resolve the definition, substitute the arguments, and hash the
/// resulting structure. This keeps `pair<u32, string>` wire/hash-compatible
/// with a plain record of the same shape.
fn hash_app(
    path: &TypePath,
    args: &[Type],
    types: &[TypeDef],
    stack: &mut Vec<String>,
) -> TypeHash {
    if let Some(name) = path.as_simple() {
        // Cycle within the same instantiation: recurse as a self-reference.
        if stack.iter().any(|s| s == name) {
            return HASH_SELF_REF;
        }
        if let Some(td) = types.iter().find(|t| t.name() == name) {
            if td.type_params().len() == args.len() {
                let inst = td.instantiate(args);
                stack.push(name.to_string());
                let h = hash_typedef_inner(&inst, types, stack);
                stack.pop();
                return h;
            }
        }
    }
    // Unresolved or mismatched-arity application: fall back to a nominal
    // by-name hash so it still produces a stable value.
    hash_ref(path, types, stack)
}

fn hash_ref(path: &TypePath, types: &[TypeDef], stack: &mut Vec<String>) -> TypeHash {
    // Explicit self-reference: `self` in a recursive type definition.
    if path.is_self_ref() {
        return HASH_SELF_REF;
    }

    // Simple named ref: try to resolve against the in-scope typedefs.
    if let Some(name) = path.as_simple() {
        // Cycle: this name is already being hashed further up the stack.
        if stack.iter().any(|s| s == name) {
            return HASH_SELF_REF;
        }
        if let Some(td) = types.iter().find(|t| t.name() == name) {
            stack.push(name.to_string());
            let h = hash_typedef_inner(td, types, stack);
            stack.pop();
            return h;
        }
    }

    // Unresolved or qualified path: fall back to a path-based hash so refs
    // to types we can't see still produce a stable (if nominal) hash.
    let path_str = path.to_string();
    let mut hasher = Sha256::new();
    hasher.update(b"ref:");
    hasher.update(path_str.as_bytes());
    TypeHash::from_bytes(hasher.finalize().into())
}

fn hash_typedef_inner(td: &TypeDef, types: &[TypeDef], stack: &mut Vec<String>) -> TypeHash {
    match td {
        TypeDef::Alias { ty, .. } => hash_type_inner(ty, types, stack),
        TypeDef::Record { fields, .. } => {
            // Collect (name, hash) pairs, sort by name for canonical ordering.
            let pairs: Vec<(String, TypeHash)> = fields
                .iter()
                .map(|f| (f.name.clone(), hash_type_inner(&f.ty, types, stack)))
                .collect();
            let mut sorted: Vec<_> = pairs.iter().map(|(n, h)| (n.as_str(), *h)).collect();
            sorted.sort_by(|a, b| a.0.cmp(b.0));
            hash_record(&sorted)
        }
        TypeDef::Variant { cases, .. } => {
            // Unit payloads → None (matches actor-side `Option<TypeDesc>` shape).
            let pairs: Vec<(String, Option<TypeHash>)> = cases
                .iter()
                .map(|c| {
                    let payload = if c.payload.is_unit() {
                        None
                    } else {
                        Some(hash_type_inner(&c.payload, types, stack))
                    };
                    (c.name.clone(), payload)
                })
                .collect();
            let mut sorted: Vec<_> = pairs.iter().map(|(n, h)| (n.as_str(), *h)).collect();
            sorted.sort_by(|a, b| a.0.cmp(b.0));
            hash_variant(&sorted)
        }
        TypeDef::Enum { cases, .. } => {
            // Enum hashes as a variant with all-None payloads (matches actor side).
            let mut sorted: Vec<(&str, Option<TypeHash>)> =
                cases.iter().map(|c| (c.as_str(), None)).collect();
            sorted.sort_by(|a, b| a.0.cmp(b.0));
            hash_variant(&sorted)
        }
        TypeDef::Flags { .. } => HASH_FLAGS,
    }
}

/// Compute the hash for a function signature with no in-scope typedefs.
///
/// Equivalent to `hash_function_from_sig_in(func, &[])`. Refs resolve nominally.
pub fn hash_function_from_sig(func: &Function) -> TypeHash {
    hash_function_from_sig_in(func, &[])
}

/// Compute the hash for a function signature, resolving refs against `types`.
pub fn hash_function_from_sig_in(func: &Function, types: &[TypeDef]) -> TypeHash {
    let param_hashes: Vec<_> = func
        .params
        .iter()
        .map(|p| hash_type_in(&p.ty, types))
        .collect();
    let result_hashes: Vec<_> = func
        .results
        .iter()
        .map(|t| hash_type_in(t, types))
        .collect();
    hash_function(&param_hashes, &result_hashes)
}

/// Compute the interface hash for an Arena containing functions.
///
/// The Arena is treated as an interface — its name, in-scope type definitions,
/// and function signatures are hashed to produce a content-addressed
/// interface hash. Named refs in function signatures resolve structurally
/// against `interface_arena.types`.
pub fn compute_interface_hash(interface_arena: &Arena) -> TypeHash {
    // Resolve refs against this interface's own typedefs.
    let types: &[TypeDef] = &interface_arena.types;

    // Create bindings for each function (sorted by name for determinism)
    let mut bindings: Vec<_> = interface_arena
        .functions
        .iter()
        .map(|f| Binding {
            name: &f.name,
            hash: hash_function_from_sig_in(f, types),
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

    #[error("failed to parse wasm module: {0}")]
    WasmParse(String),
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
const TAG_MAP: u32 = 22;
const TAG_SET: u32 = 23;

// ============================================================================
// Metadata Decoding
// ============================================================================

/// The 4-byte magic (`CGRF`) that prefixes packr's metadata data segment.
pub const CGRF_MAGIC: [u8; 4] = [0x43, 0x47, 0x52, 0x46];

/// Statically extract the CGRF `__pack_types` metadata bytes from a wasm module
/// by scanning its data segments — no instantiation required. Returns `None` if
/// the module carries no packr metadata segment (e.g. an older or non-packr
/// module).
pub fn find_cgrf_metadata(wasm: &[u8]) -> Result<Option<Vec<u8>>, wasmparser::BinaryReaderError> {
    use wasmparser::{Parser, Payload};

    for payload in Parser::new(0).parse_all(wasm) {
        if let Payload::DataSection(reader) = payload? {
            for data in reader {
                let bytes = data?.data;
                if bytes.len() >= 4 && bytes[0..4] == CGRF_MAGIC {
                    return Ok(Some(bytes.to_vec()));
                }
            }
        }
    }
    Ok(None)
}

/// Extract and decode a wasm module's CGRF metadata into an [`Arena`].
///
/// Convenience wrapper: runs [`find_cgrf_metadata`] then [`decode_metadata`].
/// Returns `None` if the module carries no packr metadata segment.
pub fn metadata_from_module(wasm: &[u8]) -> Result<Option<Arena>, MetadataError> {
    match find_cgrf_metadata(wasm).map_err(|e| MetadataError::WasmParse(e.to_string()))? {
        Some(bytes) => Ok(Some(decode_metadata(&bytes)?)),
        None => Ok(None),
    }
}

/// Extract and decode a wasm module's CGRF metadata into [`MetadataWithHashes`].
///
/// Convenience wrapper: runs [`find_cgrf_metadata`] then
/// [`decode_metadata_with_hashes`]. Returns `None` if the module carries no
/// packr metadata segment.
pub fn metadata_with_hashes_from_module(
    wasm: &[u8],
) -> Result<Option<MetadataWithHashes>, MetadataError> {
    match find_cgrf_metadata(wasm).map_err(|e| MetadataError::WasmParse(e.to_string()))? {
        Some(bytes) => Ok(Some(decode_metadata_with_hashes(&bytes)?)),
        None => Ok(None),
    }
}

/// Decode CGRF bytes into an Arena.
///
/// The metadata format is a record with "imports" and "exports" lists,
/// each containing function signatures. This is converted to an Arena
/// with two child arenas: one for imports, one for exports.
pub fn decode_metadata(bytes: &[u8]) -> Result<Arena, MetadataError> {
    let value = decode_prefix(bytes)
        .map(|(v, _)| v)
        .map_err(|e| MetadataError::DecodeFailed(format!("{:?}", e)))?;

    match value {
        Value::Record { fields, .. } => {
            let mut imports = Vec::new();
            let mut exports = Vec::new();
            let mut type_defs = Vec::new();
            let mut type_params = Vec::new();

            for (name, val) in fields {
                match name.as_str() {
                    "imports" => imports = decode_func_sig_list(val, &mut type_defs)?,
                    "exports" => exports = decode_func_sig_list(val, &mut type_defs)?,
                    "type-params" => type_params = decode_type_param_list(val)?,
                    _ => {}
                }
            }

            // Build an Arena with imports and exports as child arenas
            let mut arena = Arena::new("package");
            arena.type_params = type_params;

            if !imports.is_empty() {
                let mut import_arena = Arena::new("imports");
                let mut by_interface: HashMap<String, Vec<Function>> = HashMap::new();
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
                let mut by_interface: HashMap<String, Vec<Function>> = HashMap::new();
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
    let value = decode_prefix(bytes)
        .map(|(v, _)| v)
        .map_err(|e| MetadataError::DecodeFailed(format!("{:?}", e)))?;

    match value {
        Value::Record { fields, .. } => {
            let mut imports = Vec::new();
            let mut exports = Vec::new();
            let mut import_hashes = Vec::new();
            let mut export_hashes = Vec::new();
            let mut type_defs = Vec::new();
            let mut type_params = Vec::new();

            for (name, val) in fields {
                match name.as_str() {
                    "imports" => imports = decode_func_sig_list(val, &mut type_defs)?,
                    "exports" => exports = decode_func_sig_list(val, &mut type_defs)?,
                    "import-hashes" => import_hashes = decode_interface_hash_list(val)?,
                    "export-hashes" => export_hashes = decode_interface_hash_list(val)?,
                    "type-params" => type_params = decode_type_param_list(val)?,
                    _ => {}
                }
            }

            // Build the arena (same as decode_metadata)
            let mut arena = Arena::new("package");
            arena.type_params = type_params;

            if !imports.is_empty() {
                let mut import_arena = Arena::new("imports");
                let mut by_interface: HashMap<String, Vec<Function>> = HashMap::new();
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
                let mut by_interface: HashMap<String, Vec<Function>> = HashMap::new();
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
                                    let a = match &parts[0] {
                                        Value::U64(v) => *v,
                                        _ => 0,
                                    };
                                    let b = match &parts[1] {
                                        Value::U64(v) => *v,
                                        _ => 0,
                                    };
                                    let c = match &parts[2] {
                                        Value::U64(v) => *v,
                                        _ => 0,
                                    };
                                    let d = match &parts[3] {
                                        Value::U64(v) => *v,
                                        _ => 0,
                                    };
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
fn decode_func_sig_list(
    value: Value,
    type_defs: &mut Vec<TypeDef>,
) -> Result<Vec<(String, Function)>, MetadataError> {
    match value {
        Value::List { items, .. } => items
            .into_iter()
            .map(|v| decode_func_sig(v, type_defs))
            .collect(),
        _ => Err(MetadataError::InvalidStructure(
            "expected list of function signatures".into(),
        )),
    }
}

/// Decode a function signature, collecting discovered TypeDefs.
/// Returns (interface_name, Function).
fn decode_func_sig(
    value: Value,
    type_defs: &mut Vec<TypeDef>,
) -> Result<(String, Function), MetadataError> {
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
                        params = decode_param_list(val, type_defs)?;
                    }
                    "results" => {
                        results = decode_type_list(val, type_defs)?;
                    }
                    _ => {}
                }
            }

            let mut func = Function::with_signature(name, params, results);
            // Attach discovered type definitions to the function
            func.types = type_defs.clone();

            Ok((interface, func))
        }
        _ => Err(MetadataError::InvalidStructure(
            "expected record for function signature".into(),
        )),
    }
}

fn decode_param_list(
    value: Value,
    type_defs: &mut Vec<TypeDef>,
) -> Result<Vec<Param>, MetadataError> {
    match value {
        Value::List { items, .. } => items
            .into_iter()
            .map(|v| decode_param(v, type_defs))
            .collect(),
        _ => Err(MetadataError::InvalidStructure(
            "expected list of parameters".into(),
        )),
    }
}

fn decode_param(value: Value, type_defs: &mut Vec<TypeDef>) -> Result<Param, MetadataError> {
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
                        ty = decode_type_collecting(val, type_defs)?;
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

fn decode_type_list(
    value: Value,
    type_defs: &mut Vec<TypeDef>,
) -> Result<Vec<Type>, MetadataError> {
    match value {
        Value::List { items, .. } => items
            .into_iter()
            .map(|v| decode_type_collecting(v, type_defs))
            .collect(),
        _ => Err(MetadataError::InvalidStructure(
            "expected list of types".into(),
        )),
    }
}

fn decode_type_collecting(
    value: Value,
    type_defs: &mut Vec<TypeDef>,
) -> Result<Type, MetadataError> {
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
                    Ok(Type::list(decode_type_collecting(inner, type_defs)?))
                }
                TAG_OPTION => {
                    let inner = payload.into_iter().next().ok_or_else(|| {
                        MetadataError::InvalidStructure("option missing inner type".into())
                    })?;
                    Ok(Type::option(decode_type_collecting(inner, type_defs)?))
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
                                    "ok" => ok = decode_type_collecting(val, type_defs)?,
                                    "err" => err = decode_type_collecting(val, type_defs)?,
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
                    decode_record_type(record, type_defs)
                }
                TAG_VARIANT => {
                    let record = payload.into_iter().next().ok_or_else(|| {
                        MetadataError::InvalidStructure("variant missing payload".into())
                    })?;
                    decode_variant_type(record, type_defs)
                }
                TAG_TUPLE => {
                    let list = payload.into_iter().next().ok_or_else(|| {
                        MetadataError::InvalidStructure("tuple missing payload".into())
                    })?;
                    match list {
                        Value::List { items, .. } => {
                            let types: Result<Vec<_>, _> = items
                                .into_iter()
                                .map(|v| decode_type_collecting(v, type_defs))
                                .collect();
                            Ok(Type::tuple(types?))
                        }
                        _ => Err(MetadataError::InvalidStructure(
                            "tuple payload not a list".into(),
                        )),
                    }
                }
                TAG_VALUE => Ok(Type::Value),
                TAG_UNIT => Ok(Type::Unit),
                TAG_MAP => {
                    let record = payload.into_iter().next().ok_or_else(|| {
                        MetadataError::InvalidStructure("map missing payload".into())
                    })?;
                    match record {
                        Value::Record { fields, .. } => {
                            let mut key = Type::Unit;
                            let mut value = Type::Unit;
                            for (name, val) in fields {
                                match name.as_str() {
                                    "key" => key = decode_type_collecting(val, type_defs)?,
                                    "value" => value = decode_type_collecting(val, type_defs)?,
                                    _ => {}
                                }
                            }
                            Ok(Type::map(key, value))
                        }
                        _ => Err(MetadataError::InvalidStructure(
                            "map payload not a record".into(),
                        )),
                    }
                }
                TAG_SET => {
                    let inner = payload.into_iter().next().ok_or_else(|| {
                        MetadataError::InvalidStructure("set missing element type".into())
                    })?;
                    Ok(Type::set(decode_type_collecting(inner, type_defs)?))
                }
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

fn decode_record_type(value: Value, type_defs: &mut Vec<TypeDef>) -> Result<Type, MetadataError> {
    match value {
        Value::Record {
            fields: rec_fields, ..
        } => {
            let mut name = String::new();
            let mut decoded_fields = Vec::new();

            for (fname, val) in rec_fields {
                match fname.as_str() {
                    "name" => {
                        if let Value::String(s) = val {
                            name = s;
                        }
                    }
                    "fields" => {
                        if let Value::List { items, .. } = val {
                            for item in items {
                                if let Value::Record {
                                    fields: field_rec, ..
                                } = item
                                {
                                    let mut field_name = String::new();
                                    let mut field_type = Type::Value;
                                    for (fn_name, fn_val) in field_rec {
                                        match fn_name.as_str() {
                                            "name" => {
                                                if let Value::String(s) = fn_val {
                                                    field_name = s;
                                                }
                                            }
                                            "type" => {
                                                field_type =
                                                    decode_type_collecting(fn_val, type_defs)?;
                                            }
                                            _ => {}
                                        }
                                    }
                                    decoded_fields.push(Field::new(field_name, field_type));
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }

            // Store the TypeDef for later resolution
            if !name.is_empty() && !decoded_fields.is_empty() {
                // Only add if we don't already have this type
                if !type_defs.iter().any(|td| td.name() == name) {
                    type_defs.push(TypeDef::Record {
                        name: name.clone(),
                        type_params: Vec::new(),
                        fields: decoded_fields,
                    });
                }
            }

            Ok(Type::Ref(TypePath::simple(name)))
        }
        _ => Err(MetadataError::InvalidStructure(
            "record payload not a record".into(),
        )),
    }
}

fn decode_variant_type(value: Value, type_defs: &mut Vec<TypeDef>) -> Result<Type, MetadataError> {
    match value {
        Value::Record {
            fields: rec_fields, ..
        } => {
            let mut name = String::new();
            let mut decoded_cases = Vec::new();

            for (fname, val) in rec_fields {
                match fname.as_str() {
                    "name" => {
                        if let Value::String(s) = val {
                            name = s;
                        }
                    }
                    "cases" => {
                        if let Value::List { items, .. } = val {
                            for item in items {
                                if let Value::Record {
                                    fields: case_rec, ..
                                } = item
                                {
                                    let mut case_name = String::new();
                                    let mut case_payload = Type::Unit;
                                    for (cn, cv) in case_rec {
                                        match cn.as_str() {
                                            "name" => {
                                                if let Value::String(s) = cv {
                                                    case_name = s;
                                                }
                                            }
                                            "payload" => {
                                                if let Value::Option {
                                                    value: Some(payload_val),
                                                    ..
                                                } = cv
                                                {
                                                    case_payload = decode_type_collecting(
                                                        *payload_val,
                                                        type_defs,
                                                    )?;
                                                }
                                            }
                                            _ => {}
                                        }
                                    }
                                    decoded_cases.push(Case::new(case_name, case_payload));
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }

            // Store the TypeDef for later resolution
            if !name.is_empty()
                && !decoded_cases.is_empty()
                && !type_defs.iter().any(|td| td.name() == name)
            {
                type_defs.push(TypeDef::Variant {
                    name: name.clone(),
                    type_params: Vec::new(),
                    cases: decoded_cases,
                });
            }

            Ok(Type::Ref(TypePath::simple(name)))
        }
        _ => Err(MetadataError::InvalidStructure(
            "variant payload not a record".into(),
        )),
    }
}

/// Decode the `type-params` metadata field (a list of `{name, constraint}`).
fn decode_type_param_list(val: Value) -> Result<Vec<TypeParam>, MetadataError> {
    let items = match val {
        Value::List { items, .. } => items,
        _ => {
            return Err(MetadataError::InvalidStructure(
                "type-params must be a list".into(),
            ))
        }
    };
    let mut params = Vec::with_capacity(items.len());
    for item in items {
        if let Value::Record { fields, .. } = item {
            let mut name = String::new();
            let mut constraint = String::new();
            for (n, v) in fields {
                match (n.as_str(), v) {
                    ("name", Value::String(s)) => name = s,
                    ("constraint", Value::String(s)) => constraint = s,
                    _ => {}
                }
            }
            params.push(TypeParam::new(
                name,
                if constraint.is_empty() {
                    None
                } else {
                    Some(constraint)
                },
            ));
        }
    }
    Ok(params)
}
