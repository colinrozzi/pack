//! Host-side metadata types for package type information.
//!
//! Packages embed CGRF-encoded metadata accessible via `__pack_types`.
//! This module provides types and a decoder for that metadata.

use crate::abi::{decode, Value};

/// Metadata about a package's type signatures.
#[derive(Debug, Clone)]
pub struct PackageMetadata {
    pub imports: Vec<FunctionSignature>,
    pub exports: Vec<FunctionSignature>,
}

/// A function signature.
#[derive(Debug, Clone)]
pub struct FunctionSignature {
    pub interface: String,
    pub name: String,
    pub params: Vec<ParamSignature>,
    pub results: Vec<TypeDesc>,
}

/// A parameter signature.
#[derive(Debug, Clone)]
pub struct ParamSignature {
    pub name: String,
    pub ty: TypeDesc,
}

/// A type descriptor.
#[derive(Debug, Clone, PartialEq)]
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
        fields: Vec<FieldDesc>,
    },
    Variant {
        name: std::string::String,
        cases: Vec<CaseDesc>,
    },
    Tuple(Vec<TypeDesc>),
    Value,
}

/// A record field descriptor.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldDesc {
    pub name: String,
    pub ty: TypeDesc,
}

/// A variant case descriptor.
#[derive(Debug, Clone, PartialEq)]
pub struct CaseDesc {
    pub name: String,
    pub payload: Option<TypeDesc>,
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
}

/// Decode CGRF bytes into PackageMetadata.
pub fn decode_metadata(bytes: &[u8]) -> Result<PackageMetadata, MetadataError> {
    let value = decode(bytes).map_err(|e| MetadataError::DecodeFailed(format!("{:?}", e)))?;

    match value {
        Value::Record { fields, .. } => {
            let mut imports = None;
            let mut exports = None;

            for (name, val) in fields {
                match name.as_str() {
                    "imports" => imports = Some(decode_func_sig_list(val)?),
                    "exports" => exports = Some(decode_func_sig_list(val)?),
                    _ => {}
                }
            }

            Ok(PackageMetadata {
                imports: imports.unwrap_or_default(),
                exports: exports.unwrap_or_default(),
            })
        }
        _ => Err(MetadataError::InvalidStructure(
            "expected record at top level".into(),
        )),
    }
}

fn decode_func_sig_list(value: Value) -> Result<Vec<FunctionSignature>, MetadataError> {
    match value {
        Value::List { items, .. } => items.into_iter().map(decode_func_sig).collect(),
        _ => Err(MetadataError::InvalidStructure(
            "expected list of function signatures".into(),
        )),
    }
}

fn decode_func_sig(value: Value) -> Result<FunctionSignature, MetadataError> {
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
                        results = decode_type_desc_list(val)?;
                    }
                    _ => {}
                }
            }

            Ok(FunctionSignature {
                interface,
                name,
                params,
                results,
            })
        }
        _ => Err(MetadataError::InvalidStructure(
            "expected record for function signature".into(),
        )),
    }
}

fn decode_param_list(value: Value) -> Result<Vec<ParamSignature>, MetadataError> {
    match value {
        Value::List { items, .. } => items.into_iter().map(decode_param).collect(),
        _ => Err(MetadataError::InvalidStructure(
            "expected list of parameters".into(),
        )),
    }
}

fn decode_param(value: Value) -> Result<ParamSignature, MetadataError> {
    match value {
        Value::Record { fields, .. } => {
            let mut name = String::new();
            let mut ty = TypeDesc::Value;

            for (field_name, val) in fields {
                match field_name.as_str() {
                    "name" => {
                        if let Value::String(s) = val {
                            name = s;
                        }
                    }
                    "type" => {
                        ty = decode_type_desc(val)?;
                    }
                    _ => {}
                }
            }

            Ok(ParamSignature { name, ty })
        }
        _ => Err(MetadataError::InvalidStructure(
            "expected record for parameter".into(),
        )),
    }
}

fn decode_type_desc_list(value: Value) -> Result<Vec<TypeDesc>, MetadataError> {
    match value {
        Value::List { items, .. } => items.into_iter().map(decode_type_desc).collect(),
        _ => Err(MetadataError::InvalidStructure(
            "expected list of type descriptors".into(),
        )),
    }
}

