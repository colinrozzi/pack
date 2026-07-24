//! The M2 composition consumer fixture (the acceptance entry component).
//!
//! `comp-app2` imports BOTH `math.double` and `util.inc` (no host imports) and
//! exports `run`, implemented as `inc(double(n))`. Composed against `math-real`
//! (exports `math.double`) and `comp-util` (exports `util.inc`), both imports are
//! satisfied internally by bridging shims — the composite stands alone across
//! three isolated memories.
//!
//! `run(21)` = `inc(double(21))` = `inc(42)` = `43`.

#![no_std]

extern crate alloc;

use packr_guest::{export, import_from};

packr_guest::setup_guest!();

packr_guest::pack_types! {
    imports {
        math {
            double: func(n: s64) -> s64,
        }
        util {
            inc: func(n: s64) -> s64,
        }
    }
    exports {
        run: func(n: s64) -> s64,
    }
}

/// Wired to the linked `math` provider at compose time.
#[import_from("math")]
fn double(n: i64) -> i64;

/// Wired to the linked `util` provider at compose time.
#[import_from("util")]
fn inc(n: i64) -> i64;

/// run(n) = inc(double(n)).
#[export]
fn run(n: i64) -> i64 {
    inc(double(n))
}
