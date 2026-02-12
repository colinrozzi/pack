//! Compile-time metadata encoding for pack_types!() macro.
//!
//! Converts type descriptors to `pack_abi::Value` and encodes to CGRF bytes.
//! Also computes Merkle-tree hashes for type compatibility checking.

use pack_abi::{
    encode, Value, ValueType, TypeHash, Binding,
    HASH_BOOL, HASH_U8, HASH_U16, HASH_U32, HASH_U64,
    HASH_S8, HASH_S16, HASH_S32, HASH_S64,
    HASH_F32, HASH_F64, HASH_CHAR, HASH_STRING, HASH_FLAGS,
    HASH_SELF_REF,
    hash_list, hash_option, hash_result, hash_tuple,
    hash_record, hash_variant, hash_function, hash_interface,
};
use std::collections::HashMap;

/// A function signature for metadata.
pub struct FuncSig {
    pub interface: String,
    pub name: String,
    pub params: Vec<(String, TypeDesc)>,
    pub results: Vec<TypeDesc>,
}

/// A type descriptor for metadata encoding.
pub enum TypeDesc {
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
    Flags,
    List(Box<TypeDesc>),
    Option(Box<TypeDesc>),
    Result {
        ok: Box<TypeDesc>,
        err: Box<TypeDesc>,
    },
    Record {
        name: std::string::String,
        fields: Vec<(std::string::String, TypeDesc)>,
    },
    Variant {
        name: std::string::String,
        cases: Vec<(std::string::String, Option<TypeDesc>)>,
    },
    Tuple(Vec<TypeDesc>),
    Value,
}

impl TypeDesc {
    /// Convert to a pack_abi::Value variant representing a type-desc.
    pub fn to_value(&self) -> Value {
        match self {
            TypeDesc::Bool => variant_no_payload("bool", 0),
            TypeDesc::U8 => variant_no_payload("u8", 1),
            TypeDesc::U16 => variant_no_payload("u16", 2),
            TypeDesc::U32 => variant_no_payload("u32", 3),
            TypeDesc::U64 => variant_no_payload("u64", 4),
            TypeDesc::S8 => variant_no_payload("s8", 5),
            TypeDesc::S16 => variant_no_payload("s16", 6),
            TypeDesc::S32 => variant_no_payload("s32", 7),
            TypeDesc::S64 => variant_no_payload("s64", 8),
            TypeDesc::F32 => variant_no_payload("f32", 9),
            TypeDesc::F64 => variant_no_payload("f64", 10),
            TypeDesc::Char => variant_no_payload("char", 11),
            TypeDesc::String => variant_no_payload("string", 12),
            TypeDesc::Flags => variant_no_payload("flags", 13),
            TypeDesc::List(inner) => Value::Variant {
                type_name: "type-desc".into(),
                case_name: "list".into(),
                tag: 14,
                payload: vec![inner.to_value()],
            },
            TypeDesc::Option(inner) => Value::Variant {
                type_name: "type-desc".into(),
                case_name: "option".into(),
                tag: 15,
                payload: vec![inner.to_value()],
            },
            TypeDesc::Result { ok, err } => Value::Variant {
                type_name: "type-desc".into(),
                case_name: "result".into(),
                tag: 16,
                payload: vec![Value::Record {
                    type_name: "result-desc".into(),
                    fields: vec![
                        ("ok".into(), ok.to_value()),
                        ("err".into(), err.to_value()),
                    ],
                }],
            },
            TypeDesc::Record { name, fields } => Value::Variant {
                type_name: "type-desc".into(),
                case_name: "record".into(),
                tag: 17,
                payload: vec![Value::Record {
                    type_name: "record-desc".into(),
                    fields: vec![
                        ("name".into(), Value::String(name.clone())),
                        (
                            "fields".into(),
                            Value::List {
                                elem_type: ValueType::Record("".into()),
                                items: fields
                                    .iter()
                                    .map(|(n, t)| Value::Record {
                                        type_name: "field-desc".into(),
                                        fields: vec![
                                            ("name".into(), Value::String(n.clone())),
                                            ("type".into(), t.to_value()),
                                        ],
                                    })
                                    .collect(),
                            },
                        ),
                    ],
                }],
            },
            TypeDesc::Variant { name, cases } => Value::Variant {
                type_name: "type-desc".into(),
                case_name: "variant".into(),
                tag: 18,
                payload: vec![Value::Record {
                    type_name: "variant-desc".into(),
                    fields: vec![
                        ("name".into(), Value::String(name.clone())),
                        (
                            "cases".into(),
                            Value::List {
                                elem_type: ValueType::Record("".into()),
                                items: cases
                                    .iter()
                                    .map(|(n, t)| Value::Record {
                                        type_name: "case-desc".into(),
                                        fields: vec![
                                            ("name".into(), Value::String(n.clone())),
                                            (
                                                "payload".into(),
                                                match t {
                                                    Some(td) => Value::Option {
                                                        inner_type: ValueType::Variant("".into()),
                                                        value: Some(Box::new(td.to_value())),
                                                    },
                                                    None => Value::Option {
                                                        inner_type: ValueType::Variant("".into()),
                                                        value: None,
                                                    },
                                                },
                                            ),
                                        ],
                                    })
                                    .collect(),
                            },
                        ),
                    ],
                }],
            },
            TypeDesc::Tuple(items) => Value::Variant {
                type_name: "type-desc".into(),
                case_name: "tuple".into(),
                tag: 19,
                payload: vec![Value::List {
                    elem_type: ValueType::Variant("".into()),
                    items: items.iter().map(|t| t.to_value()).collect(),
                }],
            },
            TypeDesc::Value => variant_no_payload("value", 20),
        }
    }

