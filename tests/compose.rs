//! Static composition (`pack compose`) end-to-end test.
//!
//! Composes the allocator + doubler + adder fixtures into ONE self-contained
//! `.wasm` and runs `adder.process` — which calls `doubler.double` internally —
//! on a bare wasmtime with NO imports provided. Proves the merge + memory
//! unification + allocator internalization produce a working, standalone module.
//!
//! Requires `wasm-merge` (binaryen) on PATH; skips cleanly if absent (CI runs it
//! under `nix develop`, which provides binaryen).
//!
//! Fixtures (`assets/*_fixedbase.wasm`) are the packages built with the fixed-base
//! recipe: `RUSTFLAGS="-Clink-arg=--import-memory -Clink-arg=--initial-memory=8388608
//! -Clink-arg=--stack-first -Clink-arg=-zstack-size=262144
//! -Clink-arg=--global-base=<327680|851968> -Clink-arg=--no-entry"`.

use std::path::Path;

use packr::abi::{decode, encode, Value};
use packr::{compose, ComposeSpec, PackageSpec};
use wasmtime::{Engine, Instance, Linker, Module, Store};

fn asset(name: &str) -> Vec<u8> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join(name);
    std::fs::read(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

fn wasm_merge_available() -> bool {
    std::process::Command::new("wasm-merge")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[test]
fn compose_adder_calls_doubler_standalone() {
    if !wasm_merge_available() {
        eprintln!("SKIP: wasm-merge (binaryen) not on PATH");
        return;
    }

    let composed = compose(&ComposeSpec::new(vec![
        PackageSpec::new("pack:alloc", asset("pack_alloc_module.wasm")),
        PackageSpec::new("math", asset("doubler_fixedbase.wasm")), // adder imports "math"
        PackageSpec::new("adder", asset("adder_fixedbase.wasm")),
    ]))
    .expect("compose failed");

    // Load on a BARE runtime — zero imports, no harness.
    let engine = Engine::default();
    let module = Module::new(&engine, &composed).expect("composed module invalid");
    let mut store = Store::new(&engine, ());
    let inst = Linker::new(&engine)
        .instantiate(&mut store, &module)
        .expect("instantiate failed (module should be self-contained)");
    if let Ok(c) = inst.get_typed_func::<(), ()>(&mut store, "__wasm_call_ctors") {
        c.call(&mut store, ()).unwrap();
    }

    for (input, expected) in [(5, 11), (0, 1), (100, 201), (-3, -5)] {
        assert_eq!(
            call_process(&mut store, &inst, input),
            Value::S64(expected),
            "process({input})"
        );
    }
}

/// Marshal `n` through the composed module's `process` via the packr ABI.
fn call_process(store: &mut Store<()>, inst: &Instance, n: i64) -> Value {
    let mem = inst
        .exports(&mut *store)
        .filter_map(|e| e.into_memory())
        .next()
        .expect("composed module must export a memory");

    let bytes = encode(&Value::S64(n)).unwrap();
    let pa = inst
        .get_typed_func::<i32, i32>(&mut *store, "__pack_alloc")
        .unwrap();
    let in_ptr = pa.call(&mut *store, bytes.len() as i32).unwrap();
    mem.write(&mut *store, in_ptr as usize, &bytes).unwrap();
    let slots = pa.call(&mut *store, 8).unwrap();

    let f = inst
        .get_typed_func::<(i32, i32, i32, i32), i32>(&mut *store, "process")
        .unwrap();
    let status = f
        .call(&mut *store, (in_ptr, bytes.len() as i32, slots, slots + 4))
        .unwrap();
    assert_eq!(status, 0, "guest returned error status");

    let mut sb = [0u8; 8];
    mem.read(&*store, slots as usize, &mut sb).unwrap();
    let out_ptr = i32::from_le_bytes(sb[0..4].try_into().unwrap()) as usize;
    let out_len = i32::from_le_bytes(sb[4..8].try_into().unwrap()) as usize;
    let mut out = vec![0u8; out_len];
    mem.read(&*store, out_ptr, &mut out).unwrap();
    decode(&out).unwrap()
}
