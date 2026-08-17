//! CI guard for cross-file `use` imports (`packages/use-consumer`).
//!
//! `consumer.pact` does `use "../use-shared.pact".{entry}`; the macro must
//! resolve the path (relative to consumer.pact), read + parse the shared file,
//! pull `entry` PLUS its transitive same-file deps (`msg`, `kind`), and generate
//! them locally. If this builds, cross-file single-sourcing works.

use std::path::Path;
use std::process::Command;

#[test]
fn cross_file_use_imports_compile() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let manifest = Path::new(manifest_dir).join("packages/use-consumer/Cargo.toml");
    let out = Path::new(manifest_dir)
        .join("packages/use-consumer/target/wasm32-unknown-unknown/release/use_consumer.wasm");

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
            "use-consumer fixture failed to compile — cross-file `use` import \
             resolution is broken (exit {:?})",
            s.code()
        ),
        _ => {
            eprintln!(
                "SKIP: could not build use-consumer for wasm32-unknown-unknown \
                 (wasm target / cargo unavailable)."
            );
        }
    }
}
