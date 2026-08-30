//! `map<K, V>` is FIRST-CLASS: it lowers to `BTreeMap<K, V>` in Rust and
//! marshals as a `Value::Map` (its own wire node kind + hash), distinct from
//! `list<tuple<K, V>>`. These tests pin that first-class identity — a map is no
//! longer erased to a list of key/value pairs.

use std::collections::BTreeMap;

use packr::abi::{encode, Value};
use packr::metadata::hash_type;
use packr::parser::{decode_with_schema, encode_with_schema, parse_pact, PactExport};
use packr::types::Type;

fn string_u32_map() -> Type {
    Type::map(Type::String, Type::U32)
}

fn list_of_pairs() -> Type {
    Type::list(Type::Tuple(vec![Type::String, Type::U32]))
}

#[test]
fn map_hashes_distinctly_from_list_of_pairs() {
    // First-class: `map<string, u32>` has its own type hash, distinct from the
    // erased `list<tuple<string, u32>>` form.
    assert_ne!(hash_type(&string_u32_map()), hash_type(&list_of_pairs()));
}

#[test]
fn map_parsed_from_pact_hashes_as_first_class_map() {
    let src = r#"
        interface api {
            exports {
                lookup: func(m: map<string, u32>) -> u32
            }
        }
    "#;
    let iface = parse_pact(src).expect("parse map interface");
    let func = iface
        .exports
        .iter()
        .find_map(|e| match e {
            PactExport::Function(f) if f.name == "lookup" => Some(f),
            _ => None,
        })
        .expect("lookup func");
    let param_ty = &func.params[0].ty;
    // The parsed type hashes as a first-class map (matching a hand-built
    // `Type::map`), and NOT as the erased list-of-pairs.
    assert_eq!(hash_type(param_ty), hash_type(&string_u32_map()));
    assert_ne!(hash_type(param_ty), hash_type(&list_of_pairs()));
}

#[test]
fn map_value_roundtrips_through_schema() {
    let mut m = BTreeMap::new();
    m.insert("beta".to_string(), 2u32);
    m.insert("alpha".to_string(), 1u32);
    m.insert("gamma".to_string(), 3u32);

    let value: Value = m.clone().into();
    let ty = string_u32_map();

    // Encode/validate against the map type, then decode back and reconstruct.
    let bytes = encode_with_schema(&[], &value, &ty).expect("encode map");
    let decoded = decode_with_schema(&[], &bytes, &ty, None).expect("decode map");

    let back: BTreeMap<String, u32> = decoded.try_into().expect("value -> BTreeMap");
    assert_eq!(back, m);
}

#[test]
fn map_encodes_as_first_class_map_not_list() {
    let mut m = BTreeMap::new();
    m.insert("k1".to_string(), 10u32);
    m.insert("k2".to_string(), 20u32);

    // A map is a `Value::Map` and encodes DISTINCTLY from the equivalent
    // key-sorted list of `tuple<K, V>` pairs (the old erased form).
    let map_value: Value = m.into();
    assert!(matches!(map_value, Value::Map { .. }));

    let list_value = Value::List {
        elem_type: packr::abi::ValueType::Tuple(vec![
            packr::abi::ValueType::String,
            packr::abi::ValueType::U32,
        ]),
        items: vec![
            Value::Tuple(vec![Value::String("k1".to_string()), Value::U32(10)]),
            Value::Tuple(vec![Value::String("k2".to_string()), Value::U32(20)]),
        ],
    };

    assert_ne!(
        encode(&map_value).expect("encode map"),
        encode(&list_value).expect("encode list"),
    );
}
