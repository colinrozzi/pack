//! A provider whose `math` interface INTENTIONALLY mismatches `comp-app`'s import.
//!
//! `comp-app` imports `math { double: func(n: s64) -> s64 }`; this exports
//! `math { double: func(n: s32) -> s32 }`. Same interface + function name, but a
//! different signature — so the interface's Merkle hash differs. The hash-checked
//! composer must REJECT a link from `comp-app`'s `math.double` to this provider,
//! at compose time, rather than silently producing a composite that mis-marshals
//! `s64` args into an `s32` callee. Used by the negative test in
//! `tests/compose_hashcheck.rs`.

#![no_std]

extern crate alloc;

use packr_guest::{export, Value};

packr_guest::setup_guest!();

packr_guest::pack_types! {
    exports {
        math {
            double: func(n: s32) -> s32,
        }
    }
}

#[export]
fn double(input: Value) -> Value {
    match input {
        Value::S32(n) => Value::S32(n.wrapping_mul(2)),
        other => other,
    }
}
