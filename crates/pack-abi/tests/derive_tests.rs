//! Tests for the GraphValue derive macro
//!
//! Run with: cargo test -p pack-abi --features derive

#![cfg(feature = "derive")]

use packr_abi::{GraphValue, Rec, Value};

// ============================================================================
// Struct tests
// ============================================================================

#[derive(Debug, Clone, PartialEq, GraphValue)]
struct Point {
    x: i64,
    y: i64,
}

#[test]
fn struct_to_value() {
    let point = Point { x: 10, y: 20 };
    let value: Value = point.into();

    match value {
        Value::Record { fields, .. } => {
            assert_eq!(fields.len(), 2);
            assert_eq!(fields[0].0, "x");
            assert_eq!(fields[0].1, Value::S64(10));
            assert_eq!(fields[1].0, "y");
            assert_eq!(fields[1].1, Value::S64(20));
        }
        _ => panic!("Expected Record"),
    }
}

#[test]
fn value_to_struct() {
    let value = Value::Record {
        type_name: String::new(),
        fields: vec![
            ("x".to_string(), Value::S64(10)),
            ("y".to_string(), Value::S64(20)),
        ],
    };

    let point: Point = value.try_into().unwrap();
    assert_eq!(point.x, 10);
    assert_eq!(point.y, 20);
}

#[test]
fn struct_roundtrip() {
    let original = Point { x: 42, y: -17 };
    let value: Value = original.clone().into();
    let back: Point = value.try_into().unwrap();
    assert_eq!(original, back);
}

// ============================================================================
// Nested struct tests
// ============================================================================

#[derive(Debug, Clone, PartialEq, GraphValue)]
struct Rectangle {
    top_left: Point,
    bottom_right: Point,
}

#[test]
fn nested_struct_roundtrip() {
    let original = Rectangle {
        top_left: Point { x: 0, y: 10 },
        bottom_right: Point { x: 100, y: 0 },
    };
    let value: Value = original.clone().into();
    let back: Rectangle = value.try_into().unwrap();
    assert_eq!(original, back);
}

// ============================================================================
// Tuple struct tests
// ============================================================================

#[derive(Debug, Clone, PartialEq, GraphValue)]
struct Pair(i64, i64);

#[test]
fn tuple_struct_roundtrip() {
    let original = Pair(100, 200);
    let value: Value = original.clone().into();
    let back: Pair = value.try_into().unwrap();
    assert_eq!(original, back);
}

// ============================================================================
// Unit struct tests
// ============================================================================

#[derive(Debug, Clone, PartialEq, GraphValue)]
struct Unit;

#[test]
fn unit_struct_roundtrip() {
    let original = Unit;
    let value: Value = original.clone().into();
    let back: Unit = value.try_into().unwrap();
    assert_eq!(original, back);
}

// ============================================================================
// Enum tests
// ============================================================================

#[derive(Debug, Clone, PartialEq, GraphValue)]
enum Shape {
    Circle(f64),
    Rectangle(f64, f64),
    Point,
}

#[test]
fn enum_unit_variant() {
    let original = Shape::Point;
    let value: Value = original.clone().into();

    match &value {
        Value::Variant { tag, payload, .. } => {
            assert_eq!(*tag, 2);
            assert!(payload.is_empty());
        }
        _ => panic!("Expected Variant"),
    }

    let back: Shape = value.try_into().unwrap();
    assert_eq!(original, back);
}

#[test]
fn enum_single_payload() {
    let original = Shape::Circle(5.0);
    let value: Value = original.clone().into();

    match &value {
        Value::Variant { tag, payload, .. } => {
            assert_eq!(*tag, 0);
            assert_eq!(payload.len(), 1);
            assert_eq!(payload[0], Value::F64(5.0));
        }
        _ => panic!("Expected Variant"),
    }

    let back: Shape = value.try_into().unwrap();
    assert_eq!(original, back);
}

#[test]
fn enum_tuple_payload() {
    let original = Shape::Rectangle(10.0, 20.0);
    let value: Value = original.clone().into();

    match &value {
        Value::Variant { tag, payload, .. } => {
            assert_eq!(*tag, 1);
            assert_eq!(payload.len(), 2);
            assert_eq!(payload[0], Value::F64(10.0));
            assert_eq!(payload[1], Value::F64(20.0));
        }
        _ => panic!("Expected Variant"),
    }

    let back: Shape = value.try_into().unwrap();
    assert_eq!(original, back);
}

