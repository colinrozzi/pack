//! A provider that exports the `math` interface — grouped, symmetric with how
//! adder *imports* it. Used to test interface-to-interface hash matching.

#![no_std]

extern crate alloc;

use packr_guest::{export, Value};

packr_guest::setup_guest!();

// Export the `math` INTERFACE (grouped), mirroring adder's grouped import.
packr_guest::pack_types! {
    exports {
        math {
            double: func(n: s64) -> s64,
        }
    }
}

/// double(n) = n * 2. Value mode: marshals the S64 that adder's `math.double`
/// import stub sends.
#[export]
fn double(input: Value) -> Value {
    match input {
        Value::S64(n) => Value::S64(n * 2),
        other => other,
    }
}
