//! End-to-end acceptance test for interface-level generics through composition.
//!
//! Composes TWO components:
//!   - `gen-node` (entry): imports the `sm` interface GENERICALLY, over an
//!     interface-level parameter `s` (`type s: serializable`), and exports
//!     `run(n) = sm.apply(n)`.
//!   - `gen-sm`: exports the SAME `sm` interface with `s` pinned to `s64`,
//!     `apply(x) = x + 1`.
//!
//! Because one side is generic (`apply(x: s) -> s`) and the other concrete
//! (`apply(x: s64) -> s64`), their interface hashes DIFFER — so this compose
//! succeeds only because compose-time unification (M4b) binds `s := s64` and
//! reconciles the link instead of rejecting it. The whole chain is exercised on
//! real wasm: the guest embeds `type_params` in `__pack_types` (M4-guest), the
//! host decodes them, and `reconcile_generic_link` accepts the link.
//!
//! Then it loads the composite and calls `run(41)`, asserting `42` — proving the
//! value crossed the memory gap through the reconciled generic link.

use packr::abi::Value;
use packr::compose::{compose, Component, GraphLink};
use packr::runtime::Runtime;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Build one fixture package to wasm, or `None` if the toolchain is unavailable
/// (so the test skips with a clear message rather than failing).
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

/// Count the memories in a wasm module via walrus.
fn memory_count(wasm: &[u8]) -> usize {
    let module = walrus::Module::from_buffer(wasm).expect("composite parses");
    module.memories.iter().count()
}

#[test]
fn compose_generic_node_with_concrete_sm() {
    let (node, sm) = match (build_component("gen-node"), build_component("gen-sm")) {
        (Some(n), Some(s)) => (
            std::fs::read(n).expect("read gen-node wasm"),
            std::fs::read(s).expect("read gen-sm wasm"),
        ),
        _ => {
            eprintln!(
                "SKIP: could not build generic fixtures for wasm32-unknown-unknown \
                 (wasm target / cargo unavailable)."
            );
            return;
        }
    };

    let components = vec![
        Component {
            name: "node".to_string(),
            wasm: node,
            entry: true,
        },
        Component {
            name: "sm".to_string(),
            wasm: sm,
            entry: false,
        },
    ];
    let links = vec![GraphLink {
        consumer: "node".to_string(),
        import_module: "sm".to_string(),
        import_name: "apply".to_string(),
        provider: "sm".to_string(),
        export_name: "apply".to_string(),
    }];

    // The load-bearing assertion: this compose must SUCCEED even though the
    // node imports `sm` generically and gen-sm exports it concretely. Without
    // compose-time unification the differing interface hashes would reject it.
    let composite = match compose(components, &links) {
        Ok(c) => c,
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("wasm-merge") || msg.contains("binaryen") {
                eprintln!("SKIP: {msg}");
                return;
            }
            panic!(
                "generic compose failed — reconciliation should have bound s := s64 \
                 and accepted the link: {e:?}"
            );
        }
    };

    // Isolation preserved: two components, two memories.
    assert_eq!(
        memory_count(&composite),
        2,
        "the composite must keep the two components in separate memories"
    );

    // Correctness: the value crossed the reconciled generic boundary.
    let runtime = Runtime::new();
    let module = runtime
        .load_module(&composite)
        .expect("load composite module");
    let mut instance = module.instantiate().expect("instantiate composite");
    let result = instance
        .call_with_value("run", &Value::S64(41))
        .expect("call run on composite");
    assert_eq!(
        result,
        Value::S64(42),
        "run(41) = sm.apply(41) = 42, marshalled across the reconciled generic link"
    );
}