// ============================================================================
// Enum with struct variants
// ============================================================================

#[derive(Debug, Clone, PartialEq, GraphValue)]
enum Message {
    Quit,
    Move { x: i32, y: i32 },
    Write(String),
}

#[test]
fn enum_struct_variant() {
    let original = Message::Move { x: 10, y: 20 };
    let value: Value = original.clone().into();
    let back: Message = value.try_into().unwrap();
    assert_eq!(original, back);
}

#[test]
fn enum_string_variant() {
    let original = Message::Write("hello".to_string());
    let value: Value = original.clone().into();
    let back: Message = value.try_into().unwrap();
    assert_eq!(original, back);
}

// ============================================================================
// Recursive type tests
// ============================================================================

#[derive(Debug, Clone, PartialEq, GraphValue)]
enum Tree {
    Leaf(i64),
    Node(Vec<Tree>),
}

#[test]
fn recursive_enum_leaf() {
    let original = Tree::Leaf(42);
    let value: Value = original.clone().into();
    let back: Tree = value.try_into().unwrap();
    assert_eq!(original, back);
}

#[test]
fn recursive_enum_nested() {
    let original = Tree::Node(vec![
        Tree::Leaf(1),
        Tree::Node(vec![Tree::Leaf(2), Tree::Leaf(3)]),
        Tree::Leaf(4),
    ]);
    let value: Value = original.clone().into();
    let back: Tree = value.try_into().unwrap();
    assert_eq!(original, back);
}

// ============================================================================
// Attribute tests
// ============================================================================

#[derive(Debug, Clone, PartialEq, GraphValue)]
struct Person {
    #[graph(rename = "full_name")]
    name: String,
    age: i64,
}

#[test]
fn rename_attribute() {
    let person = Person {
        name: "Alice".to_string(),
        age: 30,
    };
    let value: Value = person.into();

    match value {
        Value::Record { fields, .. } => {
            assert!(fields.iter().any(|(name, _)| name == "full_name"));
            assert!(fields.iter().any(|(name, _)| name == "age"));
        }
        _ => panic!("Expected Record"),
    }
}

// ============================================================================
// Vec tests
// ============================================================================

#[derive(Debug, Clone, PartialEq, GraphValue)]
struct Container {
    items: Vec<i64>,
    name: String,
}

#[test]
fn vec_field_roundtrip() {
    let original = Container {
        items: vec![1, 2, 3],
        name: "test".to_string(),
    };
    let value: Value = original.clone().into();
    let back: Container = value.try_into().unwrap();
    assert_eq!(original, back);
}

#[test]
fn empty_vec_roundtrip() {
    let original = Container {
        items: vec![],
        name: "empty".to_string(),
    };
    let value: Value = original.clone().into();
    let back: Container = value.try_into().unwrap();
    assert_eq!(original, back);
}

// ============================================================================
// forward_compatible: schema-evolution-tolerant decode
// ============================================================================

// Simulates appending a `cc` field: MsgV1 is the pre-add shape, MsgV2 the post-add
// shape. Both opt into forward_compatible. StrictMsg is the negative control.
#[derive(Debug, Clone, PartialEq, GraphValue)]
#[graph(forward_compatible)]
struct MsgV1 {
    id: i64,
    body: String,
}

#[derive(Debug, Clone, PartialEq, GraphValue)]
#[graph(forward_compatible)]
struct MsgV2 {
    id: i64,
    body: String,
    cc: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, GraphValue)]
struct StrictMsg {
    id: i64,
    body: String,
}

#[test]
fn forward_compat_old_reads_new_ignores_extra_field() {
    // The ROLLBACK case: an OLD build (MsgV1) reads NEW data (MsgV2 with cc). The
    // extra `cc` field is ignored instead of erroring.
    let v2 = MsgV2 {
        id: 7,
        body: "hi".to_string(),
        cc: vec!["a".to_string()],
    };
    let value: Value = v2.into();
    let v1: MsgV1 = value.try_into().unwrap();
    assert_eq!(
        v1,
        MsgV1 {
            id: 7,
            body: "hi".to_string()
        }
    );
}