    /// Compute the Merkle-tree hash of this type.
    ///
    /// Type names are NOT included (structural hashing).
    /// Field/case names ARE included.
    pub fn to_hash(&self) -> TypeHash {
        match self {
            TypeDesc::Bool => HASH_BOOL,
            TypeDesc::U8 => HASH_U8,
            TypeDesc::U16 => HASH_U16,
            TypeDesc::U32 => HASH_U32,
            TypeDesc::U64 => HASH_U64,
            TypeDesc::S8 => HASH_S8,
            TypeDesc::S16 => HASH_S16,
            TypeDesc::S32 => HASH_S32,
            TypeDesc::S64 => HASH_S64,
            TypeDesc::F32 => HASH_F32,
            TypeDesc::F64 => HASH_F64,
            TypeDesc::Char => HASH_CHAR,
            TypeDesc::String => HASH_STRING,
            TypeDesc::Flags => HASH_FLAGS,
            TypeDesc::List(inner) => hash_list(&inner.to_hash()),
            TypeDesc::Option(inner) => hash_option(&inner.to_hash()),
            TypeDesc::Result { ok, err } => hash_result(&ok.to_hash(), &err.to_hash()),
            TypeDesc::Tuple(items) => {
                let hashes: Vec<_> = items.iter().map(|t| t.to_hash()).collect();
                hash_tuple(&hashes)
            }
            TypeDesc::Record { fields, .. } => {
                // Sort fields by name for canonical ordering
                let mut sorted: Vec<_> = fields.iter()
                    .map(|(n, t)| (n.as_str(), t.to_hash()))
                    .collect();
                sorted.sort_by(|a, b| a.0.cmp(b.0));
                hash_record(&sorted)
            }
            TypeDesc::Variant { cases, .. } => {
                // Sort cases by name for canonical ordering
                let mut sorted: Vec<_> = cases.iter()
                    .map(|(n, t)| (n.as_str(), t.as_ref().map(|td| td.to_hash())))
                    .collect();
                sorted.sort_by(|a, b| a.0.cmp(b.0));
                hash_variant(&sorted)
            }
            TypeDesc::Value => HASH_SELF_REF, // Treat 'value' as self-ref for now
        }
    }
}

