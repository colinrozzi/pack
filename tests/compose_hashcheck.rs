//! Hash-checked links: `compose` rejects a link whose two sides disagree on the
//! interface's Merkle hash, and accepts one where they match.
//!
//! `comp-app` imports `math { double: func(n: s64) -> s64 }`.
//!   - `math-real` exports `math { double: func(n: s64) -> s64 }` — hashes MATCH,
//!     compose succeeds.
//!   - `math-wrong` exports `math { double: func(n: s32) -> s32 }` — same names,
//!     different signature => different interface hash, compose is REJECTED at
//!     compose time (instead of silently mis-marshalling s64 args into an s32
//!     callee at runtime).

use packr::compose::{compose, Component, GraphLink};
use std::path::{Path, PathBuf};
use std::process::Command;

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

fn read(pkg: &str) -> Option<Vec<u8>> {
    let p = build_component(pkg)?;
    Some(std::fs::read(p).expect("read wasm"))
}

fn app_math_link() -> Vec<GraphLink> {
    vec![GraphLink {
        consumer: "app".to_string(),
        import_module: "math".to_string(),
        import_name: "double".to_string(),
        provider: "math".to_string(),
        export_name: "double".to_string(),
    }]
}

fn components(app: Vec<u8>, provider: Vec<u8>) -> Vec<Component> {
    vec![
        Component {
            name: "app".to_string(),
            wasm: app,
            entry: true,
        },
        Component {
            name: "math".to_string(),
            wasm: provider,
            entry: false,
        },
    ]
}

#[test]
fn matching_interface_hashes_compose_ok() {
    let (Some(app), Some(good)) = (read("comp-app"), read("math-real")) else {
        eprintln!("SKIP: fixtures unavailable (wasm target / cargo).");
        return;
    };

    match compose(components(app, good), &app_math_link()) {
        Ok(_) => {} // matching hashes: the link is accepted.
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("wasm-merge") || msg.contains("binaryen") {
                eprintln!("SKIP: {msg}");
                return;
            }
            panic!("compose of matching interfaces must succeed, got: {e:?}");
        }
    }
}

#[test]
fn mismatched_interface_hashes_are_rejected() {
    let (Some(app), Some(wrong)) = (read("comp-app"), read("math-wrong")) else {
        eprintln!("SKIP: fixtures unavailable (wasm target / cargo).");
        return;
    };

    let err = match compose(components(app, wrong), &app_math_link()) {
        Ok(_) => panic!(
            "compose must REJECT a link whose consumer imports `math` as s64->s64 \
             but whose provider exports it as s32->s32 (interface hashes differ)"
        ),
        Err(e) => e.to_string(),
    };

    // The rejection must be the hash check (not a merge/toolchain error).
    assert!(
        err.contains("hash-checked link rejected") && err.contains("different hash"),
        "expected a hash-mismatch rejection, got: {err}"
    );
    // And it must name the offending interface.
    assert!(
        err.contains("interface `math`"),
        "rejection should name the `math` interface, got: {err}"
    );
}
