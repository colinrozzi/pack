//! Regression fixture for the `wit!(from "path")` file form.
//!
//! Instead of an inline `wit! { ... }` block or a `wit/` directory, this crate
//! points the macro at a SHARED definition file that lives outside the crate
//! (`packages/shared-api.wit+`). If this package builds, the file form resolves
//! the path (relative to `CARGO_MANIFEST_DIR`), reads it, and generates the same
//! types the inline form would.

#![no_std]

extern crate alloc;

use packr_guest::export;

packr_guest::setup_guest!();

// Read the shared definition from a file outside this crate. A relative path is
// resolved against the crate root (`CARGO_MANIFEST_DIR`).
packr_guest::wit!(from "../shared-api.wit+");

/// Forces the generated `Greeting` From/TryFrom (via the derive) to compile,
/// proving the file-sourced types marshal exactly like inline ones.
#[export]
fn greet(g: Greeting) -> Greeting {
    Greeting {
        to: g.to,
        times: g.times + 1,
    }
}
