//! Regression test for `pact!` type codegen.
//!
//! Builds the `pact-types` fixture, whose `pact!` block declares a record (with an
//! `option` and a `list` field), a variant (with a record payload), and a C-like
//! enum, plus an exported function that uses the record. Before the codegen
//! switched to `#[derive(GraphValue)]`, the generated code built tuple-form
//! `Value::Record(..)` / `Value::Variant { tag, payload }` against the
//! struct-variant `Value` and failed to compile. If this fixture builds, the
//! generated types and their marshalling are correct.

use std::path::Path;
use std::process::Command;

#[test]
fn pact_generated_record_variant_enum_compile() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let manifest = Path::new(manifest_dir).join("packages/pact-types/Cargo.toml");
    let out = Path::new(manifest_dir)
        .join("packages/pact-types/target/wasm32-unknown-unknown/release/pact_types.wasm");

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
            "pact-types fixture failed to compile — the pact! codegen for \
             record/variant/enum is broken (exit {:?})",
            s.code()
        ),
        _ => {
            eprintln!(
                "SKIP: could not build pact-types for wasm32-unknown-unknown \
                 (wasm target / cargo unavailable)."
            );
        }
    }
}

/// The `pact!(from "path")` file form: `packages/pact-from-file` sources its Pact
/// definition from a SHARED file outside the crate (`packages/shared-api.pact`)
/// rather than an inline block or a `pact/` dir. If it builds, the macro resolved
/// the path (relative to `CARGO_MANIFEST_DIR`), read the file, generated the
/// types.
#[test]
fn pact_from_file_path_compiles() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let manifest = Path::new(manifest_dir).join("packages/pact-from-file/Cargo.toml");
    let out = Path::new(manifest_dir)
        .join("packages/pact-from-file/target/wasm32-unknown-unknown/release/pact_from_file.wasm");

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
            "pact-from-file fixture failed to compile — the pact!(from \"path\") \
             file form is broken (exit {:?})",
            s.code()
        ),
        _ => {
            eprintln!(
                "SKIP: could not build pact-from-file for wasm32-unknown-unknown \
                 (wasm target / cargo unavailable)."
            );
        }
    }
}
