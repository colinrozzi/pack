//! Schema-aware validation for graph buffers.

use std::collections::HashMap;

use crate::abi::{encode, Decoder, GraphBuffer, GraphCodec, Limits, NodeKind, Value};
use crate::wit_plus::{EnumDef, FlagsDef, RecordDef, Type, TypeDef, VariantDef};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ValidationError {
    #[error("Invalid encoding: {0}")]
    InvalidEncoding(String),
    #[error("Undefined type: {0}")]
    UndefinedType(String),
    #[error("Self reference used outside of a type definition")]
    SelfRefOutsideType,
    #[error("Type mismatch at node {node}: expected {expected}, got {actual}")]
    TypeMismatch {
        node: u32,
        expected: String,
        actual: String,
    },
    #[error("Variant tag out of range at node {node}: tag {tag}, max {max}")]
    VariantTagOutOfRange {
        node: u32,
        tag: u32,
        max: usize,
    },
    #[error("Variant payload mismatch at node {node} tag {tag}")]
    VariantPayloadMismatch {
        node: u32,
        tag: u32,
    },
    #[error("Unsupported type: {0}")]
    UnsupportedType(String),
}

pub fn validate_graph_against_type(
    types: &[TypeDef],
    buffer: &GraphBuffer,
    root_type: &Type,
) -> Result<(), ValidationError> {
    buffer
        .validate_basic()
        .map_err(|err| ValidationError::InvalidEncoding(err.to_string()))?;

    let mut map = HashMap::new();
    for def in types {
        map.insert(def.name().to_string(), def);
    }

    let mut assigned: HashMap<u32, String> = HashMap::new();
    validate_type(
        buffer,
        buffer.root,
        root_type,
        None,
        &map,
        &mut assigned,
    )
}

pub fn decode_with_schema(
    types: &[TypeDef],
    bytes: &[u8],
    root_type: &Type,
    limits: Option<&Limits>,
) -> Result<Value, ValidationError> {
    let limits = limits.copied().unwrap_or_default();
    let buffer = GraphBuffer::from_bytes_with_limits(bytes, &limits)
        .map_err(|err| ValidationError::InvalidEncoding(err.to_string()))?;
    buffer
        .validate_basic_with_limits(&limits)
        .map_err(|err| ValidationError::InvalidEncoding(err.to_string()))?;
    validate_graph_against_type(types, &buffer, root_type)?;

    let decoder = Decoder::new(&buffer);
    Value::decode_graph(&decoder, buffer.root)
        .map_err(|err| ValidationError::InvalidEncoding(err.to_string()))
}

pub fn encode_with_schema(
    types: &[TypeDef],
    value: &Value,
    root_type: &Type,
) -> Result<Vec<u8>, ValidationError> {
    let mut map = HashMap::new();
    for def in types {
        map.insert(def.name().to_string(), def);
    }
    validate_value(value, root_type, None, &map)?;
    encode(value).map_err(|err| ValidationError::InvalidEncoding(err.to_string()))
}

