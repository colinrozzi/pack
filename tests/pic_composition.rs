//! 1b integration: a real package runs on the in-wasm allocator via PIC
//! dynamic linking — the proof that ties the whole allocator decoupling
//! together.
//!
//! Both the allocator and the `echo` package are PIC side modules. This test
//! plays the role of the runtime's dynamic loader: one shared memory + table,
//! disjoint `__memory_base`/stack per module, the allocator's heap region via
//! GOT globals, and `pack:alloc` wired to the allocator's exports.

use packr::abi::{decode, encode, Value, ValueType};
use std::path::Path;
use wasmtime::{
    Engine, Global, GlobalType, Instance, Linker, Memory, MemoryType, Module, Mutability, Ref,
    RefType, Store, Table, TableType, Val, ValType,
};

// Shared-memory layout (bytes):
//   [0, PKG_BASE)          allocator BSS (tiny)
//   [PKG_BASE, STACK_TOP)  package data (low) + package stack (grows down from STACK_TOP)
//   [HEAP_BASE, HEAP_END)  heap, managed by the allocator (64 KiB gap after the stack)
const MEM_PAGES: u32 = 64; // 4 MiB
const PKG_BASE: i32 = 0x1_0000;
const PKG_STACK_TOP: i32 = 0x10_0000;
const HEAP_BASE: i32 = 0x11_0000;
const HEAP_END: i32 = 0x40_0000;
const TABLE_MIN: u32 = 32;

fn read_wasm(rel: &str) -> Vec<u8> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
    std::fs::read(&p)
        .unwrap_or_else(|e| panic!("read {}: {} — build the packages first", p.display(), e))
}

fn const_i32(store: &mut Store<()>, v: i32) -> Global {
    Global::new(
        store,
        GlobalType::new(ValType::I32, Mutability::Const),
        Val::I32(v),
    )
    .unwrap()
}
fn var_i32(store: &mut Store<()>, v: i32) -> Global {
    Global::new(
        store,
        GlobalType::new(ValType::I32, Mutability::Var),
        Val::I32(v),
    )
    .unwrap()
}

/// Play the dynamic loader: instantiate allocator + package sharing one memory
/// and table, and return the package instance + the shared memory + table.
fn load_group(store: &mut Store<()>, engine: &Engine) -> (Instance, Memory, Table) {
    let alloc_mod = Module::new(
        engine,
        read_wasm(
            "packages/pack-alloc/target/wasm32-unknown-unknown/release/pack_alloc_module.wasm",
        ),
    )
    .expect("compile allocator");
    let pkg_mod = Module::new(
        engine,
        read_wasm("packages/echo-pic/target/wasm32-unknown-unknown/release/echo_pic_package.wasm"),
    )
    .expect("compile echo-pic");

    let mem = Memory::new(&mut *store, MemoryType::new(MEM_PAGES, None)).expect("shared memory");
    let table = Table::new(
        &mut *store,
        TableType::new(RefType::FUNCREF, TABLE_MIN, None),
        Ref::Func(None),
    )
    .expect("shared table");

    // Allocator side module. Place its data/mstate in the protected gap
    // (0x100000..0x110000) between the package stack and the heap, so nothing
    // else can zero its control struct (granularity etc.).
    let a_membase = const_i32(&mut *store, 0x10_8000);
    let a_tablebase = const_i32(&mut *store, 0);
    let heap_base = var_i32(&mut *store, HEAP_BASE);
    let heap_end = var_i32(&mut *store, HEAP_END);
    let mut al = Linker::new(engine);
    al.define(&*store, "env", "memory", mem).unwrap();
    al.define(&*store, "env", "__memory_base", a_membase)
        .unwrap();
    al.define(&*store, "env", "__table_base", a_tablebase)
        .unwrap();
    al.define(&*store, "GOT.mem", "__heap_base", heap_base)
        .unwrap();
    al.define(&*store, "GOT.mem", "__heap_end", heap_end)
        .unwrap();
    let alloc_inst = al
        .instantiate(&mut *store, &alloc_mod)
        .expect("instantiate allocator");
    let alloc_fn = alloc_inst.get_export(&mut *store, "alloc").unwrap();
    let dealloc_fn = alloc_inst.get_export(&mut *store, "dealloc").unwrap();

    // Package side module (disjoint base + own stack).
    let p_membase = const_i32(&mut *store, PKG_BASE);
    let p_tablebase = const_i32(&mut *store, 0);
    let p_sp = var_i32(&mut *store, PKG_STACK_TOP);
    // GOT slot for __data_end (real actors import it); resolved after instantiation.
    let p_data_end = var_i32(&mut *store, PKG_BASE);
    let mut pl = Linker::new(engine);
    pl.define(&*store, "env", "memory", mem).unwrap();
    pl.define(&*store, "env", "__indirect_function_table", table)
        .unwrap();
    pl.define(&*store, "env", "__stack_pointer", p_sp).unwrap();
    pl.define(&*store, "env", "__memory_base", p_membase)
        .unwrap();
    pl.define(&*store, "env", "__table_base", p_tablebase)
        .unwrap();
    pl.define(&*store, "GOT.mem", "__data_end", p_data_end)
        .unwrap();
    pl.define(&*store, "pack:alloc", "alloc", alloc_fn).unwrap();
    pl.define(&*store, "pack:alloc", "dealloc", dealloc_fn)
        .unwrap();
    let pkg = pl
        .instantiate(&mut *store, &pkg_mod)
        .expect("instantiate package");

    // PIC init, mirroring the runtime loader: resolve GOT.mem.__data_end to
    // __memory_base + the module's exported offset, apply data relocs (stored
    // pointer fixups — __wasm_call_ctors is empty and does NOT do this), then ctors.
    if let Some(g) = pkg.get_global(&mut *store, "__data_end") {
        if let Val::I32(off) = g.get(&mut *store) {
            p_data_end
                .set(&mut *store, Val::I32(PKG_BASE + off))
                .unwrap();
        }
    }
    if let Ok(relocs) = pkg.get_typed_func::<(), ()>(&mut *store, "__wasm_apply_data_relocs") {
        relocs.call(&mut *store, ()).unwrap();
    }
    pkg.get_typed_func::<(), ()>(&mut *store, "__wasm_call_ctors")
        .expect("__wasm_call_ctors")
        .call(&mut *store, ())
        .expect("run ctors");

    (pkg, mem, table)
}