/// Compute the hash for a function signature.
pub fn hash_func_sig(sig: &FuncSig) -> TypeHash {
    let param_hashes: Vec<_> = sig.params.iter().map(|(_, t)| t.to_hash()).collect();
    let result_hashes: Vec<_> = sig.results.iter().map(|t| t.to_hash()).collect();
    hash_function(&param_hashes, &result_hashes)
}

/// An interface with its computed hash.
pub struct InterfaceHash {
    pub name: String,
    pub hash: TypeHash,
}

/// Compute per-interface hashes from function signatures.
///
/// Groups functions by interface name and computes a hash for each interface.
pub fn compute_interface_hashes(funcs: &[FuncSig]) -> Vec<InterfaceHash> {
    // Group functions by interface
    let mut by_interface: HashMap<&str, Vec<&FuncSig>> = HashMap::new();
    for sig in funcs {
        by_interface.entry(&sig.interface).or_default().push(sig);
    }

    // Compute hash for each interface
    let mut result: Vec<InterfaceHash> = by_interface.into_iter()
        .map(|(iface_name, funcs)| {
            // Create bindings for each function (sorted by name)
            let mut bindings: Vec<_> = funcs.iter()
                .map(|f| Binding {
                    name: f.name.as_str(),
                    hash: hash_func_sig(f),
                })
                .collect();
            bindings.sort_by(|a, b| a.name.cmp(b.name));

            let iface_hash = hash_interface(
                iface_name,
                &[], // No type bindings for now (types are inlined)
                &bindings,
            );

            InterfaceHash {
                name: iface_name.to_string(),
                hash: iface_hash,
            }
        })
        .collect();

    // Sort by interface name for deterministic output
    result.sort_by(|a, b| a.name.cmp(&b.name));
    result
}

fn variant_no_payload(case_name: &str, tag: usize) -> Value {
    Value::Variant {
        type_name: "type-desc".into(),
        case_name: case_name.into(),
        tag,
        payload: vec![],
    }
}

fn func_sig_to_value(sig: &FuncSig) -> Value {
    Value::Record {
        type_name: "function-sig".into(),
        fields: vec![
            ("interface".into(), Value::String(sig.interface.clone())),
            ("name".into(), Value::String(sig.name.clone())),
            (
                "params".into(),
                Value::List {
                    elem_type: ValueType::Record("".into()),
                    items: sig
                        .params
                        .iter()
                        .map(|(name, ty)| Value::Record {
                            type_name: "param-sig".into(),
                            fields: vec![
                                ("name".into(), Value::String(name.clone())),
                                ("type".into(), ty.to_value()),
                            ],
                        })
                        .collect(),
                },
            ),
            (
                "results".into(),
                Value::List {
                    elem_type: ValueType::Variant("".into()),
                    items: sig.results.iter().map(|t| t.to_value()).collect(),
                },
            ),
        ],
    }
}

/// Convert an InterfaceHash to a Value for encoding.
fn interface_hash_to_value(ih: &InterfaceHash) -> Value {
    let (a, b, c, d) = ih.hash.to_u64s();
    Value::Record {
        type_name: "interface-hash".into(),
        fields: vec![
            ("name".into(), Value::String(ih.name.clone())),
            ("hash".into(), Value::Tuple(vec![
                Value::U64(a),
                Value::U64(b),
                Value::U64(c),
                Value::U64(d),
            ])),
        ],
    }
}

/// Encode metadata (imports, exports, and interface hashes) into CGRF bytes.
pub fn encode_metadata(imports: &[FuncSig], exports: &[FuncSig]) -> Vec<u8> {
    // Compute interface hashes
    let import_hashes = compute_interface_hashes(imports);
    let export_hashes = compute_interface_hashes(exports);

    let metadata = Value::Record {
        type_name: "package-metadata".into(),
        fields: vec![
            (
                "imports".into(),
                Value::List {
                    elem_type: ValueType::Record("".into()),
                    items: imports.iter().map(func_sig_to_value).collect(),
                },
            ),
            (
                "exports".into(),
                Value::List {
                    elem_type: ValueType::Record("".into()),
                    items: exports.iter().map(func_sig_to_value).collect(),
                },
            ),
            (
                "import-hashes".into(),
                Value::List {
                    elem_type: ValueType::Record("".into()),
                    items: import_hashes.iter().map(interface_hash_to_value).collect(),
                },
            ),
            (
                "export-hashes".into(),
                Value::List {
                    elem_type: ValueType::Record("".into()),
                    items: export_hashes.iter().map(interface_hash_to_value).collect(),
                },
            ),
        ],
    };

    encode(&metadata).expect("failed to encode metadata")
}