fn decode_type_desc(value: Value) -> Result<TypeDesc, MetadataError> {
    match value {
        Value::Variant { tag, payload, .. } => match tag {
            0 => Ok(TypeDesc::Bool),
            1 => Ok(TypeDesc::U8),
            2 => Ok(TypeDesc::U16),
            3 => Ok(TypeDesc::U32),
            4 => Ok(TypeDesc::U64),
            5 => Ok(TypeDesc::S8),
            6 => Ok(TypeDesc::S16),
            7 => Ok(TypeDesc::S32),
            8 => Ok(TypeDesc::S64),
            9 => Ok(TypeDesc::F32),
            10 => Ok(TypeDesc::F64),
            11 => Ok(TypeDesc::Char),
            12 => Ok(TypeDesc::String),
            13 => Ok(TypeDesc::Flags),
            14 => {
                let inner = payload.into_iter().next().ok_or_else(|| {
                    MetadataError::InvalidStructure("list missing element type".into())
                })?;
                Ok(TypeDesc::List(Box::new(decode_type_desc(inner)?)))
            }
            15 => {
                let inner = payload.into_iter().next().ok_or_else(|| {
                    MetadataError::InvalidStructure("option missing inner type".into())
                })?;
                Ok(TypeDesc::Option(Box::new(decode_type_desc(inner)?)))
            }
            16 => {
                let record = payload.into_iter().next().ok_or_else(|| {
                    MetadataError::InvalidStructure("result missing payload".into())
                })?;
                match record {
                    Value::Record { fields, .. } => {
                        let mut ok = TypeDesc::Bool;
                        let mut err = TypeDesc::String;
                        for (name, val) in fields {
                            match name.as_str() {
                                "ok" => ok = decode_type_desc(val)?,
                                "err" => err = decode_type_desc(val)?,
                                _ => {}
                            }
                        }
                        Ok(TypeDesc::Result {
                            ok: Box::new(ok),
                            err: Box::new(err),
                        })
                    }
                    _ => Err(MetadataError::InvalidStructure(
                        "result payload not a record".into(),
                    )),
                }
            }
            17 => {
                let record = payload.into_iter().next().ok_or_else(|| {
                    MetadataError::InvalidStructure("record missing payload".into())
                })?;
                decode_record_desc(record)
            }
            18 => {
                let record = payload.into_iter().next().ok_or_else(|| {
                    MetadataError::InvalidStructure("variant missing payload".into())
                })?;
                decode_variant_desc(record)
            }
            19 => {
                let list = payload.into_iter().next().ok_or_else(|| {
                    MetadataError::InvalidStructure("tuple missing payload".into())
                })?;
                match list {
                    Value::List { items, .. } => {
                        let descs: Result<Vec<_>, _> =
                            items.into_iter().map(decode_type_desc).collect();
                        Ok(TypeDesc::Tuple(descs?))
                    }
                    _ => Err(MetadataError::InvalidStructure(
                        "tuple payload not a list".into(),
                    )),
                }
            }
            20 => Ok(TypeDesc::Value),
            _ => Err(MetadataError::InvalidStructure(format!(
                "unknown type-desc tag: {}",
                tag
            ))),
        },
        _ => Err(MetadataError::InvalidStructure(
            "expected variant for type-desc".into(),
        )),
    }
}

fn decode_record_desc(value: Value) -> Result<TypeDesc, MetadataError> {
    match value {
        Value::Record {
            fields: rec_fields, ..
        } => {
            let mut name = String::new();
            let mut fields = Vec::new();
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
                                    fields: ffields, ..
                                } = item
                                {
                                    let mut field_name = String::new();
                                    let mut field_type = TypeDesc::Value;
                                    for (fn2, fv) in ffields {
                                        match fn2.as_str() {
                                            "name" => {
                                                if let Value::String(s) = fv {
                                                    field_name = s;
                                                }
                                            }
                                            "type" => {
                                                field_type = decode_type_desc(fv)?;
                                            }
                                            _ => {}
                                        }
                                    }
                                    fields.push(FieldDesc {
                                        name: field_name,
                                        ty: field_type,
                                    });
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ok(TypeDesc::Record { name, fields })
        }
        _ => Err(MetadataError::InvalidStructure(
            "record payload not a record".into(),
        )),
    }
}

fn decode_variant_desc(value: Value) -> Result<TypeDesc, MetadataError> {
    match value {
        Value::Record {
            fields: rec_fields, ..
        } => {
            let mut name = String::new();
            let mut cases = Vec::new();
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
                                    fields: cfields, ..
                                } = item
                                {
                                    let mut case_name = String::new();
                                    let mut case_payload = None;
                                    for (cn, cv) in cfields {
                                        match cn.as_str() {
                                            "name" => {
                                                if let Value::String(s) = cv {
                                                    case_name = s;
                                                }
                                            }
                                            "payload" => {
                                                if let Value::Option {
                                                    value: Some(inner), ..
                                                } = cv
                                                {
                                                    case_payload =
                                                        Some(decode_type_desc(*inner)?);
                                                }
                                            }
                                            _ => {}
                                        }
                                    }
                                    cases.push(CaseDesc {
                                        name: case_name,
                                        payload: case_payload,
                                    });
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ok(TypeDesc::Variant { name, cases })
        }
        _ => Err(MetadataError::InvalidStructure(
            "variant payload not a record".into(),
        )),
    }
}