/// Probe: multiple *concurrently live* allocations must be distinct and
/// non-overlapping (echo keeps many live at once; the earlier alloc_module test
/// only ever had one).
#[test]
fn allocator_hands_out_disjoint_concurrent_blocks() {
    let engine = Engine::default();
    let mut store = Store::new(&engine, ());
    let (pkg, _mem, table) = load_group(&mut store, &engine);

    // The active elem segment must have populated the shared table at __table_base=0
    // with the package's 13 funcref entries (drop/fmt/Write shims used by indirect
    // calls). A null entry here means an indirect call would trap.
    for i in 0..13u64 {
        let r = table.get(&mut store, i).expect("table index in range");
        let func = r.as_func().expect("funcref");
        assert!(
            func.is_some(),
            "shared table slot {i} is NULL — not populated"
        );
    }

    let pack_alloc = pkg
        .get_typed_func::<i32, i32>(&mut store, "__pack_alloc")
        .unwrap();

    let mut blocks: Vec<(i32, i32)> = Vec::new(); // (ptr, size)
    for i in 0..16u32 {
        let sz = 16 + (i as i32) * 24;
        let p = pack_alloc.call(&mut store, sz).expect("alloc");
        assert!(
            p >= HEAP_BASE && p < HEAP_END,
            "block {i} ptr {p:#x} outside heap"
        );
        for (q, qsz) in &blocks {
            let (a0, a1) = (p, p + sz);
            let (b0, b1) = (*q, *q + *qsz);
            assert!(
                a1 <= b0 || b1 <= a0,
                "block {p:#x}+{sz} overlaps {q:#x}+{qsz}"
            );
        }
        blocks.push((p, sz));
    }
}

