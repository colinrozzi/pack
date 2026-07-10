//! 1a proof: packages run on the DECOUPLED (imported) allocator.
//!
//! The `echo` package is built with `setup_guest!()`, which now installs the
//! `ImportedAllocator` shim — it imports `pack:alloc.alloc/dealloc` instead of
//! embedding an allocator. The runtime supplies a default *per-instance*,
//! non-intercepted `pack:alloc` provider, so a rich `Value` must round-trip
//! end to end with no allocator baked into the package binary.
//!
//! Unlike the `graph_abi_echo_*` tests (which use a hand-written WAT module),
//! this loads the real compiled `echo` *package*, so it actually exercises the
//! guest global allocator forwarding out to the imported `pack:alloc`.

use packr::abi::{Value, ValueType};
use packr::Runtime;
use std::path::Path;

fn echo_package() -> Vec<u8> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("packages/echo/target/wasm32-unknown-unknown/release/echo_package.wasm");
    std::fs::read(&p).unwrap_or_else(|e| {
        panic!(
            "read echo package at {}: {} — build it first: \
             cd packages/echo && cargo build --target wasm32-unknown-unknown --release",
            p.display(),
            e
        )
    })
}

fn round_trip(input: Value) {
    let runtime = Runtime::new();
    let module = runtime
        .load_module(&echo_package())
        .expect("load echo package");
    let mut instance = module.instantiate().expect("instantiate echo package");
    let output = instance
        .call_with_value("echo", &input)
        .expect("call echo through the imported allocator");
    assert_eq!(
        output, input,
        "echo must round-trip a value through the imported allocator"
    );
}

#[test]
fn imported_allocator_round_trips_string() {
    // Strings heap-allocate in the guest: decode + re-encode both hit pack:alloc.
    round_trip(Value::String("decoupled allocator: hello".to_string()));
}

#[test]
fn imported_allocator_round_trips_list() {
    round_trip(Value::List {
        elem_type: ValueType::S64,
        items: vec![Value::S64(1), Value::S64(2), Value::S64(3), Value::S64(-9)],
    });
}

#[test]
fn imported_allocator_round_trips_nested_strings() {
    round_trip(Value::List {
        elem_type: ValueType::String,
        items: vec![
            Value::String("alpha".to_string()),
            Value::String("beta".to_string()),
            Value::String("gamma-with-enough-length-to-force-a-heap-allocation".to_string()),
        ],
    });
}
