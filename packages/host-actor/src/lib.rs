//! A host-importing actor fixture — the first fixture whose composite has a
//! NON-EMPTY residual surface.
//!
//! It imports two *different kinds* of interface at once:
//!   - `theater:simple/runtime` — a HOST interface. No package provides it; the
//!     runtime supplies it at instantiate. It MUST survive composition as
//!     residual surface.
//!   - `math` — a HELPER interface. A linked provider (`math-real`) exports it,
//!     so `pack compose`/`link` internalizes the call.
//!
//! After composing this against a math provider, the `math` import is gone
//! (satisfied internally) while `theater:simple/runtime` remains — exactly the
//! import shape of a universal self-contained actor. The old `internalize`
//! zero-imports gate forbade this; the residual-surface model allows it.
//!
//! `process` stands in for a lifecycle handler (`handle-send`, etc.); the
//! residual-host mechanics are identical regardless of the export's name/shape,
//! and `value -> value` keeps the fixture runnable through the existing harness.
//!
//! Note: nothing here is theater-specific to *packr*. The composer treats
//! `theater:simple/runtime` as just-another-unsatisfied-import; it survives
//! because no package in the set exports it, not because of its name.

#![no_std]

extern crate alloc;

use packr_guest::{export, import_from, Value};

packr_guest::setup_guest!();

packr_guest::pack_types! {
    imports {
        // HOST interface — survives as residual (no package provides it).
        theater:simple/runtime {
            log: func(msg: string),
        }
        // HELPER interface — internalized by the linker (a provider exports it).
        math {
            double: func(n: s64) -> s64,
        }
    }
    exports {
        process: func(input: value) -> value,
    }
}

/// The residual host call. Wired to the runtime's `log` at instantiate.
#[import_from("theater:simple/runtime")]
fn log(msg: &str);

/// The internalized helper call. Wired to the linked provider at compose time.
#[import_from("math")]
fn double(n: i64) -> i64;

/// process(n) = double(n) + 1, after logging through the host.
#[export]
fn process(input: Value) -> Value {
    log("host-actor: processing");
    match input {
        Value::S64(n) => Value::S64(double(n) + 1),
        other => other,
    }
}