fn round_trip(input: Value) {
    // Sanity: must round-trip host-side first, so any failure below is PIC, not ABI.
    assert_eq!(
        decode(&encode(&input).unwrap()).unwrap(),
        input,
        "host-side abi round-trip"
    );

    let engine = Engine::default();
    let mut store = Store::new(&engine, ());
    let (pkg, mem, _table) = load_group(&mut store, &engine);

    let pack_alloc = pkg
        .get_typed_func::<i32, i32>(&mut store, "__pack_alloc")
        .unwrap();
    let echo = pkg
        .get_typed_func::<(i32, i32, i32, i32), i32>(&mut store, "echo")
        .unwrap();

    let input_bytes = encode(&input).unwrap();
    let in_ptr = pack_alloc
        .call(&mut store, input_bytes.len() as i32)
        .unwrap();
    mem.write(&mut store, in_ptr as usize, &input_bytes)
        .unwrap();
    let slots = pack_alloc.call(&mut store, 8).unwrap();
    let status = echo
        .call(
            &mut store,
            (in_ptr, input_bytes.len() as i32, slots, slots + 4),
        )
        .expect("echo call");
    assert_eq!(status, 0, "echo returned error status");

    let read_i32 = |store: &Store<()>, at: i32| {
        let mut b = [0u8; 4];
        mem.read(store, at as usize, &mut b).unwrap();
        i32::from_le_bytes(b)
    };
    let out_ptr = read_i32(&store, slots);
    let out_len = read_i32(&store, slots + 4);
    let mut out = vec![0u8; out_len as usize];
    mem.read(&store, out_ptr as usize, &mut out).unwrap();
    let output = decode(&out).unwrap();

    assert_eq!(
        output, input,
        "value did not round-trip through the in-wasm allocator"
    );
}

#[test]
fn pic_package_round_trips_string() {
    round_trip(Value::String(
        "PIC shared in-wasm allocator works".to_string(),
    ));
}

#[test]
fn pic_package_round_trips_list_s64() {
    round_trip(Value::List {
        elem_type: ValueType::S64,
        items: vec![Value::S64(1), Value::S64(2), Value::S64(3), Value::S64(-9)],
    });
}

#[test]
fn pic_single_long_string() {
    round_trip(Value::String(
        "a-longer-string-to-force-heap-allocation".to_string(),
    ));
}

#[test]
fn pic_list_two_short_strings() {
    round_trip(Value::List {
        elem_type: ValueType::String,
        items: vec![
            Value::String("ab".to_string()),
            Value::String("cd".to_string()),
        ],
    });
}

// Regression: multiple heap strings in a structure. This used to corrupt — the
// allocator's dlmalloc `mstate` sat at low memory (__memory_base=0) and got
// zeroed (granularity -> 0 -> divide-by-zero in `free`) during the package's
// multi-allocation encode/decode. Fixed by giving the allocator its own
// protected `__memory_base` region (see `load_group`).
#[test]
fn pic_package_round_trips_nested() {
    round_trip(Value::List {
        elem_type: ValueType::String,
        items: vec![
            Value::String("alpha".to_string()),
            Value::String("a-longer-string-to-force-heap-allocation".to_string()),
        ],
    });
}

/// The whole point: this goes through the PUBLIC runtime API (`Runtime` ->
/// `load_module` -> `instantiate_pic` -> `call_with_value`), not a hand-rolled
/// loader. The runtime embeds the allocator module and does the PIC linking +
/// per-instance memory marshalling internally.
#[test]
fn runtime_api_runs_pic_package() {
    let wasm =
        read_wasm("packages/echo-pic/target/wasm32-unknown-unknown/release/echo_pic_package.wasm");
    let runtime = packr::Runtime::new();
    let module = runtime.load_module(&wasm).expect("load echo-pic");
    let mut inst = module.instantiate_pic().expect("instantiate_pic");

    let input = Value::List {
        elem_type: ValueType::String,
        items: vec![
            Value::String("through".to_string()),
            Value::String("the-runtime-api".to_string()),
        ],
    };
    let out = inst.call_with_value("echo", &input).expect("call echo");
    assert_eq!(
        out, input,
        "PIC package did not round-trip via the runtime API"
    );

    // And the transform export (doubles S64s) through the same instance path.
    let doubled = inst
        .call_with_value(
            "transform",
            &Value::List {
                elem_type: ValueType::S64,
                items: vec![Value::S64(21), Value::S64(-5)],
            },
        )
        .expect("call transform");
    assert_eq!(
        doubled,
        Value::List {
            elem_type: ValueType::S64,
            items: vec![Value::S64(42), Value::S64(-10)],
        }
    );
}

