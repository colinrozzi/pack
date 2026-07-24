//! Theater-actor ENTRY fixture for the real-theater composition e2e.
//!
//! Unlike the other compose fixtures (which export a bare `run`), this one
//! exports `theater:simple/actor.init` so the ACTUAL theater runtime can drive
//! it through its normal actor lifecycle. It imports:
//!   - `math.double` — satisfied at compose time by the `math-real` PROVIDER
//!     component (internalized by a bridging shim across the memory gap), and
//!   - `theater:simple/runtime.log` — a residual HOST import theater fills at
//!     instantiate.
//!
//! `init` calls `double(21)` across the component gap and returns the result in
//! its state record. So a passing theater test that loads the COMPOSITE and sees
//! `doubled == 42` proves a multi-memory composite runs as a real theater actor,
//! with a cross-component call executing inside the actor lifecycle and theater's
//! own host functions satisfying the residual imports.

#![no_std]

extern crate alloc;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec;
use packr_guest::{export, import, pack_types, Value, ValueType};

packr_guest::setup_guest!();

pack_types! {
    imports {
        theater:simple/runtime {
            log: func(msg: string),
        }
        math {
            double: func(n: s64) -> s64,
        }
    }
    exports {
        theater:simple/actor.init: func(state: value) -> result<value, string>,
    }
}

/// Residual HOST import — theater provides this at instantiate.
#[import(module = "theater:simple/runtime", name = "log")]
fn log(msg: String);

/// Cross-component import — satisfied by the composed `math-real` provider.
#[import(module = "math", name = "double")]
fn double(n: i64) -> i64;

/// `theater:simple/actor.init`: call `double(21)` across the component gap and
/// stash the result in state.
#[export(name = "theater:simple/actor.init")]
fn init(_input: Value) -> Value {
    let doubled = double(21);
    log(String::from(
        "comp-actor: init called double(21) across the component gap",
    ));

    let state = Value::Record {
        type_name: String::from("comp-actor-state"),
        fields: vec![(String::from("doubled"), Value::S64(doubled))],
    };

    Value::Result {
        ok_type: state.infer_type(),
        err_type: ValueType::String,
        value: Ok(Box::new(state)),
    }
}
