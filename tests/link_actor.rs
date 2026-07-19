//! Host-importing actor — the residual surface is non-empty.
//!
//! `host-actor` imports TWO kinds of interface: a HELPER (`math`, satisfied by a
//! linked provider) and a HOST interface (`theater:simple/runtime`, satisfied by
//! the runtime at instantiate — no package provides it). Linking it against
//! `math-real` internalizes the helper call but leaves the host import standing:
//! the composite's residual surface is EXACTLY `theater:simple/runtime` + the
//! lifecycle exports. That mixed outcome is what the host-agnostic residual model
//! enables and the old `internalize` zero-imports gate forbade.
//!
//! Two proofs:
//!   1. surface-only — the residual imports are exactly the host interface;
//!   2. end-to-end — instantiate with the host PROVIDING the residual `log`
//!      import, call `process`, and the internalized `double` runs correctly.
//!
//! (packr itself is host-agnostic here — `theater:simple/runtime` is realistic
//! test data, not a special case; it survives because nothing in the compose set
//! exports it, not because of its name.)

use packr::abi::{decode, encode, Value};
use packr::{link, read_surface, Layout, LinkBinary, LinkEdge};
use wasmtime::{Caller, Engine, Linker, Module, Store};

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

/// Link host-actor + math-real (+ allocator), math wired to math-real.
fn link_host_actor() -> Vec<u8> {
    link(
        vec![
            LinkBinary {
                alias: "alloc".into(),
                wasm: asset("pack_alloc_module.wasm"),
                allocator: true,
            },
            LinkBinary {
                alias: "mathreal".into(),
                wasm: asset("math_real_fixedbase.wasm"),
                allocator: false,
            },
            LinkBinary {
                alias: "actor".into(),
                wasm: asset("host_actor_fixedbase.wasm"),
                allocator: false,
            },
        ],
        &[LinkEdge {
            from_alias: "actor".into(),
            from_interface: "math".into(),
            to_alias: "mathreal".into(),
            to_interface: "math".into(),
        }],
        Layout::default(),
    )
    .expect("link host-actor against math-real")
}

#[test]
fn residual_surface_is_exactly_the_host_interface() {
    if !wasm_merge_available() {
        eprintln!("SKIP: wasm-merge (binaryen) not on PATH");
        return;
    }
    let composite = link_host_actor();
    let surface = read_surface(&composite).expect("composite has one coherent __pack_types");

    // The lifecycle export survives the regen surgery.
    assert!(
        surface.arena.exports().iter().any(|f| f.name == "process"),
        "composite must export process"
    );

    // `math` is internalized (a link satisfied it); `theater:simple/runtime`
    // survives as the ONLY residual import — the universal self-contained shape.
    let residual: Vec<&str> = surface
        .import_hashes
        .iter()
        .map(|h| h.name.as_str())
        .collect();
    assert_eq!(
        residual,
        vec!["theater:simple/runtime"],
        "residual imports must be exactly the host interface (math internalized)"
    );
}

#[test]
fn host_actor_runs_with_the_host_providing_the_residual_import() {
    if !wasm_merge_available() {
        eprintln!("SKIP: wasm-merge (binaryen) not on PATH");
        return;
    }
    let composite = link_host_actor();

    // process(5) = double(5) + 1 = 11 — double() is the INTERNALIZED helper;
    // log() is the RESIDUAL host import, which we (the host) provide below.
    assert_eq!(run_process_with_host_log(&composite, 5), Value::S64(11));
    assert_eq!(run_process_with_host_log(&composite, 20), Value::S64(41));
}

