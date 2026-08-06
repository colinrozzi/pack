//! The CONCRETE state-machine fixture: it exports the `sm` interface with the
//! generic parameter pinned to `s64`. Composed against `gen-node` (which imports
//! `sm` generically over `s`), this is the concrete side compose unifies
//! `s := s64` against.

#![no_std]

extern crate alloc;

use packr_guest::export;

packr_guest::setup_guest!();

// The SAME `sm` interface, concretely pinned to `s64` (no type_params).
packr_guest::pack_types! {
    exports {
        sm {
            apply: func(x: s64) -> s64,
        }
    }
}

/// apply(x) = x + 1.
#[export]
fn apply(x: i64) -> i64 {
    x + 1
}
