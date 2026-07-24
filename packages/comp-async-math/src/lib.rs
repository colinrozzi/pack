//! The M3 async-composition provider fixture.
//!
//! `comp-async-math` exports `math.double` — but before returning it calls a
//! HOST import `host.tick`. When the composite is instantiated with `tick`
//! provided as a genuinely ASYNC host function (one that `await`s), the guest
//! fiber SUSPENDS inside `double`, *underneath* the synchronous bridging shim
//! that the entry consumer calls it through.
//!
//! This proves the M3 hypothesis: the shim is plain synchronous wasm, and
//! wasmtime's async support suspends the ENTIRE fiber (consumer frame → shim
//! frame → this provider frame, and every memory) at the async host-import
//! boundary, then resumes it. The suspend happens transparently below the shim,
//! so composition needs no async-specific shim machinery.
//!
//! Composed against `comp-app` (which imports `math.double` and exports
//! `run(n) = double(n)`), the `math` import is satisfied internally by the shim
//! while `host.tick` survives as a RESIDUAL import that the host supplies at
//! instantiate — exactly the residual-surface shape a real host-importing actor
//! has.

#![no_std]

extern crate alloc;

use packr_guest::{export, import_from};

packr_guest::setup_guest!();

packr_guest::pack_types! {
    imports {
        // HOST interface — survives as residual (no package provides it). The
        // test provides `tick` as an async fn, so the guest fiber suspends here.
        host {
            tick: func(),
        }
    }
    exports {
        math {
            double: func(n: s64) -> s64,
        }
    }
}

/// The residual host call. Wired to the runtime's async `tick` at instantiate.
/// It takes no args and returns nothing; the host provides it as an awaiting
/// async fn, so this call is the suspension point.
#[import_from("host")]
fn tick();

/// double(n) = n * 2, but only after crossing the async host boundary via
/// `tick()`. When the host suspends inside `tick`, the whole composed fiber
/// (including the shim that called this) suspends with it.
#[export]
fn double(n: i64) -> i64 {
    tick();
    n * 2
}