fn validate_type(
    buffer: &GraphBuffer,
    index: u32,
    ty: &Type,
    self_name: Option<&str>,
    types: &HashMap<String, &TypeDef>,
    assigned: &mut HashMap<u32, String>,
) -> Result<(), ValidationError> {
    let type_key = type_key(ty, self_name);
    if let Some(existing) = assigned.get(&index) {
        if existing != &type_key {
            return Err(ValidationError::TypeMismatch {
                node: index,
                expected: existing.clone(),
                actual: type_key,
            });
        }
        return Ok(());
    }

    assigned.insert(index, type_key.clone());

    let node = buffer.nodes.get(index as usize).ok_or_else(|| {
        ValidationError::InvalidEncoding(format!("Node index {index} out of range"))
    })?;

    match ty {
        Type::Bool => expect_kind(index, node.kind, NodeKind::Bool),
        Type::U8 => expect_kind(index, node.kind, NodeKind::U8),
        Type::U16 => expect_kind(index, node.kind, NodeKind::U16),
        Type::U32 => expect_kind(index, node.kind, NodeKind::U32),
        Type::U64 => expect_kind(index, node.kind, NodeKind::U64),
        Type::S8 => expect_kind(index, node.kind, NodeKind::S8),
        Type::S16 => expect_kind(index, node.kind, NodeKind::S16),
        Type::S32 => expect_kind(index, node.kind, NodeKind::S32),
        Type::S64 => expect_kind(index, node.kind, NodeKind::S64),
        Type::F32 => expect_kind(index, node.kind, NodeKind::F32),
        Type::F64 => expect_kind(index, node.kind, NodeKind::F64),
        Type::Char => expect_kind(index, node.kind, NodeKind::Char),
        Type::String => expect_kind(index, node.kind, NodeKind::String),
        Type::List(inner) => {
            expect_kind(index, node.kind, NodeKind::List)?;
            let mut cursor = PayloadCursor::new(&node.payload);
            let count = cursor.read_u32()? as usize;
            let mut child_indices = Vec::with_capacity(count);
            for _ in 0..count {
                child_indices.push(cursor.read_u32()?);
            }
            cursor.finish(index)?;
            for child in child_indices {
                validate_type(buffer, child, inner, self_name, types, assigned)?;
            }
            Ok(())
        }
        Type::Option(inner) => {
            expect_kind(index, node.kind, NodeKind::Option)?;
            let mut cursor = PayloadCursor::new(&node.payload);
            let has_value = cursor.read_u8()?;
            let child = if has_value == 1 {
                Some(cursor.read_u32()?)
            } else {
                None
            };
            cursor.finish(index)?;
            if let Some(child) = child {
                validate_type(buffer, child, inner, self_name, types, assigned)?;
            }
            Ok(())
        }
        Type::Result { ok, err } => {
            expect_kind(index, node.kind, NodeKind::Variant)?;
            let mut cursor = PayloadCursor::new(&node.payload);
            let tag = cursor.read_u32()?;
            let has_payload = cursor.read_u8()?;
            let child = if has_payload == 1 {
                Some(cursor.read_u32()?)
            } else {
                None
            };
            cursor.finish(index)?;

            let expected = match tag {
                0 => ok.as_deref(),
                1 => err.as_deref(),
                _ => {
                    return Err(ValidationError::VariantTagOutOfRange {
                        node: index,
                        tag,
                        max: 2,
                    })
                }
            };

            match (expected, child) {
                (None, None) => Ok(()),
                (Some(expected), Some(child)) => {
                    validate_type(buffer, child, expected, self_name, types, assigned)
                }
                _ => Err(ValidationError::VariantPayloadMismatch { node: index, tag }),
            }
        }
        Type::Tuple(items) => {
            expect_kind(index, node.kind, NodeKind::Tuple)?;
            let mut cursor = PayloadCursor::new(&node.payload);
            let count = cursor.read_u32()? as usize;
            let mut child_indices = Vec::with_capacity(count);
            for _ in 0..count {
                child_indices.push(cursor.read_u32()?);
            }
            cursor.finish(index)?;
            if count != items.len() {
                return Err(ValidationError::TypeMismatch {
                    node: index,
                    expected: format!("tuple({})", items.len()),
                    actual: format!("tuple({count})"),
                });
            }
            for (child, item) in child_indices.into_iter().zip(items) {
                validate_type(buffer, child, item, self_name, types, assigned)?;
            }
            Ok(())
        }
        Type::Named(name) => {
            let def = types
                .get(name)
                .ok_or_else(|| ValidationError::UndefinedType(name.clone()))?;
            validate_typedef(buffer, index, def, Some(name), types, assigned)
        }
        Type::SelfRef => {
            let name = self_name.ok_or(ValidationError::SelfRefOutsideType)?;
            let def = types
                .get(name)
                .ok_or_else(|| ValidationError::UndefinedType(name.to_string()))?;
            validate_typedef(buffer, index, def, Some(name), types, assigned)
        }
    }
}

