//! A PIC package that IMPORTS a host function and calls it — exercises host
//! function arg/return marshalling under PIC dynamic linking (the seam
//! theater-dev flagged). `run` forwards its input to the host `double_it`.

#![no_std]

extern crate alloc;

use packr_guest::{export, import_from, Value};

packr_guest::setup_guest!();

packr_guest::pack_types! {
    imports {
        host {
            double_it: func(v: value) -> value,
        }
    }
    exports {
        run: func(v: value) -> value,
    }
}

#[import_from("host")]
fn double_it(v: Value) -> Value;

#[export]
fn run(input: Value) -> Value {
    double_it(input)
}
