//! Schema-aware validation for graph buffers.

use std::collections::HashMap;

use crate::abi::{encode, Decoder, GraphBuffer, GraphCodec, Limits, NodeKind, Value};
use crate::types::{Case, Field, Type, TypeDef, TypePath};
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
        Type::Unit => {
            // Unit type has no runtime representation in the graph
            // It should not appear as a node value
            Err(ValidationError::TypeMismatch {
                node: index,
                expected: "Unit".to_string(),
                actual: format!("{:?}", node.kind),
            })
        }
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
            // v2 format: [elem_type:type_tag*, count:u32, child_indices:u32*]
            cursor.skip_value_type()?;
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
            // v2 format: [inner_type:type_tag*, presence:u8, child_index?:u32]
            cursor.skip_value_type()?;
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
            expect_kind(index, node.kind, NodeKind::Result)?;
            let mut cursor = PayloadCursor::new(&node.payload);
            // v2 format: [ok_type:type_tag*, err_type:type_tag*, tag:u32, has_payload:u8, child_index?:u32]
            cursor.skip_value_type()?; // ok_type
            cursor.skip_value_type()?; // err_type
            let tag = cursor.read_u32()?;
            let has_payload = cursor.read_u8()?;
            let child = if has_payload == 1 {
                Some(cursor.read_u32()?)
            } else {
                None
            };
            cursor.finish(index)?;

            let expected = match tag {
                0 => {
                    // ok branch
                    if ok.is_unit() {
                        None
                    } else {
                        Some(ok.as_ref())
                    }
                }
                1 => {
                    // err branch
                    if err.is_unit() {
                        None
                    } else {
                        Some(err.as_ref())
                    }
                }
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
        Type::Ref(path) => {
            if path.is_self_ref() {
                let name = self_name.ok_or(ValidationError::SelfRefOutsideType)?;
                let def = types
                    .get(name)
                    .ok_or_else(|| ValidationError::UndefinedType(name.to_string()))?;
                validate_typedef(buffer, index, def, Some(name), types, assigned)
            } else if let Some(name) = path.as_simple() {
                let def = types
                    .get(name)
                    .ok_or_else(|| ValidationError::UndefinedType(name.to_string()))?;
                validate_typedef(buffer, index, def, Some(name), types, assigned)
            } else {
                // Qualified paths - not yet supported
                Err(ValidationError::UnsupportedType(format!(
                    "qualified type path: {}",
                    path
                )))
            }
        }
        Type::Value => {
            // Value type is a dynamic escape hatch - accept any node kind
            Ok(())
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
        TypeDef::Alias { ty, .. } => validate_type(buffer, index, ty, self_name, types, assigned),
        TypeDef::Record { name, fields } => {
            validate_record(buffer, index, name, fields, types, assigned)
        }
        TypeDef::Variant { name, cases } => {
            validate_variant(buffer, index, name, cases, types, assigned)
        }
        TypeDef::Enum { name, cases } => validate_enum(buffer, index, name, cases),
        TypeDef::Flags { name, flags } => validate_flags(buffer, index, name, flags),
    }
}

