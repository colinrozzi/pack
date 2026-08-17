//! `set<T>` is front-end sugar: it lowers to `BTreeSet<T>` in Rust and erases to
//! a key-sorted `list<T>` on the wire and in metadata. These tests pin that
//! erasure — a set must hash, encode, and validate identically to the
//! equivalent (sorted) list.

use std::collections::BTreeSet;

use packr::abi::{encode, Value};
use packr::metadata::hash_type;
use packr::parser::{decode_with_schema, encode_with_schema, parse_pact, PactExport};
use packr::types::Type;

fn string_set() -> Type {
    Type::set(Type::String)
}

fn desugared() -> Type {
    Type::list(Type::String)
}

#[test]
fn set_hashes_identically_to_list() {
    // Erasure: `set<string>` and `list<string>` are metadata-identical, so host
    // and guest agree on the link hash.
    assert_eq!(hash_type(&string_set()), hash_type(&desugared()));
}

#[test]
fn set_parsed_from_pact_hashes_like_list() {
    let src = r#"
        interface api {
            exports {
                members: func(s: set<string>) -> u32
            }
        }
    "#;
    let iface = parse_pact(src).expect("parse set interface");
    let func = iface
        .exports
        .iter()
        .find_map(|e| match e {
            PactExport::Function(f) if f.name == "members" => Some(f),
            _ => None,
        })
        .expect("members func");
    let param_ty = &func.params[0].ty;
    assert_eq!(hash_type(param_ty), hash_type(&desugared()));
}

#[test]
fn set_value_roundtrips_through_schema() {
    let mut s = BTreeSet::new();
    s.insert("beta".to_string());
    s.insert("alpha".to_string());
    s.insert("gamma".to_string());

    let value: Value = s.clone().into();
    let ty = string_set();

    let bytes = encode_with_schema(&[], &value, &ty).expect("encode set");
    let decoded = decode_with_schema(&[], &bytes, &ty, None).expect("decode set");

    let back: BTreeSet<String> = decoded.try_into().expect("value -> BTreeSet");
    assert_eq!(back, s);
}

#[test]
fn set_encoding_is_byte_identical_to_sorted_list() {
    // Insertion order must not matter: a set encodes as the key-sorted list.
    let mut s = BTreeSet::new();
    s.insert("k2".to_string());
    s.insert("k1".to_string());

    let set_value: Value = s.into();
    let list_value = Value::List {
        elem_type: packr::abi::ValueType::String,
        items: vec![
            Value::String("k1".to_string()),
            Value::String("k2".to_string()),
        ],
    };

    assert_eq!(
        encode(&set_value).expect("encode set"),
        encode(&list_value).expect("encode list"),
    );
}
