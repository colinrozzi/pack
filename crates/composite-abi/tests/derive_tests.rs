//! Tests for the GraphValue derive macro

use composite_abi::{GraphValue, Value};

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
        Value::Record(fields) => {
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
    let value = Value::Record(vec![
        ("x".to_string(), Value::S64(10)),
        ("y".to_string(), Value::S64(20)),
    ]);

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
        Value::Variant { tag, payload } => {
            assert_eq!(*tag, 2);
            assert!(payload.is_none());
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
        Value::Variant { tag, payload } => {
            assert_eq!(*tag, 0);
            assert!(payload.is_some());
            assert_eq!(**payload.as_ref().unwrap(), Value::F64(5.0));
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
        Value::Variant { tag, payload } => {
            assert_eq!(*tag, 1);
            assert!(payload.is_some());
            match payload.as_ref().unwrap().as_ref() {
                Value::Tuple(items) => {
                    assert_eq!(items.len(), 2);
                    assert_eq!(items[0], Value::F64(10.0));
                    assert_eq!(items[1], Value::F64(20.0));
                }
                _ => panic!("Expected Tuple payload"),
            }
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
        Tree::Node(vec![
            Tree::Leaf(2),
            Tree::Leaf(3),
        ]),
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
        Value::Record(fields) => {
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