/// Instantiate the composite and drive `process`, PROVIDING the residual
/// `theater:simple/runtime.log` host import — the loader's job in miniature.
fn run_process_with_host_log(wasm: &[u8], input: i64) -> Value {
    let engine = Engine::default();
    let module = Module::new(&engine, wasm).unwrap();
    let mut store = Store::new(&engine, ());

    // Satisfy the one residual import with a real host implementation that speaks
    // packr's cross-call ABI: (in_ptr, in_len, out_ptr_ptr, out_len_ptr) -> status.
    // It decodes the log message (proving marshalling), then guest-allocates a
    // unit result and returns status=1 (guest owns + frees the buffer).
    let mut linker = Linker::new(&engine);
    linker
        .func_wrap(
            "theater:simple/runtime",
            "log",
            |mut caller: Caller<'_, ()>,
             in_ptr: i32,
             in_len: i32,
             out_ptr_ptr: i32,
             out_len_ptr: i32|
             -> i32 {
                let mem = caller
                    .get_export("memory")
                    .and_then(|e| e.into_memory())
                    .expect("composite exports memory");

                // Decode the incoming log message — proves the residual host call
                // marshals correctly through the internalized allocator.
                let mut inbuf = vec![0u8; in_len as usize];
                mem.read(&caller, in_ptr as usize, &mut inbuf).unwrap();
                match decode(&inbuf) {
                    Ok(Value::String(s)) => assert_eq!(
                        s, "host-actor: processing",
                        "the residual host call must carry the actor's static string \
                         intact — regression guard for the .rodata/CGRF-strip bug"
                    ),
                    other => panic!("host log expected the actor's message, got {other:?}"),
                }

                // Return unit, guest-allocated (the real host's large-return path).
                let out = encode(&Value::Tuple(vec![])).unwrap();
                let pack_alloc = caller
                    .get_export("__pack_alloc")
                    .and_then(|e| e.into_func())
                    .expect("composite exports __pack_alloc")
                    .typed::<i32, i32>(&caller)
                    .unwrap();
                let optr = pack_alloc.call(&mut caller, out.len() as i32).unwrap();
                mem.write(&mut caller, optr as usize, &out).unwrap();
                mem.write(&mut caller, out_ptr_ptr as usize, &optr.to_le_bytes())
                    .unwrap();
                mem.write(
                    &mut caller,
                    out_len_ptr as usize,
                    &(out.len() as i32).to_le_bytes(),
                )
                .unwrap();
                1 // guest-allocated: the guest frees it
            },
        )
        .unwrap();

    let inst = linker.instantiate(&mut store, &module).unwrap();
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

/// The bundled `DEFAULT_ALLOCATOR_WASM` (no vendored blob) links a single actor
/// into a self-contained composite exactly like the on-disk allocator asset —
/// this is the version-locked allocator theater's fixture build uses.
#[test]
fn bundled_allocator_produces_a_self_contained_composite() {
    if !wasm_merge_available() {
        eprintln!("SKIP: wasm-merge (binaryen) not on PATH");
        return;
    }
    use packr::{link, read_surface, Layout, LinkBinary};

    // Single actor + the BUNDLED allocator, zero link edges — the no-helper
    // self-contained recipe. (host-actor's `math` import stays residual here
    // since it's unlinked; a real no-helper actor imports only host interfaces.)
    let composite = link(
        vec![
            LinkBinary {
                alias: "alloc".into(),
                wasm: packr::DEFAULT_ALLOCATOR_WASM.to_vec(),
                allocator: true,
            },
            LinkBinary {
                alias: "actor".into(),
                wasm: asset("host_actor_fixedbase.wasm"),
                allocator: false,
            },
        ],
        &[],
        Layout::default(),
    )
    .expect("link with the bundled allocator");

    // Self-contained: owns its memory, exports the marshalling ABI, and the
    // allocator is internalized (pack:alloc is NOT a residual import).
    let surface = read_surface(&composite).expect("composite metadata");
    let residual: Vec<&str> = surface
        .import_hashes
        .iter()
        .map(|h| h.name.as_str())
        .collect();
    assert!(
        !residual.iter().any(|n| n.contains("alloc")),
        "the bundled allocator must be internalized, got residual {residual:?}"
    );
}

/// Regression: the composite lifecycle-export trim must preserve an
/// INTERFACE-QUALIFIED lifecycle export (`theater:simple/actor.init`) — the shape
/// every real theater actor has. The trim keys on the RAW wasm export symbol
/// (`<interface>.<fn>`); building the allow-list from the bare arena fn name
/// (`init`) deleted it, so theater failed "Function not found" at spawn.
///
/// The trim runs only when the actor is a link *entry* — i.e. it links against a
/// provider (`[[link]]` edge), which is exactly the "actor imports a library"
/// case the bug was reported for. So this MUST link `lifecycle-actor` against a
/// `math` provider, not zero-edge (which skips the trim). `host-actor`'s bare
/// `.process` can't catch it either (qualified == bare there).
#[test]
fn link_preserves_interface_qualified_lifecycle_exports() {
    if !wasm_merge_available() {
        eprintln!("SKIP: wasm-merge (binaryen) not on PATH");
        return;
    }
    use packr::{link, Layout, LinkBinary, LinkEdge};

    // Actor links `math` → math-real: this makes the actor the entry, so the
    // lifecycle-export trim actually runs (the path that over-deleted the export).
    let composite = link(
        vec![
            LinkBinary {
                alias: "alloc".into(),
                wasm: packr::DEFAULT_ALLOCATOR_WASM.to_vec(),
                allocator: true,
            },
            LinkBinary {
                alias: "mathreal".into(),
                wasm: asset("math_real_fixedbase.wasm"),
                allocator: false,
            },
            LinkBinary {
                alias: "actor".into(),
                wasm: asset("lifecycle_actor_fixedbase.wasm"),
                allocator: false,
            },
        ],
        &[LinkEdge {
            from_alias: "actor".into(),
            from_interface: "math".into(),
            to_alias: "mathreal".into(),
            to_interface: "math".into(),
        }],
        Layout::default(),
    )
    .expect("link lifecycle-actor against math-real");

    // theater finds the entry by its RAW wasm export symbol, so assert at that
    // level (the regenerated metadata still lists it — only the raw export is
    // wrongly deleted). Module::exports() reads them without instantiating.
    let engine = Engine::default();
    let module = Module::new(&engine, &composite).expect("valid composite");
    let exports: Vec<String> = module.exports().map(|e| e.name().to_string()).collect();
    assert!(
        exports.iter().any(|e| e == "theater:simple/actor.init"),
        "composite must retain the interface-qualified lifecycle export, got {exports:?}"
    );
}

/// The link-time safety net: two members whose fixed-base data regions overlap
/// must be rejected up front, not emitted as a composite that silently traps at
/// runtime. `host-actor` and `lifecycle-actor` are both built at --global-base
/// 0xD0000, so their regions collide. (Fires before compose — no wasm-merge.)
#[test]
fn link_rejects_overlapping_member_regions() {
    use packr::{link, Layout, LinkBinary};
    let err = link(
        vec![
            LinkBinary {
                alias: "alloc".into(),
                wasm: packr::DEFAULT_ALLOCATOR_WASM.to_vec(),
                allocator: true,
            },
            LinkBinary {
                alias: "a".into(),
                wasm: asset("host_actor_fixedbase.wasm"),
                allocator: false,
            },
            LinkBinary {
                alias: "b".into(),
                wasm: asset("lifecycle_actor_fixedbase.wasm"),
                allocator: false,
            },
        ],
        &[],
        Layout::default(),
    )
    .err()
    .expect("overlapping members must be rejected at link time");
    assert!(
        format!("{err}").contains("overlap"),
        "error should name the region overlap, got: {err}"
    );
}

/// Regression for the prod mail-spine hang (0.10.2): a member whose `.rodata`
/// overruns the default `alloc_base` overwrites the bundled allocator's dlmalloc
/// structures, so the first allocation traps or spins forever. `link()` must
/// auto-fit the layout above the member so it composes AND allocates cleanly.
/// `bigrodata`'s `__data_end` (~0xFC756) exceeds the default `alloc_base`
/// (0xE0000); without the fit its first `__pack_alloc` traps.
#[test]
fn link_fits_layout_above_a_big_rodata_member() {
    if !wasm_merge_available() {
        eprintln!("SKIP: wasm-merge (binaryen) not on PATH");
        return;
    }
    use packr::{link, Layout, LinkBinary};

    let composite = link(
        vec![
            LinkBinary {
                alias: "alloc".into(),
                wasm: packr::DEFAULT_ALLOCATOR_WASM.to_vec(),
                allocator: true,
            },
            LinkBinary {
                alias: "actor".into(),
                wasm: asset("bigrodata_fixedbase.wasm"),
                allocator: false,
            },
        ],
        &[],
        Layout::default(), // would overrun without fit_layout
    )
    .expect("link big-rodata actor");

    // Instantiate (bigrodata has no residual host imports) and allocate — the
    // allocator is uncorrupted only because fit_layout raised it above the member.
    let engine = Engine::default();
    let module = Module::new(&engine, &composite).expect("valid composite");
    let mut store = Store::new(&engine, ());
    let inst = wasmtime::Linker::new(&engine)
        .instantiate(&mut store, &module)
        .expect("instantiate");
    if let Ok(c) = inst.get_typed_func::<(), ()>(&mut store, "__wasm_call_ctors") {
        c.call(&mut store, ()).unwrap();
    }
    let pa = inst
        .get_typed_func::<i32, i32>(&mut store, "__pack_alloc")
        .unwrap();
    let ptr = pa
        .call(&mut store, 64)
        .expect("first __pack_alloc must not trap on a fitted composite");
    assert!(ptr > 0, "allocator returned null on the fitted composite");
}
