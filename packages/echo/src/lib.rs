//! A package that can decode, inspect, and re-encode graph values.
//!
//! This demonstrates using the composite-guest crate to simplify
//! WASM package development.

#![no_std]

extern crate alloc;

use alloc::boxed::Box;
use composite_guest::{export, Value};

// Set up allocator and panic handler
composite_guest::setup_guest!();

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
        Value::List(items) => Value::List(items.into_iter().map(transform_value).collect()),
        Value::Tuple(items) => Value::Tuple(items.into_iter().map(transform_value).collect()),
        Value::Option(Some(inner)) => {
            Value::Option(Some(Box::new(transform_value(*inner))))
        }
        Value::Variant { tag, payload } => Value::Variant {
            tag,
            payload: payload.map(|p| Box::new(transform_value(*p))),
        },
        Value::Record(fields) => Value::Record(
            fields
                .into_iter()
                .map(|(name, val)| (name, transform_value(val)))
                .collect(),
        ),
        // Other types pass through unchanged
        other => other,
    }
}
