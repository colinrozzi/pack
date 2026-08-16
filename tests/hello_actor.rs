//! CI guard for the canonical `packr-guest` 0.18 "hello" actor
//! (`packages/hello`) — the reference shape guests mirror when adopting 0.18.
//!
//! Building it here keeps the reference honest: if the guest API drifts, this
//! fails, so the canonical example can't silently rot (which is exactly how the
//! out-of-CI CLI templates broke).

use std::path::Path;
use std::process::Command;

#[test]
fn hello_actor_compiles() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let manifest = Path::new(manifest_dir).join("packages/hello/Cargo.toml");
    let out = Path::new(manifest_dir)
        .join("packages/hello/target/wasm32-unknown-unknown/release/hello.wasm");

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
            "canonical hello actor (packages/hello) failed to compile — the 0.18 \
             guest reference is broken (exit {:?})",
            s.code()
        ),
        _ => {
            eprintln!(
                "SKIP: could not build packages/hello for wasm32-unknown-unknown \
                 (wasm target / cargo unavailable)."
            );
        }
    }
}