fn validate_typedef(
    buffer: &GraphBuffer,
    index: u32,
    def: &TypeDef,
    self_name: Option<&str>,
    types: &HashMap<String, &TypeDef>,
    assigned: &mut HashMap<u32, String>,
) -> Result<(), ValidationError> {
    match def {
        TypeDef::Alias(_, ty) => validate_type(buffer, index, ty, self_name, types, assigned),
        TypeDef::Record(record) => validate_record(buffer, index, record, types, assigned),
        TypeDef::Variant(variant) => validate_variant(buffer, index, variant, types, assigned),
        TypeDef::Enum(enum_def) => validate_enum(buffer, index, enum_def),
        TypeDef::Flags(flags) => validate_flags(buffer, index, flags),
    }
}

fn validate_value(
    value: &Value,
    ty: &Type,
    self_name: Option<&str>,
    types: &HashMap<String, &TypeDef>,
) -> Result<(), ValidationError> {
    match (value, ty) {
        (Value::Bool(_), Type::Bool) => Ok(()),
        (Value::U8(_), Type::U8)
        | (Value::U16(_), Type::U16)
        | (Value::U32(_), Type::U32)
        | (Value::U64(_), Type::U64) => Ok(()),
        (Value::S8(_), Type::S8) | (Value::S16(_), Type::S16) => Ok(()),
        (Value::S32(_), Type::S32) | (Value::S64(_), Type::S64) => Ok(()),
        (Value::F32(_), Type::F32) | (Value::F64(_), Type::F64) => Ok(()),
        (Value::Char(_), Type::Char) => Ok(()),
        (Value::String(_), Type::String) => Ok(()),
        (Value::List(items), Type::List(inner)) => {
            for item in items {
                validate_value(item, inner, self_name, types)?;
            }
            Ok(())
        }
        (Value::Option(option), Type::Option(inner)) => {
            if let Some(item) = option.as_deref() {
                validate_value(item, inner, self_name, types)?;
            }
            Ok(())
        }
        (Value::Tuple(items), Type::Tuple(inner_types)) => {
            if items.len() != inner_types.len() {
                return Err(ValidationError::TypeMismatch {
                    node: 0,
                    expected: format!("tuple({})", inner_types.len()),
                    actual: format!("tuple({})", items.len()),
                });
            }
            for (item, inner) in items.iter().zip(inner_types) {
                validate_value(item, inner, self_name, types)?;
            }
            Ok(())
        }
        (value, Type::Named(name)) => {
            let def = types
                .get(name)
                .ok_or_else(|| ValidationError::UndefinedType(name.clone()))?;
            validate_value_named(value, def, types, self_name)
        }
        (value, Type::SelfRef) => {
            let name = self_name.ok_or(ValidationError::SelfRefOutsideType)?;
            let def = types
                .get(name)
                .ok_or_else(|| ValidationError::UndefinedType(name.to_string()))?;
            validate_value_named(value, def, types, self_name)
        }
        (value, _) => Err(ValidationError::TypeMismatch {
            node: 0,
            expected: format!("{ty:?}"),
            actual: format!("{value:?}"),
        }),
    }
}