#[test]
fn forward_compat_new_reads_old_defaults_missing_field() {
    // The MIGRATION case: a NEW build (MsgV2) reads OLD data (MsgV1 without cc).
    // `cc` defaults instead of erroring — retiring hand-written pad-missing migrations.
    let v1 = MsgV1 {
        id: 7,
        body: "hi".to_string(),
    };
    let value: Value = v1.into();
    let v2: MsgV2 = value.try_into().unwrap();
    assert_eq!(
        v2,
        MsgV2 {
            id: 7,
            body: "hi".to_string(),
            cc: Vec::new()
        }
    );
}

#[test]
fn forward_compat_named_tolerates_reorder() {
    // Named decode is by NAME, so a reordered record still decodes.
    let reordered = Value::Record {
        type_name: String::new(),
        fields: vec![
            ("body".to_string(), Value::String("hi".to_string())),
            ("id".to_string(), Value::S64(7)),
        ],
    };
    let v1: MsgV1 = reordered.try_into().unwrap();
    assert_eq!(
        v1,
        MsgV1 {
            id: 7,
            body: "hi".to_string()
        }
    );
}

#[test]
fn strict_default_rejects_field_count_mismatch() {
    // Without #[graph(forward_compatible)], an extra field is still a hard error —
    // strictness is preserved everywhere it isn't explicitly opted out.
    let v2 = MsgV2 {
        id: 7,
        body: "hi".to_string(),
        cc: vec![],
    };
    let value: Value = v2.into();
    let r: Result<StrictMsg, _> = value.try_into();
    assert!(
        r.is_err(),
        "strict decode must reject a field-count mismatch"
    );
}

#[test]
fn forward_compat_encode_is_unchanged() {
    // forward_compatible only affects DECODE tolerance; encode still writes every
    // field, so the wire format is unchanged.
    let m = MsgV2 {
        id: 1,
        body: "x".to_string(),
        cc: vec!["a".to_string()],
    };
    let value: Value = m.clone().into();
    match &value {
        Value::Record { fields, .. } => assert_eq!(fields.len(), 3),
        _ => panic!("expected record"),
    }
    let back: MsgV2 = value.try_into().unwrap();
    assert_eq!(m, back);
}

// Tuple structs decode POSITIONALLY, so forward_compatible is append-only.
#[derive(Debug, Clone, PartialEq, GraphValue)]
#[graph(forward_compatible)]
struct TupV1(i64);

#[derive(Debug, Clone, PartialEq, GraphValue)]
#[graph(forward_compatible)]
struct TupV2(i64, i64);

#[test]
fn forward_compat_tuple_append_only() {
    // old-reads-new: TupV1 decodes a 2-element tuple, ignoring the trailing extra.
    let value: Value = TupV2(1, 2).into();
    let v1: TupV1 = value.try_into().unwrap();
    assert_eq!(v1, TupV1(1));

    // new-reads-old: TupV2 decodes a 1-element tuple, defaulting the missing trailing.
    let value2: Value = TupV1(9).into();
    let v2: TupV2 = value2.try_into().unwrap();
    assert_eq!(v2, TupV2(9, 0));
}

// ============================================================================
// Generic type tests (user-defined generics, M2)
// ============================================================================

#[derive(Debug, Clone, PartialEq, GraphValue)]
struct GenPair<A, B> {
    first: A,
    second: B,
}

#[test]
fn generic_struct_roundtrip() {
    let original = GenPair {
        first: 7u32,
        second: "hi".to_string(),
    };
    let value: Value = original.clone().into();
    let back: GenPair<u32, String> = value.try_into().unwrap();
    assert_eq!(original, back);
}

#[test]
fn generic_struct_distinct_instantiation_roundtrip() {
    // A different instantiation of the same generic type.
    let original = GenPair {
        first: true,
        second: vec![1i64, 2, 3],
    };
    let value: Value = original.clone().into();
    let back: GenPair<bool, Vec<i64>> = value.try_into().unwrap();
    assert_eq!(original, back);
}

// Generic parameter used inside a built-in container (`Vec<T>`). Note: a
// generic `Option<T>` field is not yet supported by the derive — packr's
// `Option<T>` decode goes through the `FromValue` trait (to avoid a coherence
// clash) rather than `TryFrom<Value>`, which the derive relies on. That's a
// pre-existing library limitation, tracked as an M2 follow-up.
#[derive(Debug, Clone, PartialEq, GraphValue)]
struct GenWrapper<T> {
    items: Vec<T>,
    label: String,
}

