//! PIC build of the echo package, used by the 1b dynamic-linking loader test
//! (`tests/pic_composition.rs`). Identical logic to `packages/echo`, but built
//! as a position-independent side module (see `.cargo/config.toml`) so it can
//! share a runtime-assigned memory with the in-wasm allocator.

#![no_std]

extern crate alloc;

use alloc::boxed::Box;
use packr_guest::{export, Value};

packr_guest::setup_guest!();

packr_guest::pack_types! {
    exports {
        echo: func(input: value) -> value,
        transform: func(input: value) -> value,
        describe: func(input: value) -> value,
        data_end_addr: func(input: value) -> value,
    }
}

// `__data_end` is defined in packr-guest (the `pic` feature). Referencing it from
// THIS crate is a cross-crate data reference, which under `-shared` routes through
// `GOT.mem.__data_end` — the same import real actors' dependency trees emit. The
// loader must satisfy it, so this keeps that path exercised in-tree.
extern "C" {
    static __data_end: u8;
}

#[export]
fn data_end_addr(_input: Value) -> Value {
    Value::S64(unsafe { core::ptr::addr_of!(__data_end) as i64 })
}

/// Exercises `format!()` with `&'static str` fragments. The literal pieces
/// ("n=", "!") live as pointers stored in the data segment; under a PIC loader
/// that doesn't apply data relocations they keep raw offsets and read as blank,
/// so the result would be just the interpolated number with the fragments gone.
#[export]
fn describe(input: Value) -> Value {
    let n = match input {
        Value::S64(n) => n,
        _ => -1,
    };
    Value::String(alloc::format!("n={n}!"))
}

#[export]
fn echo(input: Value) -> Value {
    input
}

#[export]
fn transform(input: Value) -> Value {
    transform_value(input)
}

fn transform_value(value: Value) -> Value {
    match value {
        Value::S64(n) => Value::S64(n * 2),
        Value::List { elem_type, items } => Value::List {
            elem_type,
            items: items.into_iter().map(transform_value).collect(),
        },
        Value::Tuple(items) => Value::Tuple(items.into_iter().map(transform_value).collect()),
        Value::Option { inner_type, value: Some(inner) } => Value::Option {
            inner_type,
            value: Some(Box::new(transform_value(*inner))),
        },
        Value::Record { type_name, fields } => Value::Record {
            type_name,
            fields: fields
                .into_iter()
                .map(|(name, val)| (name, transform_value(val)))
                .collect(),
        },
        other => other,
    }
}
