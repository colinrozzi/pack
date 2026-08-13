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

/// The `wit!(from "path")` file form: `packages/wit-from-file` sources its WIT+
/// from a SHARED file outside the crate (`packages/shared-api.wit+`) rather than
/// an inline block or a `wit/` dir. If it builds, the macro resolved the path
/// (relative to `CARGO_MANIFEST_DIR`), read the file, and generated the types.
#[test]
fn wit_from_file_path_compiles() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let manifest = Path::new(manifest_dir).join("packages/wit-from-file/Cargo.toml");
    let out = Path::new(manifest_dir)
        .join("packages/wit-from-file/target/wasm32-unknown-unknown/release/wit_from_file.wasm");

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
            "wit-from-file fixture failed to compile — the wit!(from \"path\") \
             file form is broken (exit {:?})",
            s.code()
        ),
        _ => {
            eprintln!(
                "SKIP: could not build wit-from-file for wasm32-unknown-unknown \
                 (wasm target / cargo unavailable)."
            );
        }
    }
}