#[test]
fn generic_container_fields_roundtrip() {
    let original = GenWrapper {
        items: vec![1i64, 2, 3],
        label: "xs".to_string(),
    };
    let value: Value = original.clone().into();
    let back: GenWrapper<i64> = value.try_into().unwrap();
    assert_eq!(original, back);
}

// Generic enum.
#[derive(Debug, Clone, PartialEq, GraphValue)]
enum GenChoice<T> {
    Nothing,
    Just(T),
}

#[test]
fn generic_enum_roundtrip() {
    let just: GenChoice<u32> = GenChoice::Just(5);
    let value: Value = just.clone().into();
    let back: GenChoice<u32> = value.try_into().unwrap();
    assert_eq!(just, back);

    let nothing: GenChoice<u32> = GenChoice::Nothing;
    let value: Value = nothing.clone().into();
    let back: GenChoice<u32> = value.try_into().unwrap();
    assert_eq!(nothing, back);
}

// ============================================================================
// Option fields (decode via FromValue) — previously unsupported
// ============================================================================

#[derive(Debug, Clone, PartialEq, GraphValue)]
struct WithOption {
    id: u32,
    label: Option<String>,
    tags: Vec<i64>,
}

#[test]
fn option_field_roundtrip() {
    for label in [Some("hi".to_string()), None] {
        let original = WithOption {
            id: 7,
            label,
            tags: vec![1, 2, 3],
        };
        let value: Value = original.clone().into();
        let back: WithOption = value.try_into().unwrap();
        assert_eq!(original, back);
    }
}

// A GENERIC option field — the case the derive could not handle before the
// switch from TryFrom to FromValue for field decode.
#[derive(Debug, Clone, PartialEq, GraphValue)]
struct GenOpt<T> {
    val: Option<T>,
    also: Vec<T>,
}

#[test]
fn generic_option_field_roundtrip() {
    let original = GenOpt {
        val: Some(42u32),
        also: vec![1u32, 2],
    };
    let value: Value = original.clone().into();
    let back: GenOpt<u32> = value.try_into().unwrap();
    assert_eq!(original, back);

    let none: GenOpt<u32> = GenOpt {
        val: None,
        also: vec![],
    };
    let value: Value = none.clone().into();
    let back: GenOpt<u32> = value.try_into().unwrap();
    assert_eq!(none, back);
}

// ============================================================================
// Directly-recursive types via Box (a boxed self-reference)
// ============================================================================

#[derive(Debug, Clone, PartialEq, GraphValue)]
enum Expr {
    Lit(i64),
    Neg(Box<Expr>),
    Add(Box<Expr>, Box<Expr>),
}

#[test]
fn boxed_recursive_enum_roundtrip() {
    // Add(Neg(Lit(2)), Lit(3))
    let original = Expr::Add(
        Box::new(Expr::Neg(Box::new(Expr::Lit(2)))),
        Box::new(Expr::Lit(3)),
    );
    let value: Value = original.clone().into();
    let back: Expr = value.try_into().unwrap();
    assert_eq!(original, back);
}

// A recursive STRUCT needs an `Option<_<Self>>` field (the base case), which a
// bare `Box` cannot decode (see value.rs). `Rec<T>` — a packr-owned indirection
// — works in every position, including nested in a container, so recursive
// structs round-trip.
#[derive(Debug, Clone, PartialEq, GraphValue)]
struct RecCons {
    head: i64,
    tail: Option<Rec<RecCons>>,
}

#[test]
fn recursive_struct_via_rec_roundtrip() {
    // 1 -> 2 -> 3
    let list = RecCons {
        head: 1,
        tail: Some(Rec::new(RecCons {
            head: 2,
            tail: Some(Rec::new(RecCons {
                head: 3,
                tail: None,
            })),
        })),
    };
    let value: Value = list.clone().into();
    let back: RecCons = value.try_into().unwrap();
    assert_eq!(list, back);
    // Deref reaches the inner value.
    assert_eq!(list.tail.as_ref().unwrap().head, 2);
}

// `Rec<T>` also works for a variant self-reference (and nested in a Vec).
#[derive(Debug, Clone, PartialEq, GraphValue)]
enum RecTree {
    Leaf(i64),
    Node(Rec<RecTree>, Vec<RecTree>),
}

