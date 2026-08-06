//! A GENERIC node fixture: it imports the `sm` interface parameterised over an
//! interface-level generic `s` (`type s: serializable`), and forwards a call
//! into it. Composed against `gen-sm` (which exports the SAME interface with `s`
//! pinned to `s64`), the hashes differ — so compose must UNIFY `s := s64` and
//! reconcile the link, rather than reject it.
//!
//! `run(n) = sm.apply(n)`, and `gen-sm.apply(n) = n + 1`, so `run(41) = 42`
//! after the value crosses the memory gap through the reconciled generic link.

#![no_std]

extern crate alloc;

use packr_guest::{export, import_from};

packr_guest::setup_guest!();

// The `sm` interface is declared GENERIC over `s` (interface-level parameter),
// which pack_types! embeds into __pack_types as this package's type_params.
packr_guest::pack_types! {
    type s: serializable
    imports {
        sm {
            apply: func(x: s) -> s,
        }
    }
    exports {
        run: func(n: s64) -> s64,
    }
}

/// Wired to the linked `sm` provider at compose time. `s` is erased at the wire,
/// so the stub marshals the concrete value the SM was pinned to.
#[import_from("sm")]
fn apply(x: i64) -> i64;

/// run(n) = sm.apply(n).
#[export]
fn run(n: i64) -> i64 {
    apply(n)
}
