//! Package linker — hash-checked interface matching + mock testing.
//!
//! adder *imports* the `math` interface. `math-real` and `math-mock` both
//! *export* it with the same interface hash, so they are type-safe substitutes;
//! `doubler` exports a different `double` (`value->value`, no `math` interface)
//! and is correctly rejected — even though `pack compose` would blindly fuse it.
//! Linking adder against the mock and running it proves the mock takes effect.

use packr::abi::{decode, encode, Value};
use packr::{check_interface_link, compose, read_surface, ComposeSpec, LinkError, PackageSpec};
use wasmtime::{Engine, Linker, Module, Store};

fn asset(name: &str) -> Vec<u8> {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
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
fn hash_check_accepts_valid_providers_and_rejects_type_mismatch() {
    let adder = read_surface(&asset("adder_fixedbase.wasm")).unwrap();
    let real = read_surface(&asset("math_real_fixedbase.wasm")).unwrap();
    let mock = read_surface(&asset("math_mock_fixedbase.wasm")).unwrap();
    let doubler = read_surface(&asset("doubler_fixedbase.wasm")).unwrap();

    // Real + mock both PROVIDE the `math` interface adder REQUIRES (equal hashes).
    check_interface_link(&adder, "math", &real).expect("math-real satisfies adder.math");
    check_interface_link(&adder, "math", &mock).expect("math-mock satisfies adder.math");

    // doubler exports `double: value->value` under no `math` interface — rejected,
    // even though the graph ABI would let it marshal and `pack compose` would fuse it.
    match check_interface_link(&adder, "math", &doubler) {
        Err(LinkError::ProviderMissingInterface(iface)) => assert_eq!(iface, "math"),
        other => panic!("expected doubler to be rejected, got {other:?}"),
    }
}

#[test]
fn linking_against_the_mock_makes_the_mock_take_effect() {
    if !wasm_merge_available() {
        eprintln!("SKIP: wasm-merge (binaryen) not on PATH");
        return;
    }

    // real provider: adder.process(5) = 5*2 + 1 = 11
    assert_eq!(
        link_and_process("math_real_fixedbase.wasm", 5),
        Value::S64(11)
    );

    // mock provider: adder.process(n) = 100 + 1 = 101 for ANY n — the mock ran.
    assert_eq!(
        link_and_process("math_mock_fixedbase.wasm", 5),
        Value::S64(101)
    );
    assert_eq!(
        link_and_process("math_mock_fixedbase.wasm", 999),
        Value::S64(101)
    );
}

/// The linker in miniature: type-safe gate, then fuse adder + provider and run.
fn link_and_process(provider_asset: &str, input: i64) -> Value {
    let adder = asset("adder_fixedbase.wasm");
    let provider = asset(provider_asset);

    // Gate: the link must type-check before we fuse.
    check_interface_link(
        &read_surface(&adder).unwrap(),
        "math",
        &read_surface(&provider).unwrap(),
    )
    .expect("link must type-check");

    // Fuse: the provider named "math" satisfies adder's `math` import.
    let wasm = compose(&ComposeSpec::new(vec![
        PackageSpec::new("pack:alloc", asset("pack_alloc_module.wasm")),
        PackageSpec::new("math", provider),
        PackageSpec::new("adder", adder),
    ]))
    .expect("compose");

    run_process(&wasm, input)
}

fn run_process(wasm: &[u8], input: i64) -> Value {
    let engine = Engine::default();
    let module = Module::new(&engine, wasm).unwrap();
    let mut store = Store::new(&engine, ());
    let inst = Linker::new(&engine)
        .instantiate(&mut store, &module)
        .unwrap();
    if let Ok(c) = inst.get_typed_func::<(), ()>(&mut store, "__wasm_call_ctors") {
        c.call(&mut store, ()).unwrap();
    }
    let mem = inst
        .exports(&mut store)
        .filter_map(|e| e.into_memory())
        .next()
        .unwrap();

    let bytes = encode(&Value::S64(input)).unwrap();
    let pa = inst
        .get_typed_func::<i32, i32>(&mut store, "__pack_alloc")
        .unwrap();
    let in_ptr = pa.call(&mut store, bytes.len() as i32).unwrap();
    mem.write(&mut store, in_ptr as usize, &bytes).unwrap();
    let slots = pa.call(&mut store, 8).unwrap();
    let f = inst
        .get_typed_func::<(i32, i32, i32, i32), i32>(&mut store, "process")
        .unwrap();
    let status = f
        .call(&mut store, (in_ptr, bytes.len() as i32, slots, slots + 4))
        .unwrap();
    assert_eq!(status, 0);
    let mut sb = [0u8; 8];
    mem.read(&store, slots as usize, &mut sb).unwrap();
    let op = i32::from_le_bytes(sb[0..4].try_into().unwrap()) as usize;
    let ol = i32::from_le_bytes(sb[4..8].try_into().unwrap()) as usize;
    let mut out = vec![0u8; ol];
    mem.read(&store, op, &mut out).unwrap();
    decode(&out).unwrap()
}
