//! A package that doubles numbers — PIC build for shared-memory composition.
//!
//! Identical logic to `packages/doubler`, but built as a position-independent
//! side module (see `.cargo/config.toml` + the `pic` feature) so it can share a
//! runtime-assigned memory with the in-wasm allocator and other packages. Used by
//! the multi-package PIC composition test alongside `adder-pic`.

#![no_std]

extern crate alloc;

use packr_guest::{export, Value, ValueType};

packr_guest::setup_guest!();

packr_guest::pack_types! {
    exports {
        double: func(input: value) -> value,
        big: func(n: s64) -> value,
    }
}

/// Double an i64 value.
#[export]
fn double(input: Value) -> Value {
    match input {
        Value::S64(n) => Value::S64(n * 2),
        other => other,
    }
}

/// Return a List<s64> of `n` elements — a deliberately LARGE cross-package return
/// (used to prove the composition frees provider result buffers, not leaks them).
#[export]
fn big(n: i64) -> Value {
    let count = n.max(0) as usize;
    Value::List {
        elem_type: ValueType::S64,
        items: (0..count as i64).map(Value::S64).collect(),
    }
}
