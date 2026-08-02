//! Regression test for the LTO interior-`__pack_types` compose bug.
//!
//! When the entry component is built with `lto = true`, its CGRF metadata blob is
//! merged into the module's single `.rodata` data segment, so `__pack_types`'s
//! baked-in DATA_ADDR is an *interior* offset (segment base + rel), not the start
//! of a dedicated segment. The pre-fix `strip_internalized_imports_from_metadata`
//! only matched a segment whose base EXACTLY equalled DATA_ADDR, so it bailed with
//! "no active data segment found at metadata address ..." whenever the entry's sole
//! import was internalized (mesh's sm-smoke / sm-trivial repro).
//!
//! `comp-actor` happens to land its CGRF at `.rodata` offset 0 (rel == 0), so
//! `compose_actor.rs` never exercised the interior path. Linker ordering isn't
//! something we can pin, so rather than hope a fixture lands its metadata interior,
//! we take a real entry wasm and *force* the interior layout deterministically:
//! lower the CGRF segment's base by `PAD` and prepend `PAD` zero bytes, which keeps
//! every absolute address identical while making DATA_ADDR sit at rel == PAD. Then
//! we compose and assert the internalized `math` import was stripped — the exact
//! path that used to fail.

use packr::compose::{compose, Component, GraphLink};
use std::path::{Path, PathBuf};
use std::process::Command;
use walrus::{ir::Value as IrValue, ConstExpr, DataKind, Module};

fn build_component(pkg: &str) -> Option<PathBuf> {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let crate_name = pkg.replace('-', "_");
    let out = Path::new(manifest_dir).join(format!(
        "packages/{pkg}/target/wasm32-unknown-unknown/release/{crate_name}.wasm"
    ));
    let manifest = Path::new(manifest_dir).join(format!("packages/{pkg}/Cargo.toml"));

    let status = Command::new("cargo")
        .args([
            "build",
            "--manifest-path",
            manifest.to_str().unwrap(),
            "--target",
            "wasm32-unknown-unknown",
            "--release",
        ])
        .env(
            "RUSTFLAGS",
            "-C link-arg=--export-memory -C link-arg=--no-entry",
        )
        .status();

    match status {
        Ok(s) if s.success() && out.exists() => Some(out),
        _ if out.exists() => Some(out),
        _ => None,
    }
}

/// Rewrite `entry_wasm` so its CGRF metadata sits at a *non-zero* offset within its
/// data segment, mimicking the LTO-merged `.rodata` layout. Lowering the segment
/// base by `pad` and prepending `pad` zero bytes leaves the CGRF (and every other
/// byte) at its original absolute address, so `__pack_types`'s DATA_ADDR constant is
/// untouched and correct — it's just interior now. Returns the rewritten wasm and
/// the resulting `rel` (so the test can assert the transform actually took effect).
fn force_metadata_interior(entry_wasm: &[u8], pad: i32) -> (Vec<u8>, i32) {
    let mut module = Module::from_buffer(entry_wasm).expect("entry parses");

    // The CGRF segment is the active data segment whose bytes carry the `CGRF` magic.
    let cgrf_id = module
        .data
        .iter()
        .find(|d| {
            matches!(d.kind, DataKind::Active { .. }) && d.value.windows(4).any(|w| w == b"CGRF")
        })
        .map(|d| d.id())
        .expect("entry has a CGRF data segment");

    let seg = module.data.get_mut(cgrf_id);
    // Offset of the CGRF magic within this segment before the shift.
    let magic_off = seg
        .value
        .windows(4)
        .position(|w| w == b"CGRF")
        .expect("CGRF magic present") as i32;

    match &mut seg.kind {
        DataKind::Active {
            offset: ConstExpr::Value(IrValue::I32(base)),
            ..
        } => {
            *base -= pad;
        }
        _ => panic!("CGRF segment is not active-i32"),
    }
    let mut shifted = vec![0u8; pad as usize];
    shifted.extend_from_slice(&seg.value);
    seg.value = shifted;

    // rel of the CGRF within the (now larger) segment == original in-segment offset + pad.
    (module.emit_wasm(), magic_off + pad)
}

#[test]
fn compose_strips_internalized_import_when_metadata_is_interior() {
    let entry = match build_component("comp-actor") {
        Some(p) => std::fs::read(p).expect("read comp-actor wasm"),
        None => {
            eprintln!("SKIP: could not build comp-actor (wasm target / cargo unavailable).");
            return;
        }
    };
    let math = match build_component("math-real") {
        Some(p) => std::fs::read(p).expect("read math-real wasm"),
        None => {
            eprintln!("SKIP: could not build math-real (wasm target / cargo unavailable).");
            return;
        }
    };

    // Force the entry's metadata interior (rel > 0). This is the layout the pre-fix
    // strip could not locate.
    let (entry_interior, rel) = force_metadata_interior(&entry, 32);
    assert!(
        rel >= 32,
        "transform must place CGRF at a non-zero interior offset, got rel={rel}"
    );

    let components = vec![
        Component {
            name: "app".to_string(),
            wasm: entry_interior,
            entry: true,
        },
        Component {
            name: "math".to_string(),
            wasm: math,
            entry: false,
        },
    ];
    let links = vec![GraphLink {
        consumer: "app".to_string(),
        import_module: "math".to_string(),
        import_name: "double".to_string(),
        provider: "math".to_string(),
        export_name: "double".to_string(),
    }];

    // PRIMARY regression signal: pre-fix, `strip_internalized_imports_from_metadata`
    // bails here with "no active data segment ... at metadata address ..." because
    // DATA_ADDR is interior. Post-fix it locates the segment and slices at `rel`.
    // A wrong `rel` would decode garbage and still error, so reaching `Ok` also
    // proves the interior slice is correct.
    let composite = match compose(components, &links) {
        Ok(c) => c,
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("wasm-merge") || msg.contains("binaryen") {
                eprintln!("SKIP: {msg}");
                return;
            }
            panic!("compose over interior-metadata entry failed: {e:?}");
        }
    };

    // Internalization sanity, checked at the wasm-import level (robust — unlike the
    // static CGRF scan, which assumes the metadata sits at a segment's offset 0 and
    // so can't read this interior layout). The `math` import must be gone (satisfied
    // by the bridging shim), while the residual host import survives.
    let cmod = Module::from_buffer(&composite).expect("composite parses");
    let import_modules: Vec<String> = cmod.imports.iter().map(|i| i.module.clone()).collect();
    assert!(
        !import_modules.iter().any(|m| m == "math"),
        "internalized `math` wasm import must be deleted from the composite, got {import_modules:?}"
    );
    assert!(
        import_modules.iter().any(|m| m == "theater:simple/runtime"),
        "residual host import `theater:simple/runtime` must survive, got {import_modules:?}"
    );
}