fn validate_value(
    value: &Value,
    ty: &Type,
    self_name: Option<&str>,
    types: &HashMap<String, &TypeDef>,
) -> Result<(), ValidationError> {
    match (value, ty) {
        (_, Type::Unit) => {
            // Unit type has no runtime value representation
            Err(ValidationError::TypeMismatch {
                node: 0,
                expected: "Unit".to_string(),
                actual: format!("{value:?}"),
            })
        }
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
        (Value::List { items, .. }, Type::List(inner)) => {
            for item in items {
                validate_value(item, inner, self_name, types)?;
            }
            Ok(())
        }
        (Value::Option { value, .. }, Type::Option(inner)) => {
            if let Some(item) = value.as_deref() {
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
        (_, Type::Value) => {
            // Value type is dynamic escape hatch - accept any value
            Ok(())
        }
        (value, Type::Ref(path)) => {
            if path.is_self_ref() {
                let name = self_name.ok_or(ValidationError::SelfRefOutsideType)?;
                let def = types
                    .get(name)
                    .ok_or_else(|| ValidationError::UndefinedType(name.to_string()))?;
                validate_value_named(value, def, types, self_name)
            } else if let Some(name) = path.as_simple() {
                let def = types
                    .get(name)
                    .ok_or_else(|| ValidationError::UndefinedType(name.to_string()))?;
                validate_value_named(value, def, types, Some(name))
            } else {
                Err(ValidationError::UnsupportedType(format!(
                    "qualified type path: {}",
                    path
                )))
            }
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
        TypeDef::Alias { ty, .. } => validate_value(value, ty, self_name, types),
        TypeDef::Record { name, fields } => match value {
            Value::Record {
                fields: value_fields,
                ..
            } => {
                if value_fields.len() != fields.len() {
                    return Err(ValidationError::TypeMismatch {
                        node: 0,
                        expected: format!("record({})", fields.len()),
                        actual: format!("record({})", value_fields.len()),
                    });
                }
                for (field, (value_name, value)) in fields.iter().zip(value_fields) {
                    if field.name != *value_name {
                        return Err(ValidationError::TypeMismatch {
                            node: 0,
                            expected: format!("field {}", field.name),
                            actual: format!("field {value_name}"),
                        });
                    }
                    validate_value(value, &field.ty, Some(name), types)?;
                }
                Ok(())
            }
            _ => Err(ValidationError::TypeMismatch {
                node: 0,
                expected: format!("record({})", fields.len()),
                actual: format!("{value:?}"),
            }),
        },
        TypeDef::Variant { name, cases } => match value {
            Value::Variant { tag, payload, .. } => {
                let payload_opt = payload.first();
                validate_value_variant(*tag, payload_opt, name, cases, types)?;
                Ok(())
            }
            _ => Err(ValidationError::TypeMismatch {
                node: 0,
                expected: format!("variant({})", cases.len()),
                actual: format!("{value:?}"),
            }),
        },
        TypeDef::Enum { cases, .. } => match value {
            Value::Variant { tag, payload, .. } => {
                if *tag >= cases.len() {
                    return Err(ValidationError::VariantTagOutOfRange {
                        node: 0,
                        tag: *tag as u32,
                        max: cases.len(),
                    });
                }
                if !payload.is_empty() {
                    return Err(ValidationError::VariantPayloadMismatch {
                        node: 0,
                        tag: *tag as u32,
                    });
                }
                Ok(())
            }
            _ => Err(ValidationError::TypeMismatch {
                node: 0,
                expected: format!("enum({})", cases.len()),
                actual: format!("{value:?}"),
            }),
        },
        TypeDef::Flags { flags, .. } => match value {
            Value::Flags(mask) => {
                if flags.len() > 64 {
                    return Err(ValidationError::UnsupportedType(format!(
                        "flags size {} exceeds 64",
                        flags.len()
                    )));
                }
                let max_mask = if flags.len() == 64 {
                    u64::MAX
                } else {
                    (1u64 << flags.len()) - 1
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
                expected: format!("flags({})", flags.len()),
                actual: format!("{value:?}"),
            }),
        },
    }
}

fn validate_value_variant(
    tag: usize,
    payload: Option<&Value>,
    variant_name: &str,
    cases: &[Case],
    types: &HashMap<String, &TypeDef>,
) -> Result<(), ValidationError> {
    if tag >= cases.len() {
        return Err(ValidationError::VariantTagOutOfRange {
            node: 0,
            tag: tag as u32,
            max: cases.len(),
        });
    }

    let case = &cases[tag];
    let has_payload = !case.payload.is_unit();

    match (has_payload, payload) {
        (false, None) | (false, Some(_)) if payload.map_or(true, |_| false) => Ok(()),
        (false, Some(_)) => Err(ValidationError::VariantPayloadMismatch {
            node: 0,
            tag: tag as u32,
        }),
        (false, None) => Ok(()),
        (true, Some(payload_value)) => {
            validate_value(payload_value, &case.payload, Some(variant_name), types)
        }
        (true, None) => Err(ValidationError::VariantPayloadMismatch {
            node: 0,
            tag: tag as u32,
        }),
    }
}

fn validate_record(
    buffer: &GraphBuffer,
    index: u32,
    record_name: &str,
    fields: &[Field],
    types: &HashMap<String, &TypeDef>,
    assigned: &mut HashMap<u32, String>,
) -> Result<(), ValidationError> {
    let node = &buffer.nodes[index as usize];
    expect_kind(index, node.kind, NodeKind::Record)?;
    let mut cursor = PayloadCursor::new(&node.payload);
    // v2 format: [type_name_len:u32, type_name:utf8, field_count:u32, field_names:(len:u32, name:utf8)*, child_indices:u32*]
    let type_name_len = cursor.read_u32()? as usize;
    cursor.read_bytes(type_name_len)?; // skip type_name
    let count = cursor.read_u32()? as usize;
    // Skip field names
    for _ in 0..count {
        let name_len = cursor.read_u32()? as usize;
        cursor.read_bytes(name_len)?;
    }
    // Read child indices
    let mut child_indices = Vec::with_capacity(count);
    for _ in 0..count {
        child_indices.push(cursor.read_u32()?);
    }
    cursor.finish(index)?;
    if count != fields.len() {
        return Err(ValidationError::TypeMismatch {
            node: index,
            expected: format!("record({})", fields.len()),
            actual: format!("record({count})"),
        });
    }
    for (field, child) in fields.iter().zip(child_indices) {
        validate_type(buffer, child, &field.ty, Some(record_name), types, assigned)?;
    }
    Ok(())
}

fn validate_variant(
    buffer: &GraphBuffer,
    index: u32,
    variant_name: &str,
    cases: &[Case],
    types: &HashMap<String, &TypeDef>,
    assigned: &mut HashMap<u32, String>,
) -> Result<(), ValidationError> {
    let node = &buffer.nodes[index as usize];
    expect_kind(index, node.kind, NodeKind::Variant)?;
    let mut cursor = PayloadCursor::new(&node.payload);
    // v2 format: [type_name_len:u32, type_name:utf8, case_name_len:u32, case_name:utf8, tag:u32, payload_count:u32, child_indices:u32*]
    let type_name_len = cursor.read_u32()? as usize;
    cursor.read_bytes(type_name_len)?; // skip type_name
    let case_name_len = cursor.read_u32()? as usize;
    cursor.read_bytes(case_name_len)?; // skip case_name
    let tag = cursor.read_u32()?;
    let payload_count = cursor.read_u32()? as usize;
    let mut children = Vec::with_capacity(payload_count);
    for _ in 0..payload_count {
        children.push(cursor.read_u32()?);
    }
    cursor.finish(index)?;

    if tag as usize >= cases.len() {
        return Err(ValidationError::VariantTagOutOfRange {
            node: index,
            tag,
            max: cases.len(),
        });
    }

    let case = &cases[tag as usize];
    let has_payload = !case.payload.is_unit();

    match (has_payload, children.first()) {
        (false, None) => Ok(()),
        (true, Some(&child)) => {
            validate_type(buffer, child, &case.payload, Some(variant_name), types, assigned)
        }
        _ => Err(ValidationError::VariantPayloadMismatch { node: index, tag }),
    }
}

fn validate_enum(
    buffer: &GraphBuffer,
    index: u32,
    _enum_name: &str,
    cases: &[String],
) -> Result<(), ValidationError> {
    let node = &buffer.nodes[index as usize];
    expect_kind(index, node.kind, NodeKind::Variant)?;
    let mut cursor = PayloadCursor::new(&node.payload);
    // v2 format: [type_name_len:u32, type_name:utf8, case_name_len:u32, case_name:utf8, tag:u32, payload_count:u32, child_indices:u32*]
    let type_name_len = cursor.read_u32()? as usize;
    cursor.read_bytes(type_name_len)?; // skip type_name
    let case_name_len = cursor.read_u32()? as usize;
    cursor.read_bytes(case_name_len)?; // skip case_name
    let tag = cursor.read_u32()?;
    let payload_count = cursor.read_u32()? as usize;
    for _ in 0..payload_count {
        cursor.read_u32()?; // skip child indices
    }
    cursor.finish(index)?;
    if tag as usize >= cases.len() {
        return Err(ValidationError::VariantTagOutOfRange {
            node: index,
            tag,
            max: cases.len(),
        });
    }
    if payload_count != 0 {
        return Err(ValidationError::VariantPayloadMismatch { node: index, tag });
    }
    Ok(())
}

fn validate_flags(
    buffer: &GraphBuffer,
    index: u32,
    _flags_name: &str,
    flags: &[String],
) -> Result<(), ValidationError> {
    let node = &buffer.nodes[index as usize];
    expect_kind(index, node.kind, NodeKind::Flags)?;
    if flags.len() > 64 {
        return Err(ValidationError::UnsupportedType(format!(
            "flags size {} exceeds 64",
            flags.len()
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
        Type::Ref(path) if path.is_self_ref() => format!("self({})", self_name.unwrap_or("?")),
        Type::Ref(path) => format!("ref({})", path),
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

    /// Skip over a type tag in CGRF v2 format.
    /// Type tags are recursive: simple types are 1 byte, compound types include nested type tags.
    fn skip_value_type(&mut self) -> Result<(), ValidationError> {
        const TYPE_BOOL: u8 = 0x01;
        const TYPE_S32: u8 = 0x02;
        const TYPE_S64: u8 = 0x03;
        const TYPE_F32: u8 = 0x04;
        const TYPE_F64: u8 = 0x05;
        const TYPE_STRING: u8 = 0x06;
        const TYPE_LIST: u8 = 0x07;
        const TYPE_VARIANT: u8 = 0x08;
        const TYPE_RECORD: u8 = 0x09;
        const TYPE_OPTION: u8 = 0x0A;
        const TYPE_TUPLE: u8 = 0x0B;
        const TYPE_U8: u8 = 0x0C;
        const TYPE_U16: u8 = 0x0D;
        const TYPE_U32: u8 = 0x0E;
        const TYPE_U64: u8 = 0x0F;
        const TYPE_S8: u8 = 0x10;
        const TYPE_S16: u8 = 0x11;
        const TYPE_CHAR: u8 = 0x12;
        const TYPE_FLAGS: u8 = 0x13;
        const TYPE_RESULT: u8 = 0x14;

        let tag = self.read_u8()?;
        match tag {
            TYPE_BOOL | TYPE_U8 | TYPE_U16 | TYPE_U32 | TYPE_U64 | TYPE_S8 | TYPE_S16
            | TYPE_S32 | TYPE_S64 | TYPE_F32 | TYPE_F64 | TYPE_CHAR | TYPE_STRING | TYPE_FLAGS => {
                // Simple types: just the tag byte
                Ok(())
            }
            TYPE_LIST | TYPE_OPTION => {
                // Compound with single nested type
                self.skip_value_type()
            }
            TYPE_RESULT => {
                // Result has ok_type and err_type
                self.skip_value_type()?;
                self.skip_value_type()
            }
            TYPE_RECORD | TYPE_VARIANT => {
                // Record/Variant: tag + name_len + name
                let len = self.read_u32()? as usize;
                self.read_bytes(len)?;
                Ok(())
            }
            TYPE_TUPLE => {
                // Tuple: tag + count + elem_types
                let count = self.read_u32()? as usize;
                for _ in 0..count {
                    self.skip_value_type()?;
                }
                Ok(())
            }
            _ => Err(ValidationError::InvalidEncoding(format!(
                "Unknown type tag: {tag:#x}"
            ))),
        }
    }
}
