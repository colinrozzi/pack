//! Canonical **packr-guest 0.18** "hello" actor — the reference shape every
//! guest should mirror when adopting 0.18 (the Pact-everywhere release, where
//! all `wit` naming became `pact`).
//!
//! In your `Cargo.toml`:
//!
//! ```toml
//! [dependencies]
//! packr-guest = "0.18"
//! ```
//!
//! This fixture builds to wasm in CI (`tests/hello_actor.rs`), so it stays
//! current — it cannot silently rot the way an out-of-CI template can.

#![no_std]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use packr_guest::{export, import};

// Bump allocator + panic handler for the wasm guest. Call once, at crate root.
packr_guest::setup_guest!();

// Declare this actor's interface — embedded as `__pack_types` metadata.
// `pact_types!` uses the imports/exports block form.
packr_guest::pack_types! {
    exports {
        greet: func(name: string) -> string,
    }
}

// Import a host function. Explicit `module` + `name` form:
#[import(module = "theater:simple/runtime", name = "log")]
fn log(msg: String);
// Shorthand alternative: `#[import(pact = "theater:simple/runtime.log")]`
// derives module+name from a Pact signature path. (This attribute argument was
// `wit = "…"` before 0.18.)

// Export a function callable by the host / other actors. `#[export]` marshals
// params/return through the Graph ABI.
#[export]
fn greet(name: String) -> String {
    log(format!("greeting {name}"));
    format!("hello, {name}")
}