fn validate_value_named(
    value: &Value,
    def: &TypeDef,
    types: &HashMap<String, &TypeDef>,
    self_name: Option<&str>,
) -> Result<(), ValidationError> {
    match def {
        TypeDef::Alias(_, ty) => validate_value(value, ty, self_name, types),
        TypeDef::Record(record) => match value {
            Value::Record(fields) => {
                if fields.len() != record.fields.len() {
                    return Err(ValidationError::TypeMismatch {
                        node: 0,
                        expected: format!("record({})", record.fields.len()),
                        actual: format!("record({})", fields.len()),
                    });
                }
                for ((field_name, field_ty), (value_name, value)) in
                    record.fields.iter().zip(fields)
                {
                    if field_name != value_name {
                        return Err(ValidationError::TypeMismatch {
                            node: 0,
                            expected: format!("field {field_name}"),
                            actual: format!("field {value_name}"),
                        });
                    }
                    validate_value(value, field_ty, Some(&record.name), types)?;
                }
                Ok(())
            }
            _ => Err(ValidationError::TypeMismatch {
                node: 0,
                expected: format!("record({})", record.fields.len()),
                actual: format!("{value:?}"),
            }),
        },
        TypeDef::Variant(variant) => match value {
            Value::Variant { tag, payload } => {
                validate_value_variant(*tag, payload.as_deref(), def, types)?;
                Ok(())
            }
            _ => Err(ValidationError::TypeMismatch {
                node: 0,
                expected: format!("variant({})", variant.cases.len()),
                actual: format!("{value:?}"),
            }),
        },
        TypeDef::Enum(enum_def) => match value {
            Value::Variant { tag, payload } => {
                if *tag >= enum_def.cases.len() {
                    return Err(ValidationError::VariantTagOutOfRange {
                        node: 0,
                        tag: *tag as u32,
                        max: enum_def.cases.len(),
                    });
                }
                if payload.is_some() {
                    return Err(ValidationError::VariantPayloadMismatch {
                        node: 0,
                        tag: *tag as u32,
                    });
                }
                Ok(())
            }
            _ => Err(ValidationError::TypeMismatch {
                node: 0,
                expected: format!("enum({})", enum_def.cases.len()),
                actual: format!("{value:?}"),
            }),
        },
        TypeDef::Flags(flags) => match value {
            Value::Flags(mask) => {
                if flags.flags.len() > 64 {
                    return Err(ValidationError::UnsupportedType(format!(
                        "flags size {} exceeds 64",
                        flags.flags.len()
                    )));
                }
                let max_mask = if flags.flags.len() == 64 {
                    u64::MAX
                } else {
                    (1u64 << flags.flags.len()) - 1
                };
                if *mask & !max_mask != 0 {
                    return Err(ValidationError::TypeMismatch {
                        node: 0,
                        expected: format!("flags mask <= {max_mask:#x}"),
                        actual: format!("{mask:#x}"),
                    });
                }
                Ok(())
            }
            _ => Err(ValidationError::TypeMismatch {
                node: 0,
                expected: format!("flags({})", flags.flags.len()),
                actual: format!("{value:?}"),
            }),
        },
    }
}

fn validate_value_variant(
    tag: usize,
    payload: Option<&Value>,
    def: &TypeDef,
    types: &HashMap<String, &TypeDef>,
) -> Result<(), ValidationError> {
    let variant = match def {
        TypeDef::Variant(variant) => variant,
        TypeDef::Enum(enum_def) => {
            if tag >= enum_def.cases.len() {
                return Err(ValidationError::VariantTagOutOfRange {
                    node: 0,
                    tag: tag as u32,
                    max: enum_def.cases.len(),
                });
            }
            if payload.is_some() {
                return Err(ValidationError::VariantPayloadMismatch {
                    node: 0,
                    tag: tag as u32,
                });
            }
            return Ok(());
        }
        _ => {
            return Err(ValidationError::TypeMismatch {
                node: 0,
                expected: format!("{def:?}"),
                actual: "variant".to_string(),
            })
        }
    };

    if tag >= variant.cases.len() {
        return Err(ValidationError::VariantTagOutOfRange {
            node: 0,
            tag: tag as u32,
            max: variant.cases.len(),
        });
    }
    let case = &variant.cases[tag];
    match (&case.payload, payload) {
        (None, None) => Ok(()),
        (Some(payload_ty), Some(payload_value)) => {
            validate_value(payload_value, payload_ty, Some(&variant.name), types)
        }
        _ => Err(ValidationError::VariantPayloadMismatch {
            node: 0,
            tag: tag as u32,
        }),
    }
}

fn validate_record(
    buffer: &GraphBuffer,
    index: u32,
    record: &RecordDef,
    types: &HashMap<String, &TypeDef>,
    assigned: &mut HashMap<u32, String>,
) -> Result<(), ValidationError> {
    let node = &buffer.nodes[index as usize];
    expect_kind(index, node.kind, NodeKind::Record)?;
    let mut cursor = PayloadCursor::new(&node.payload);
    let count = cursor.read_u32()? as usize;
    let mut child_indices = Vec::with_capacity(count);
    for _ in 0..count {
        child_indices.push(cursor.read_u32()?);
    }
    cursor.finish(index)?;
    if count != record.fields.len() {
        return Err(ValidationError::TypeMismatch {
            node: index,
            expected: format!("record({})", record.fields.len()),
            actual: format!("record({count})"),
        });
    }
    for ((_, field_ty), child) in record.fields.iter().zip(child_indices) {
        validate_type(buffer, child, field_ty, Some(&record.name), types, assigned)?;
    }
    Ok(())
}

