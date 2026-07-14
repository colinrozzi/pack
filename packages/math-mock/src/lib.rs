//! A MOCK provider of the `math` interface — same interface as `math-real`
//! (so it hash-matches and is a type-safe substitute), but returns a sentinel
//! so a test can prove the mock, not the real provider, was called.

#![no_std]

extern crate alloc;

use packr_guest::{export, Value};

packr_guest::setup_guest!();

// Same grouped `math` interface as math-real → identical interface hash.
packr_guest::pack_types! {
    exports {
        math {
            double: func(n: s64) -> s64,
        }
    }
}

/// Mock: ignore the input, always return the sentinel 100. So `adder.process(n)`
/// = double(n) + 1 = 101 for any n — unmistakably the mock, not n*2.
#[export]
fn double(_input: Value) -> Value {
    Value::S64(100)
}
