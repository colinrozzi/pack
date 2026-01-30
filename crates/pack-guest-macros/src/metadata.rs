//! Compile-time metadata encoding for pack_types!() macro.
//!
//! Converts type descriptors to `pack_abi::Value` and encodes to CGRF bytes.

use pack_abi::{encode, Value, ValueType};

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

/// Encode metadata (imports and exports) into CGRF bytes.
pub fn encode_metadata(imports: &[FuncSig], exports: &[FuncSig]) -> Vec<u8> {
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
