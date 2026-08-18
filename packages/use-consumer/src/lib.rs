//! Regression fixture for cross-file `use` imports.
//!
//! `consumer.pact` does `use "../use-shared.pact".{entry}` — pulling `entry`
//! (and its transitive deps `msg`/`kind`) from a SEPARATE .pact file so the
//! types are single-sourced. If this builds, the macro resolved the `use` path
//! (relative to `consumer.pact`'s dir), read the file, pulled the named type +
//! its transitive same-file dependencies, and generated them here — so `Entry`,
//! `Msg`, and `Kind` all exist locally even though only `Snapshot` is declared
//! in this file.

#![no_std]

extern crate alloc;

use packr_guest::export;

packr_guest::setup_guest!();

packr_guest::pact!(from "consumer.pact");

/// Forces the generated `Snapshot` (and the use-imported `Entry`/`Msg`/`Kind`
/// it contains) to compile + marshal.
#[export]
fn snap(s: Snapshot) -> Snapshot {
    Snapshot {
        latest: s.latest,
        count: s.count + 1,
    }
}
