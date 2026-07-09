//! 1b: the in-wasm allocator as a PIC side module, driven by a minimal
//! "dynamic loader" (the mechanism the runtime will use).
//!
//! The allocator is compiled position-independent, so it imports the five
//! side-module symbols — `env.memory`, `env.__memory_base`, `env.__table_base`,
//! and GOT `__heap_base`/`__heap_end`. The loader creates one shared linear
//! memory, assigns this module a data base (0) and a heap region above its
//! tiny BSS, and provides those imports. This proves:
//!   1. a PIC allocator runs at a runtime-assigned base, and
//!   2. it reclaims memory (real `free`) — memory stays flat under churn.
//!
//! Assigning *disjoint* bases per module is exactly what lets an allocator and
//! a package share one memory without their data/stacks colliding (the thing a
//! non-PIC module cannot do).

use std::path::Path;
use wasmtime::{
    Engine, Global, GlobalType, Instance, Linker, Memory, MemoryType, Module, Mutability, Store,
    Val, ValType,
};

const PAGES: u64 = 16; // 1 MiB shared memory
const HEAP_BASE: i32 = 0x1_0000; // 64 KiB — safely above the allocator's BSS
const HEAP_END: i32 = 0x10_0000; // 1 MiB

fn alloc_module_wasm() -> Vec<u8> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("packages/pack-alloc/target/wasm32-unknown-unknown/release/pack_alloc_module.wasm");
    std::fs::read(&p).unwrap_or_else(|e| {
        panic!(
            "read alloc module {}: {} — build it: \
             cd packages/pack-alloc && cargo build --target wasm32-unknown-unknown --release",
            p.display(),
            e
        )
    })
}

/// Minimal in-process dynamic loader for the PIC allocator side module.
fn load_pic_allocator(
    store: &mut Store<()>,
    engine: &Engine,
    module: &Module,
) -> (Instance, Memory) {
    let mem = Memory::new(&mut *store, MemoryType::new(PAGES as u32, None)).expect("shared memory");

    let const_i32 = |store: &mut Store<()>, v: i32| {
        Global::new(
            store,
            GlobalType::new(ValType::I32, Mutability::Const),
            Val::I32(v),
        )
        .unwrap()
    };
    let var_i32 = |store: &mut Store<()>, v: i32| {
        Global::new(
            store,
            GlobalType::new(ValType::I32, Mutability::Var),
            Val::I32(v),
        )
        .unwrap()
    };

    let memory_base = const_i32(&mut *store, 0); // allocator BSS at offset 0
    let table_base = const_i32(&mut *store, 0);
    let heap_base = var_i32(&mut *store, HEAP_BASE);
    let heap_end = var_i32(&mut *store, HEAP_END);

    let mut linker = Linker::new(engine);
    linker.define(&*store, "env", "memory", mem).unwrap();
    linker
        .define(&*store, "env", "__memory_base", memory_base)
        .unwrap();
    linker
        .define(&*store, "env", "__table_base", table_base)
        .unwrap();
    linker
        .define(&*store, "GOT.mem", "__heap_base", heap_base)
        .unwrap();
    linker
        .define(&*store, "GOT.mem", "__heap_end", heap_end)
        .unwrap();

    let instance = linker
        .instantiate(&mut *store, module)
        .expect("instantiate PIC allocator");
    (instance, mem)
}

#[test]
fn pic_allocator_allocates_in_assigned_heap_and_stays_bounded() {
    let engine = Engine::default();
    let module = Module::new(&engine, alloc_module_wasm()).expect("compile alloc module");
    let mut store = Store::new(&engine, ());
    let (instance, mem) = load_pic_allocator(&mut store, &engine, &module);

    let alloc = instance
        .get_typed_func::<(i32, i32), i32>(&mut store, "alloc")
        .expect("alloc export");
    let dealloc = instance
        .get_typed_func::<(i32, i32, i32), ()>(&mut store, "dealloc")
        .expect("dealloc export");

    // Allocations must land in the loader-assigned heap region.
    let p = alloc.call(&mut store, (128, 8)).expect("alloc");
    assert!(
        p >= HEAP_BASE && p < HEAP_END,
        "allocation {p:#x} outside assigned heap [{HEAP_BASE:#x}, {HEAP_END:#x})"
    );
    dealloc.call(&mut store, (p, 128, 8)).expect("dealloc");

    // Real free: only one live block at a time, so memory must stay flat.
    let mut baseline = 0usize;
    for i in 0..5000u32 {
        let sz = 16 + ((i % 64) as i32) * 64;
        let q = alloc.call(&mut store, (sz, 8)).expect("alloc call");
        assert!(q != 0, "alloc returned null at iter {i}");
        dealloc.call(&mut store, (q, sz, 8)).expect("dealloc call");
        if i == 500 {
            baseline = mem.data_size(&store);
        }
    }
    assert_eq!(
        mem.data_size(&store),
        baseline,
        "linear memory grew under alloc+free churn: allocator not reclaiming"
    );
}

#[test]
fn pic_allocator_reuses_freed_block() {
    let engine = Engine::default();
    let module = Module::new(&engine, alloc_module_wasm()).expect("compile alloc module");
    let mut store = Store::new(&engine, ());
    let (instance, _mem) = load_pic_allocator(&mut store, &engine, &module);
    let alloc = instance
        .get_typed_func::<(i32, i32), i32>(&mut store, "alloc")
        .unwrap();
    let dealloc = instance
        .get_typed_func::<(i32, i32, i32), ()>(&mut store, "dealloc")
        .unwrap();

    let a = alloc.call(&mut store, (128, 8)).unwrap();
    dealloc.call(&mut store, (a, 128, 8)).unwrap();
    let b = alloc.call(&mut store, (128, 8)).unwrap();
    assert_eq!(
        a, b,
        "freeing then re-allocating the same size should reuse the block"
    );
}