/// Regression for PIC static-data relocation (Gap 2 from theater-dev's soak): the
/// loader must call `__wasm_apply_data_relocs`, else stored data pointers — e.g.
/// `format!()`'s `&'static str` fragments — keep raw offsets and read as blank.
/// echo-pic's `describe` returns `format!("n={n}!")`; a broken loader yields just
/// the interpolated number with the "n=" / "!" literal fragments gone.
#[test]
fn pic_static_string_data_is_relocated() {
    let wasm =
        read_wasm("packages/echo-pic/target/wasm32-unknown-unknown/release/echo_pic_package.wasm");
    let runtime = packr::Runtime::new();
    let module = runtime.load_module(&wasm).expect("load echo-pic");
    let mut inst = module.instantiate_pic().expect("instantiate_pic");

    let out = inst
        .call_with_value("describe", &Value::S64(7))
        .expect("call describe");
    assert_eq!(
        out,
        Value::String("n=7!".to_string()),
        "static &'static str fragments dropped under PIC — loader did not apply data relocations"
    );
}

/// Regression for Gap 3 (theater-dev soak): a real actor references `__data_end`
/// cross-crate (via its dependency tree), which under `-shared` is a
/// `GOT.mem.__data_end` import the loader must resolve — echo-pic's `data_end_addr`
/// forces the same import. Asserts the package both instantiates AND that the
/// resolved address lands in the package data region (i.e. the loader pointed
/// GOT.mem.__data_end at `__memory_base` + the module's exported offset, not a
/// dummy). Without the fix, instantiation fails with an unknown-import error.
#[test]
fn pic_got_data_end_is_resolved() {
    let wasm =
        read_wasm("packages/echo-pic/target/wasm32-unknown-unknown/release/echo_pic_package.wasm");
    let runtime = packr::Runtime::new();
    let module = runtime.load_module(&wasm).expect("load echo-pic");
    let mut inst = module
        .instantiate_pic()
        .expect("instantiate_pic must satisfy GOT.mem.__data_end");

    let out = inst
        .call_with_value("data_end_addr", &Value::S64(0))
        .expect("call data_end_addr");
    let addr = match out {
        Value::S64(a) => a,
        other => panic!("expected S64 address, got {other:?}"),
    };
    // PIC layout: package data region is [PKG_BASE=0x10000, PKG_STACK_TOP=0x100000).
    assert!(
        (0x1_0000..0x10_0000).contains(&addr),
        "GOT.mem.__data_end resolved to {addr:#x}, outside the package data region"
    );
}

/// The theater path: a PIC package that IMPORTS a host function, driven through
/// the ASYNC runtime API with a typed host function. Exercises host-function
/// arg/return marshalling under PIC (host reads the arg out of the shared
/// memory into a Value, doubles it, and writes the result back via the guest
/// allocator — the exact seam theater rides).
#[tokio::test]
async fn async_host_fn_marshals_under_pic() {
    use packr::runtime::{AsyncRuntime, Ctx};

    let wasm = read_wasm(
        "packages/hostcall-pic/target/wasm32-unknown-unknown/release/hostcall_pic_package.wasm",
    );
    let rt = AsyncRuntime::new();
    let module = rt.load_module(&wasm).expect("load hostcall-pic");
    let mut inst = module
        .instantiate_with_host_async((), |builder| {
            builder.interface("host")?.func_typed(
                "double_it",
                |_ctx: &mut Ctx<'_, ()>, v: Value| -> Value {
                    match v {
                        Value::S64(n) => Value::S64(n * 2),
                        other => other,
                    }
                },
            )?;
            Ok(())
        })
        .await
        .expect("instantiate hostcall-pic with host fn");

    let out = inst
        .call_with_value_async("run", &Value::S64(21))
        .await
        .expect("call run");
    assert_eq!(
        out,
        Value::S64(42),
        "host-function result did not marshal back under PIC"
    );
}

