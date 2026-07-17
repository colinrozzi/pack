//! A fixture that exports an INTERFACE-QUALIFIED theater lifecycle function
//! (`theater:simple/actor.init`) — the export shape every real theater actor
//! has. It exists to catch the composite lifecycle-export trim's bug:
//! `host-actor`'s bare `.process` can't, because there the arena fn name and the
//! raw wasm symbol coincide. Here the raw symbol is `theater:simple/actor.init`
//! while the arena fn name is bare `init`, so a trim keyed on the wrong one
//! deletes the export — exactly what a real actor hits.

#![no_std]

extern crate alloc;

use packr_guest::{export, import_from, Value};

packr_guest::setup_guest!();

packr_guest::pack_types! {
    imports {
        // A host interface — survives as residual (the runtime provides it).
        theater:simple/runtime {
            log: func(msg: string),
        }
        // A HELPER interface — linked to a provider, which makes this actor an
        // "entry" so the composite lifecycle-export trim actually runs (a
        // zero-edge build skips it). That's the path where the interface-
        // qualified lifecycle export must survive.
        math {
            double: func(n: s64) -> s64,
        }
    }
    exports {
        // The lifecycle entry, INTERFACE-QUALIFIED — raw wasm symbol is
        // `theater:simple/actor.init`, not bare `init`.
        theater:simple/actor {
            init: func(input: value) -> value,
        }
    }
}

#[import_from("theater:simple/runtime")]
fn log(msg: &str);

#[import_from("math")]
fn double(n: i64) -> i64;

/// The lifecycle entry. Exported as `theater:simple/actor.init`.
#[export(name = "theater:simple/actor.init")]
fn init(input: Value) -> Value {
    log("lifecycle-actor: init");
    match input {
        Value::S64(n) => Value::S64(double(n) + 1),
        other => other,
    }
}
