//! Doubles then adds one — PIC build that imports another PACKAGE's function.
//!
//! Identical logic to `packages/adder`, but a position-independent side module
//! (see `.cargo/config.toml` + the `pic` feature). Its `math::double` import is
//! wired DIRECTLY to `doubler-pic`'s `double` export in one shared memory — no
//! cross-memory bridge. Used by the multi-package PIC composition test.

#![no_std]

extern crate alloc;

use packr_guest::{export, import_from, Value};

packr_guest::setup_guest!();

packr_guest::pack_types! {
    imports {
        math {
            double: func(n: s64) -> s64,
            big: func(n: s64) -> value,
        }
    }
    exports {
        process: func(input: value) -> value,
        relay_big: func(input: value) -> value,
    }
}

/// Imported from the "math" module — wired to doubler-pic's `double` export by
/// the composition loader (shared memory, direct call).
#[import_from("math")]
fn double(n: i64) -> i64;

/// Imported large-return function (wired to doubler-pic's `big`).
#[import_from("math")]
fn big(n: i64) -> Value;

/// Relay a LARGE cross-package return: call the provider's `big(n)` and return its
/// (big) result. Exercises freeing the provider's result buffer.
#[export]
fn relay_big(input: Value) -> Value {
    match input {
        Value::S64(n) => big(n),
        other => other,
    }
}

/// Process: double the input (via the imported package), then add 1.
/// process(5) => (5 * 2) + 1 = 11
#[export]
fn process(input: Value) -> Value {
    match input {
        Value::S64(n) => {
            let doubled = double(n);
            Value::S64(doubled + 1)
        }
        other => other,
    }
}