fn validate_variant(
    buffer: &GraphBuffer,
    index: u32,
    variant: &VariantDef,
    types: &HashMap<String, &TypeDef>,
    assigned: &mut HashMap<u32, String>,
) -> Result<(), ValidationError> {
    let node = &buffer.nodes[index as usize];
    expect_kind(index, node.kind, NodeKind::Variant)?;
    let mut cursor = PayloadCursor::new(&node.payload);
    let tag = cursor.read_u32()?;
    let has_payload = cursor.read_u8()?;
    let child = if has_payload == 1 {
        Some(cursor.read_u32()?)
    } else {
        None
    };
    cursor.finish(index)?;

    if tag as usize >= variant.cases.len() {
        return Err(ValidationError::VariantTagOutOfRange {
            node: index,
            tag,
            max: variant.cases.len(),
        });
    }

    let case = &variant.cases[tag as usize];
    match (&case.payload, child) {
        (None, None) => Ok(()),
        (Some(payload_ty), Some(child)) => {
            validate_type(buffer, child, payload_ty, Some(&variant.name), types, assigned)
        }
        _ => Err(ValidationError::VariantPayloadMismatch { node: index, tag }),
    }
}

fn validate_enum(
    buffer: &GraphBuffer,
    index: u32,
    enum_def: &EnumDef,
) -> Result<(), ValidationError> {
    let node = &buffer.nodes[index as usize];
    expect_kind(index, node.kind, NodeKind::Variant)?;
    let mut cursor = PayloadCursor::new(&node.payload);
    let tag = cursor.read_u32()?;
    let has_payload = cursor.read_u8()?;
    cursor.finish(index)?;
    if tag as usize >= enum_def.cases.len() {
        return Err(ValidationError::VariantTagOutOfRange {
            node: index,
            tag,
            max: enum_def.cases.len(),
        });
    }
    if has_payload != 0 {
        return Err(ValidationError::VariantPayloadMismatch { node: index, tag });
    }
    Ok(())
}

fn validate_flags(
    buffer: &GraphBuffer,
    index: u32,
    flags: &FlagsDef,
) -> Result<(), ValidationError> {
    let node = &buffer.nodes[index as usize];
    expect_kind(index, node.kind, NodeKind::Flags)?;
    if flags.flags.len() > 64 {
        return Err(ValidationError::UnsupportedType(format!(
            "flags size {} exceeds 64",
            flags.flags.len()
        )));
    }
    Ok(())
}

fn expect_kind(
    node: u32,
    actual: NodeKind,
    expected: NodeKind,
) -> Result<(), ValidationError> {
    if actual == expected {
        Ok(())
    } else {
        Err(ValidationError::TypeMismatch {
            node,
            expected: format!("{expected:?}"),
            actual: format!("{actual:?}"),
        })
    }
}

fn type_key(ty: &Type, self_name: Option<&str>) -> String {
    match ty {
        Type::Named(name) => format!("named({name})"),
        Type::SelfRef => format!("self({})", self_name.unwrap_or("?")),
        _ => format!("{ty:?}"),
    }
}

struct PayloadCursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> PayloadCursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn read_u8(&mut self) -> Result<u8, ValidationError> {
        let bytes = self.read_bytes(1)?;
        Ok(bytes[0])
    }

    fn read_u32(&mut self) -> Result<u32, ValidationError> {
        let bytes = self.read_bytes(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn read_bytes(&mut self, len: usize) -> Result<&'a [u8], ValidationError> {
        if self.pos + len > self.bytes.len() {
            return Err(ValidationError::InvalidEncoding(
                "Truncated node payload".to_string(),
            ));
        }
        let start = self.pos;
        self.pos += len;
        Ok(&self.bytes[start..self.pos])
    }

    fn finish(self, index: u32) -> Result<(), ValidationError> {
        if self.pos != self.bytes.len() {
            Err(ValidationError::InvalidEncoding(format!(
                "Trailing payload bytes at node {index}"
            )))
        } else {
            Ok(())
        }
    }
}
