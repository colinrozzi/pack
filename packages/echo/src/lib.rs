//! A package that can decode, inspect, and re-encode graph values.
//!
//! This demonstrates using the pack-guest crate to simplify
//! WASM package development.

#![no_std]

extern crate alloc;

use alloc::boxed::Box;
use pack_guest::{export, Value};

// Set up allocator and panic handler
pack_guest::setup_guest!();

pack_guest::pack_types! {
    exports {
        echo: func(input: value) -> value,
        transform: func(input: value) -> value,
    }
}

/// Echo: decode input, re-encode unchanged, return result.
/// This proves we can decode and encode values in the package.
#[export]
fn echo(input: Value) -> Value {
    input
}

/// Transform: decode input, modify the value, re-encode.
/// Example: if it's an S64, double it; otherwise pass through.
#[export]
fn transform(input: Value) -> Value {
    transform_value(input)
}

/// Recursively transform values - double any S64
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
        Value::Variant { type_name, case_name, tag, payload } => Value::Variant {
            type_name,
            case_name,
            tag,
            payload: payload.into_iter().map(transform_value).collect(),
        },
        Value::Record { type_name, fields } => Value::Record {
            type_name,
            fields: fields
                .into_iter()
                .map(|(name, val)| (name, transform_value(val)))
                .collect(),
        },
        // Other types pass through unchanged
        other => other,
    }
}