#[test]
fn recursive_enum_via_rec_roundtrip() {
    let original = RecTree::Node(
        Rec::new(RecTree::Leaf(1)),
        vec![
            RecTree::Leaf(2),
            RecTree::Node(Rec::new(RecTree::Leaf(3)), vec![]),
        ],
    );
    let value: Value = original.clone().into();
    let back: RecTree = value.try_into().unwrap();
    assert_eq!(original, back);
}

// ============================================================================
// map<K, V> -> BTreeMap field tests
// ============================================================================

// A `map<K, V>` pact field lowers to a `BTreeMap<K, V>` field, which is
// first-class on the wire: a `Value::Map` with key-sorted entries.
#[derive(Debug, Clone, PartialEq, GraphValue)]
struct Dict {
    name: String,
    entries: std::collections::BTreeMap<String, i32>,
}

#[test]
fn map_field_encodes_as_first_class_map() {
    let mut entries = std::collections::BTreeMap::new();
    entries.insert("b".to_string(), 2);
    entries.insert("a".to_string(), 1);
    let dict = Dict {
        name: "d".to_string(),
        entries,
    };

    let value: Value = dict.into();
    match value {
        Value::Record { fields, .. } => {
            let (_, entries_val) = fields.iter().find(|(n, _)| n == "entries").unwrap();
            match entries_val {
                Value::Map { entries, .. } => {
                    assert_eq!(entries.len(), 2);
                    // BTreeMap iterates sorted, so "a" precedes "b" — canonical.
                    assert_eq!(entries[0], (Value::String("a".to_string()), Value::S32(1)));
                    assert_eq!(entries[1], (Value::String("b".to_string()), Value::S32(2)));
                }
                other => panic!("expected Map for map field, got {other:?}"),
            }
        }
        other => panic!("expected Record, got {other:?}"),
    }
}

#[test]
fn map_field_roundtrip() {
    let mut entries = std::collections::BTreeMap::new();
    entries.insert("alpha".to_string(), 10);
    entries.insert("beta".to_string(), -20);
    entries.insert("gamma".to_string(), 30);
    let original = Dict {
        name: "config".to_string(),
        entries,
    };

    let value: Value = original.clone().into();
    let back: Dict = value.try_into().unwrap();
    assert_eq!(original, back);
}

// ============================================================================
// Convergence guarantees for typed, PERSISTED state-machine state
//
// State-machine actors (chat-sm, mesh) marshal a typed GraphValue state record
// that must (a) encode BYTE-IDENTICALLY for identical logical state across
// replicas (convergence + state hashing) and (b) tolerate schema evolution of
// the persisted bytes. These tests lock both so a future change can't silently
// break replica convergence.
// ============================================================================

#[derive(Debug, Clone, PartialEq, GraphValue)]
#[graph(forward_compatible)]
struct StateV1 {
    members: std::collections::BTreeSet<(u64, u64)>,
    tags: std::collections::BTreeMap<u64, Vec<u64>>,
    log: Vec<u64>,
}

// StateV1 with one appended field — an evolved persisted schema.
#[derive(Debug, Clone, PartialEq, GraphValue)]
#[graph(forward_compatible)]
struct StateV2 {
    members: std::collections::BTreeSet<(u64, u64)>,
    tags: std::collections::BTreeMap<u64, Vec<u64>>,
    log: Vec<u64>,
    reactions: std::collections::BTreeMap<u64, u64>,
}

/// A `map`/`set` (BTreeMap/BTreeSet) field encodes deterministically —
/// insertion order must NOT affect the bytes (both iterate in key order).
#[test]
fn btreemap_btreeset_encoding_is_insertion_order_independent() {
    let mut a = StateV1 {
        members: std::collections::BTreeSet::new(),
        tags: std::collections::BTreeMap::new(),
        log: vec![3, 1, 2],
    };
    a.members.insert((9, 1));
    a.members.insert((2, 7));
    a.members.insert((9, 0));
    a.tags.insert(9, vec![0, 1]);
    a.tags.insert(2, vec![7]);

    // Same logical state, reversed insertion order.
    let mut b = StateV1 {
        members: std::collections::BTreeSet::new(),
        tags: std::collections::BTreeMap::new(),
        log: vec![3, 1, 2],
    };
    b.members.insert((9, 0));
    b.members.insert((9, 1));
    b.members.insert((2, 7));
    b.tags.insert(2, vec![7]);
    b.tags.insert(9, vec![0, 1]);

    let bytes_a = packr_abi::encode(&Value::from(a.clone())).unwrap();
    let bytes_b = packr_abi::encode(&Value::from(b)).unwrap();
    assert_eq!(
        bytes_a, bytes_b,
        "encoding must be insertion-order-independent"
    );

    // And it round-trips (BTreeSet + BTreeMap fields).
    let back: StateV1 = packr_abi::decode(&bytes_a).unwrap().try_into().unwrap();
    assert_eq!(back, a);
}

