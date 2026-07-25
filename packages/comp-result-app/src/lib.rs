//! Consumer fixture that proves `#[import_from]` decodes a `Result`-returning
//! package function WITHOUT a hand-written `TryFrom<Value>` shim.
//!
//! This file is itself the regression test: under the old macro (which decoded
//! via `result.try_into()`), `#[import_from] fn checked(...) -> Result<i64, String>`
//! would fail to compile — the composite ABI implements `FromValue` for `Result`
//! but not `TryFrom<Value>`. With the macro fixed to decode via `FromValue`, it
//! compiles and round-trips. `mesh-client`'s `Result`-returning functions hit
//! exactly this (they needed a newtype shim before the fix).

#![no_std]

extern crate alloc;

use alloc::string::String;
use packr_guest::{export, import_from};

packr_guest::setup_guest!();

packr_guest::pack_types! {
    imports {
        mathr {
            checked: func(n: s64) -> result<s64, string>,
        }
    }
    exports {
        run: func(n: s64) -> result<s64, string>,
    }
}

/// The case under test: a `Result`-returning package import.
#[import_from("mathr")]
fn checked(n: i64) -> Result<i64, String>;

/// `run(n) = checked(n)` — forwards the composed provider's `Result` straight
/// through, so a passing round-trip proves the import decoded the `result<...>`.
#[export]
fn run(n: i64) -> Result<i64, String> {
    checked(n)
}
