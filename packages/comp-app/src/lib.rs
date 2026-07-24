//! The M1 composition consumer fixture.
//!
//! `comp-app` imports ONLY `math.double` (no host imports) and exports `run`,
//! implemented by calling `double`. Composed against `math-real` (which exports
//! `math`), the `math` import is satisfied internally by a bridging shim — the
//! composite has an empty residual surface and stands alone.
//!
//! This is the minimal proof of the compose model: one component's import wired
//! to another component's export, across two isolated memories.

#![no_std]

extern crate alloc;

use packr_guest::{export, import_from};

packr_guest::setup_guest!();

packr_guest::pack_types! {
    imports {
        math {
            double: func(n: s64) -> s64,
        }
    }
    exports {
        run: func(n: s64) -> s64,
    }
}

/// The internalized helper call. Wired to the linked provider at compose time.
#[import_from("math")]
fn double(n: i64) -> i64;

/// run(n) = double(n).
#[export]
fn run(n: i64) -> i64 {
    double(n)
}