/// The 32KB blocker fix: an ASYNC host function returning a LARGE Value (>32KB)
/// guest-allocates its return buffer (awaits the guest `__pack_alloc`), so it is
/// unbounded — theater's get-chain / wat-to-wasm / store.get path. Also proves
/// re-entrancy: an async host fn calling back into the guest allocator in an
/// async store.
#[tokio::test]
async fn async_host_fn_large_return_guest_allocates() {
    use packr::runtime::{AsyncCtx, AsyncRuntime};

    let wasm = read_wasm(
        "packages/hostcall-pic/target/wasm32-unknown-unknown/release/hostcall_pic_package.wasm",
    );
    // ~8K S64s: encodes to well over 32 KiB — impossible via the fixed buffer.
    let big = Value::List {
        elem_type: ValueType::S64,
        items: (0..8192i64).map(Value::S64).collect(),
    };
    assert!(
        encode(&big).unwrap().len() > 32 * 1024,
        "test value must exceed the fixed-buffer cap"
    );

    let rt = AsyncRuntime::new();
    let module = rt.load_module(&wasm).expect("load hostcall-pic");
    let for_host = big.clone();
    let mut inst = module
        .instantiate_with_host_async((), move |builder| {
            let for_host = for_host.clone();
            builder.interface("host")?.func_async(
                "double_it",
                move |_ctx: AsyncCtx<()>, _v: Value| {
                    let out = for_host.clone();
                    async move { out }
                },
            )?;
            Ok(())
        })
        .await
        .expect("instantiate with async host fn");

    let out = inst
        .call_with_value_async("run", &Value::S64(0))
        .await
        .expect("call run");
    assert_eq!(
        out, big,
        "large async host return did not guest-allocate + marshal under PIC"
    );
}

/// Regression for the guest-allocated-return *leak*: `__import_impl` must free
/// the host-returned buffer after decoding, or every large async return leaks a
/// copy and a long-lived actor's memory climbs until OOM. We drive many large
/// async returns and watch the guest's linear-memory high-water mark: once warmed
/// up, a freed-and-reused buffer keeps memory flat, whereas a leak forces a fresh
/// `memory.grow` of ~one return per call. Asserting the post-warmup growth stays
/// below even a single return buffer catches the leak deterministically (memory
/// is `max=None`, so a leak grows rather than errors — it must be observed, not
/// exhausted).
#[tokio::test]
async fn async_host_fn_large_returns_do_not_leak() {
    use packr::runtime::{AsyncCtx, AsyncRuntime};

    let wasm = read_wasm(
        "packages/hostcall-pic/target/wasm32-unknown-unknown/release/hostcall_pic_package.wasm",
    );
    // ~256 KiB encoded: a leaked copy per call is unmistakable against the flat
    // steady-state of the fixed working set.
    let big = Value::List {
        elem_type: ValueType::S64,
        items: (0..32768i64).map(Value::S64).collect(),
    };
    let return_len = encode(&big).unwrap().len();
    assert!(
        return_len > 128 * 1024,
        "leak-test value must be large so a per-call leak dominates the working set"
    );

    let rt = AsyncRuntime::new();
    let module = rt.load_module(&wasm).expect("load hostcall-pic");
    let for_host = big.clone();
    let mut inst = module
        .instantiate_with_host_async((), move |builder| {
            let for_host = for_host.clone();
            builder.interface("host")?.func_async(
                "double_it",
                move |_ctx: AsyncCtx<()>, _v: Value| {
                    let out = for_host.clone();
                    async move { out }
                },
            )?;
            Ok(())
        })
        .await
        .expect("instantiate with async host fn");

    // Warm up so the heap reaches its steady-state high-water mark.
    for _ in 0..5 {
        let out = inst
            .call_with_value_async("run", &Value::S64(0))
            .await
            .unwrap();
        assert_eq!(out, big);
    }
    let baseline = inst.memory_size().expect("memory size");

    // Many more calls. If each leaked its return, memory climbs ~return_len/call
    // (dlmalloc can't reuse leaked blocks, so it grows the linear memory).
    const N: usize = 60;
    for i in 0..N {
        let out = inst
            .call_with_value_async("run", &Value::S64(0))
            .await
            .unwrap();
        assert_eq!(out, big, "call {i} returned the wrong value");
    }
    let growth = inst
        .memory_size()
        .expect("memory size")
        .saturating_sub(baseline);

    // Freed + reused: growth ~0. Leaked: growth ~= N * return_len (~15 MiB). The
    // buffer is freed iff post-warmup growth is under a single return.
    assert!(
        growth < return_len,
        "guest memory grew {growth} bytes over {N} calls — that is >= one {return_len}-byte \
         return, so host-returned buffers are leaking instead of being freed"
    );
}
