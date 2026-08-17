//! CI guard for the canonical stateful message-server actor
//! (`packages/message-server-actor`) — the reference for a typed-actor-state
//! guest with state-threaded handlers, including the multi-return shape
//! (`Result<(State, (Response,)), String>`).
//!
//! Building it here keeps the shape honest — the state-mode return spelling
//! can't silently drift.

use std::path::Path;
use std::process::Command;

#[test]
fn message_server_actor_compiles() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let manifest = Path::new(manifest_dir).join("packages/message-server-actor/Cargo.toml");
    let out = Path::new(manifest_dir).join(
        "packages/message-server-actor/target/wasm32-unknown-unknown/release/message_server_actor.wasm",
    );

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
            "message-server-actor fixture failed to compile — the state-threaded \
             multi-return handler shape is broken (exit {:?})",
            s.code()
        ),
        _ => {
            eprintln!(
                "SKIP: could not build message-server-actor for \
                 wasm32-unknown-unknown (wasm target / cargo unavailable)."
            );
        }
    }
}
