//! A package that processes numbers by doubling then adding one.
//!
//! Demonstrates importing from another package using #[import_from].

#![no_std]

extern crate alloc;

use pack_guest::{export, import_from, Value};

// Set up allocator and panic handler
pack_guest::setup_guest!();

/// Import the double function from the "math" module.
/// This will be wired to the doubler package's "double" export
/// by the CompositionBuilder.
#[import_from("math")]
fn double(n: i64) -> i64;

/// Process: double the input, then add 1.
/// Result for input 5 should be: (5 * 2) + 1 = 11
#[export]
fn process(input: Value) -> Value {
    match input {
        Value::S64(n) => {
            let doubled = double(n);
            Value::S64(doubled + 1)
        }
        other => other, // Pass through non-i64 values unchanged
    }
}