/// Convert a WIT parser Type to a TypeDesc.
pub fn wit_type_to_type_desc(
    ty: &crate::wit_parser::Type,
    types: &[crate::wit_parser::TypeDef],
) -> TypeDesc {
    match ty {
        crate::wit_parser::Type::Bool => TypeDesc::Bool,
        crate::wit_parser::Type::U8 => TypeDesc::U8,
        crate::wit_parser::Type::U16 => TypeDesc::U16,
        crate::wit_parser::Type::U32 => TypeDesc::U32,
        crate::wit_parser::Type::U64 => TypeDesc::U64,
        crate::wit_parser::Type::S8 => TypeDesc::S8,
        crate::wit_parser::Type::S16 => TypeDesc::S16,
        crate::wit_parser::Type::S32 => TypeDesc::S32,
        crate::wit_parser::Type::S64 => TypeDesc::S64,
        crate::wit_parser::Type::F32 => TypeDesc::F32,
        crate::wit_parser::Type::F64 => TypeDesc::F64,
        crate::wit_parser::Type::Char => TypeDesc::Char,
        crate::wit_parser::Type::String => TypeDesc::String,
        crate::wit_parser::Type::List(inner) => {
            TypeDesc::List(Box::new(wit_type_to_type_desc(inner, types)))
        }
        crate::wit_parser::Type::Option(inner) => {
            TypeDesc::Option(Box::new(wit_type_to_type_desc(inner, types)))
        }
        crate::wit_parser::Type::Result { ok, err } => TypeDesc::Result {
            ok: Box::new(
                ok.as_ref()
                    .map_or(TypeDesc::Bool, |t| wit_type_to_type_desc(t, types)),
            ),
            err: Box::new(
                err.as_ref()
                    .map_or(TypeDesc::String, |t| wit_type_to_type_desc(t, types)),
            ),
        },
        crate::wit_parser::Type::Tuple(items) => {
            TypeDesc::Tuple(items.iter().map(|t| wit_type_to_type_desc(t, types)).collect())
        }
        crate::wit_parser::Type::Named(name) => {
            if name == "value" {
                return TypeDesc::Value;
            }
            for td in types {
                if td.name() == name {
                    return typedef_to_type_desc(td, types);
                }
            }
            TypeDesc::Value
        }
        crate::wit_parser::Type::SelfRef => TypeDesc::Value,
    }
}

/// Convert a WIT TypeDef to a TypeDesc.
pub fn typedef_to_type_desc(
    td: &crate::wit_parser::TypeDef,
    types: &[crate::wit_parser::TypeDef],
) -> TypeDesc {
    match td {
        crate::wit_parser::TypeDef::Alias { ty, .. } => wit_type_to_type_desc(ty, types),
        crate::wit_parser::TypeDef::Record { name, fields } => TypeDesc::Record {
            name: name.clone(),
            fields: fields
                .iter()
                .map(|(n, t)| (n.clone(), wit_type_to_type_desc(t, types)))
                .collect(),
        },
        crate::wit_parser::TypeDef::Variant { name, cases } => TypeDesc::Variant {
            name: name.clone(),
            cases: cases
                .iter()
                .map(|c| {
                    (
                        c.name.clone(),
                        c.payload.as_ref().map(|t| wit_type_to_type_desc(t, types)),
                    )
                })
                .collect(),
        },
        crate::wit_parser::TypeDef::Enum { name, cases } => TypeDesc::Variant {
            name: name.clone(),
            cases: cases.iter().map(|c| (c.clone(), None)).collect(),
        },
        crate::wit_parser::TypeDef::Flags { .. } => TypeDesc::Flags,
    }
}
