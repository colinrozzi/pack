//! Value::Bytes: a list<u8> held as one buffer. Its wire form is the same
//! Array node as a List of U8, and an Array of u8 decodes to it.
use packr_abi::{decode, encode, Value, ValueType};

fn list_u8(b: &[u8]) -> Value {
    Value::List {
        elem_type: ValueType::U8,
        items: b.iter().map(|x| Value::U8(*x)).collect(),
    }
}

#[test]
fn bytes_encode_exactly_as_a_list_of_u8() {
    for b in [
        &b""[..],
        b"\x00\xff",
        b"hello",
        &(0..=255u8).collect::<Vec<_>>()[..],
    ] {
        assert_eq!(
            encode(&Value::Bytes(b.to_vec())).unwrap(),
            encode(&list_u8(b)).unwrap()
        );
    }
    // nested, as a host call result is
    let wrap = |v: Value| Value::Result {
        ok_type: ValueType::List(Box::new(ValueType::U8)),
        err_type: ValueType::String,
        value: Ok(Box::new(v)),
    };
    assert_eq!(
        encode(&wrap(Value::Bytes(b"abc".to_vec()))).unwrap(),
        encode(&wrap(list_u8(b"abc"))).unwrap()
    );
}

#[test]
fn an_array_of_u8_decodes_to_bytes() {
    let back = decode(&encode(&list_u8(b"xyz")).unwrap()).unwrap();
    assert_eq!(back, Value::Bytes(b"xyz".to_vec()));
    // other fixed-width arrays stay lists
    let u16s = Value::List {
        elem_type: ValueType::U16,
        items: vec![Value::U16(1), Value::U16(2)],
    };
    assert_eq!(decode(&encode(&u16s).unwrap()).unwrap(), u16s);
    // and re-encode to the same bytes
    let wire = encode(&list_u8(b"xyz")).unwrap();
    assert_eq!(encode(&decode(&wire).unwrap()).unwrap(), wire);
}

#[test]
fn both_forms_read_the_same() {
    for v in [Value::Bytes(b"ab".to_vec()), list_u8(b"ab")] {
        assert_eq!(v.as_bytes().unwrap().as_ref(), b"ab");
        assert_eq!(v.infer_type(), ValueType::List(Box::new(ValueType::U8)));
        let as_vec: Vec<u8> = Vec::<u8>::try_from(v.clone()).unwrap();
        assert_eq!(as_vec, b"ab");
    }
    assert_eq!(
        Value::Bytes(b"\x01\x02".to_vec()).to_string(),
        list_u8(b"\x01\x02").to_string()
    );
    assert!(Value::String("ab".into()).as_bytes().is_none());
    let mixed = Value::List {
        elem_type: ValueType::U8,
        items: vec![Value::U8(1), Value::U16(2)],
    };
    assert!(mixed.as_bytes().is_none());
}
