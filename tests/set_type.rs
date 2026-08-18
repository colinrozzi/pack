//! `set<T>` is FIRST-CLASS: it lowers to `BTreeSet<T>` in Rust and marshals as a
//! `Value::Set` (its own wire node kind + hash), distinct from `list<T>`. These
//! tests pin that first-class identity — a set is no longer erased to a list.
//! Determinism (insertion-order independence) is preserved via canonical,
//! key-sorted items.

use std::collections::BTreeSet;

use packr::abi::{encode, Value};
use packr::metadata::hash_type;
use packr::parser::{decode_with_schema, encode_with_schema, parse_pact, PactExport};
use packr::types::Type;

fn string_set() -> Type {
    Type::set(Type::String)
}

fn string_list() -> Type {
    Type::list(Type::String)
}

#[test]
fn set_hashes_distinctly_from_list() {
    // First-class: `set<string>` has its own type hash, distinct from
    // `list<string>`.
    assert_ne!(hash_type(&string_set()), hash_type(&string_list()));
}

#[test]
fn set_parsed_from_pact_hashes_as_first_class_set() {
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
    assert_eq!(hash_type(param_ty), hash_type(&string_set()));
    assert_ne!(hash_type(param_ty), hash_type(&string_list()));
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
fn set_encoding_is_insertion_order_independent() {
    // A set is a `Value::Set` whose items are canonical (key-sorted), so
    // insertion order must not affect the encoded bytes.
    let mut a = BTreeSet::new();
    a.insert("k2".to_string());
    a.insert("k1".to_string());
    let mut b = BTreeSet::new();
    b.insert("k1".to_string());
    b.insert("k2".to_string());

    let a_value: Value = a.into();
    let b_value: Value = b.into();
    assert!(matches!(a_value, Value::Set { .. }));

    assert_eq!(
        encode(&a_value).expect("encode set a"),
        encode(&b_value).expect("encode set b"),
    );
}

#[test]
fn set_encodes_distinctly_from_list() {
    let mut s = BTreeSet::new();
    s.insert("k1".to_string());
    s.insert("k2".to_string());

    let set_value: Value = s.into();
    let list_value = Value::List {
        elem_type: packr::abi::ValueType::String,
        items: vec![
            Value::String("k1".to_string()),
            Value::String("k2".to_string()),
        ],
    };

    assert_ne!(
        encode(&set_value).expect("encode set"),
        encode(&list_value).expect("encode list"),
    );
}
