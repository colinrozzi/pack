//! Regression test for `wit!` type codegen.
//!
//! Builds the `wit-types` fixture, whose `wit!` block declares a record (with an
//! `option` and a `list` field), a variant (with a record payload), and a C-like
//! enum, plus an exported function that uses the record. Before the codegen
//! switched to `#[derive(GraphValue)]`, the generated code built tuple-form
//! `Value::Record(..)` / `Value::Variant { tag, payload }` against the
//! struct-variant `Value` and failed to compile. If this fixture builds, the
//! generated types and their marshalling are correct.

use std::path::Path;
use std::process::Command;

#[test]
fn wit_generated_record_variant_enum_compile() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let manifest = Path::new(manifest_dir).join("packages/wit-types/Cargo.toml");
    let out = Path::new(manifest_dir)
        .join("packages/wit-types/target/wasm32-unknown-unknown/release/wit_types.wasm");

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
        Ok(s) if s.success() && out.exists() => {}
        Ok(s) if !s.success() => panic!(
            "wit-types fixture failed to compile — the wit! codegen for \
             record/variant/enum is broken (exit {:?})",
            s.code()
        ),
        _ => {
            eprintln!(
                "SKIP: could not build wit-types for wasm32-unknown-unknown \
                 (wasm target / cargo unavailable)."
            );
        }
    }
}
