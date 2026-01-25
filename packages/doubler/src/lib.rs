//! A package that doubles numbers.
//!
//! Used for testing package composition.

#![no_std]

extern crate alloc;

use composite_guest::{export, Value};

// Set up allocator and panic handler
composite_guest::setup_guest!();

/// Double an i64 value.
#[export]
fn double(input: Value) -> Value {
    match input {
        Value::S64(n) => Value::S64(n * 2),
        other => other, // Pass through non-i64 values unchanged
    }
}
