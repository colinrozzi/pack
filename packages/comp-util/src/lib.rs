//! The M2 composition provider fixture for the `util` interface.
//!
//! `comp-util` exports `util { inc: func(n: s64) -> s64 }` with `inc(n) = n + 1`.
//! Composed as a provider, its `util.inc` export is wired to `comp-app2`'s
//! `util.inc` import by a bridging shim, across two isolated memories.

#![no_std]

extern crate alloc;

use packr_guest::{export, Value};

packr_guest::setup_guest!();

// Export the `util` INTERFACE (grouped), symmetric with comp-app2's import.
packr_guest::pack_types! {
    exports {
        util {
            inc: func(n: s64) -> s64,
        }
    }
}

/// inc(n) = n + 1. Value mode: marshals the S64 the consumer's `util.inc` import
/// stub sends.
#[export]
fn inc(input: Value) -> Value {
    match input {
        Value::S64(n) => Value::S64(n + 1),
        other => other,
    }
}