/// `#[graph(forward_compatible)]` lets a PERSISTED record gain a field without
/// breaking old bytes: decoding old (V1) bytes into the evolved (V2) record
/// succeeds, with the new field defaulted.
#[test]
fn forward_compatible_decodes_old_bytes_after_field_add() {
    let mut v1 = StateV1 {
        members: std::collections::BTreeSet::new(),
        tags: std::collections::BTreeMap::new(),
        log: vec![1, 2, 3],
    };
    v1.members.insert((5, 0));
    v1.tags.insert(5, vec![0]);

    let old_bytes = packr_abi::encode(&Value::from(v1.clone())).unwrap();

    // Old bytes -> evolved record with an added field: succeeds, field defaulted.
    let v2: StateV2 = packr_abi::decode(&old_bytes).unwrap().try_into().unwrap();
    assert_eq!(v2.members, v1.members);
    assert_eq!(v2.tags, v1.tags);
    assert_eq!(v2.log, v1.log);
    assert_eq!(v2.reactions, std::collections::BTreeMap::new());

    // And the reverse: newer bytes -> old reader, extra field ignored.
    let v2_full = StateV2 {
        members: v1.members.clone(),
        tags: v1.tags.clone(),
        log: v1.log.clone(),
        reactions: {
            let mut m = std::collections::BTreeMap::new();
            m.insert(1u64, 9u64);
            m
        },
    };
    let new_bytes = packr_abi::encode(&Value::from(v2_full)).unwrap();
    let back_v1: StateV1 = packr_abi::decode(&new_bytes).unwrap().try_into().unwrap();
    assert_eq!(back_v1, v1);
}

// ============================================================================
// Regression: a variant case named `error` must not collide with the
// generated `TryFrom::Error` associated type. Before the fix, the derive spelled
// the try_from return type as `Self::Error`, which is ambiguous between an
// `Error` VARIANT and the trait's `Error` ASSOCIATED TYPE (deny-by-default
// `ambiguous_associated_items`). Any `pact!`/`wit!` variant with an `error` case
// tripped this. The fix spells the return type as the concrete `ConversionError`.
// ============================================================================

#[derive(Debug, Clone, PartialEq, GraphValue)]
enum Reply {
    Id(u64),
    State(u64),
    Error(String),
}

#[test]
fn variant_with_error_case_compiles_and_roundtrips() {
    for original in [
        Reply::Id(7),
        Reply::State(3),
        Reply::Error("boom".to_string()),
    ] {
        let value: Value = original.clone().into();
        let back: Reply = value.try_into().unwrap();
        assert_eq!(original, back);
    }
}

// A `BTreeSet` NESTED inside another container (`map<K, set<V>>` → `BTreeMap<K,
// BTreeSet<V>>`) round-trips. This exercises `KnownValueType for BTreeSet`
// (added for `set<T>`), which building the map's `elem_type` requires — a bare
// top-level `BTreeSet` field only needs `From`/`TryFrom`.
#[derive(Debug, Clone, PartialEq, GraphValue)]
struct Groups {
    groups: std::collections::BTreeMap<String, std::collections::BTreeSet<String>>,
}

#[test]
fn nested_set_in_map_roundtrips() {
    let mut groups = std::collections::BTreeMap::new();
    groups.insert("admins".to_string(), {
        let mut s = std::collections::BTreeSet::new();
        s.insert("alice".to_string());
        s.insert("bob".to_string());
        s
    });
    let original = Groups { groups };
    let value: Value = original.clone().into();
    let back: Groups = value.try_into().unwrap();
    assert_eq!(original, back);
}
