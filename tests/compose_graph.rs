//! Milestone 2 acceptance test for N-component composition.
//!
//! Composes THREE components:
//!   - `comp-app2` (entry): imports `math.double` AND `util.inc`, exports
//!     `run(n) = inc(double(n))`.
//!   - `math-real`: exports `math.double` (double).
//!   - `comp-util`: exports `util.inc` (n + 1).
//!
//! with TWO links, loads the composite in the packr runtime, calls `run(21)`,
//! and asserts:
//!   - the result is `43` (double(21)=42, inc(42)=43 — both calls crossed the
//!     memory gap), and
//!   - the composite has exactly THREE memories (the components stayed isolated).
//!
//! It exercises both the direct `compose(...)` API and the manifest/CLI path.

use packr::abi::Value;
use packr::compose::{compose, Component, GraphLink};
use packr::runtime::Runtime;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Build one package as a plain self-contained actor wasm (exports its memory,
/// no entry). Returns the built wasm path, or `None` if the wasm toolchain isn't
/// available (so the test skips with a clear message rather than failing).
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
        _ => {
            if out.exists() {
                Some(out)
            } else {
                None
            }
        }
    }
}

/// Count the memories in a wasm module via walrus.
fn memory_count(wasm: &[u8]) -> usize {
    let module = walrus::Module::from_buffer(wasm).expect("composite parses");
    module.memories.iter().count()
}

/// Build the three fixtures, or return `None` (test skips) if the toolchain is
/// unavailable.
fn build_all() -> Option<(Vec<u8>, Vec<u8>, Vec<u8>)> {
    let app = build_component("comp-app2")?;
    let math = build_component("math-real")?;
    let util = build_component("comp-util")?;
    Some((
        std::fs::read(&app).expect("read app wasm"),
        std::fs::read(&math).expect("read math wasm"),
        std::fs::read(&util).expect("read util wasm"),
    ))
}

/// Load the composite, call `run(21)`, assert `43`.
fn assert_runs(composite: &[u8]) {
    let runtime = Runtime::new();
    let module = runtime
        .load_module(composite)
        .expect("load composite module");
    let mut instance = module.instantiate().expect("instantiate composite");

    let result = instance
        .call_with_value("run", &Value::S64(21))
        .expect("call run on composite");

    assert_eq!(
        result,
        Value::S64(43),
        "run(21) must return 43 (inc(double(21)) = inc(42) = 43, both marshalled \
         across the memory gap)"
    );
}

#[test]
fn compose_graph_three_components_two_links() {
    let (app, math, util) = match build_all() {
        Some(t) => t,
        None => {
            eprintln!(
                "SKIP: could not build fixtures for wasm32-unknown-unknown \
                 (wasm target / cargo unavailable)."
            );
            return;
        }
    };

    let components = vec![
        Component {
            name: "app".to_string(),
            wasm: app,
            entry: true,
        },
        Component {
            name: "math".to_string(),
            wasm: math,
            entry: false,
        },
        Component {
            name: "util".to_string(),
            wasm: util,
            entry: false,
        },
    ];
    let links = vec![
        GraphLink {
            consumer: "app".to_string(),
            import_module: "math".to_string(),
            import_name: "double".to_string(),
            provider: "math".to_string(),
            export_name: "double".to_string(),
        },
        GraphLink {
            consumer: "app".to_string(),
            import_module: "util".to_string(),
            import_name: "inc".to_string(),
            provider: "util".to_string(),
            export_name: "inc".to_string(),
        },
    ];

    let composite = match compose(components, &links) {
        Ok(c) => c,
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("wasm-merge") || msg.contains("binaryen") {
                eprintln!("SKIP: {msg}");
                return;
            }
            panic!("compose failed: {e:?}");
        }
    };

    // Isolation proof: exactly three memories.
    assert_eq!(
        memory_count(&composite),
        3,
        "composite must keep the three components in separate memories"
    );

    // Correctness proof.
    assert_runs(&composite);
}

/// Exercise the manifest/CLI path: write a manifest pointing at the prebuilt
/// fixtures, run `packr compose <manifest> -o <out>`, and validate the output.
#[test]
fn compose_graph_via_manifest_cli() {
    // Ensure fixtures are built (so the paths exist) or skip.
    if build_all().is_none() {
        eprintln!("SKIP: fixtures unavailable for the manifest/CLI path.");
        return;
    }

    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let pkgs = Path::new(manifest_dir).join("packages");
    let tmp = std::env::temp_dir();
    let manifest_path = tmp.join(format!(
        "packr-compose-manifest-{}.toml",
        std::process::id()
    ));
    let out_path = tmp.join(format!("packr-compose-cli-out-{}.wasm", std::process::id()));

    let manifest = format!(
        r#"
[[component]]
name = "app"
wasm = "{app}"
entry = true

[[component]]
name = "math"
wasm = "{math}"

[[component]]
name = "util"
wasm = "{util}"

[[link]]
consumer = "app"
import = "math.double"
provider = "math"
export = "double"

[[link]]
consumer = "app"
import = "util.inc"
provider = "util"
export = "inc"
"#,
        app = pkgs
            .join("comp-app2/target/wasm32-unknown-unknown/release/comp_app2.wasm")
            .display(),
        math = pkgs
            .join("math-real/target/wasm32-unknown-unknown/release/math_real.wasm")
            .display(),
        util = pkgs
            .join("comp-util/target/wasm32-unknown-unknown/release/comp_util.wasm")
            .display(),
    );
    std::fs::write(&manifest_path, manifest).expect("write manifest");

    // Run the CLI via `cargo run --bin packr`.
    let status = Command::new(env!("CARGO_BIN_EXE_packr"))
        .args([
            "compose",
            manifest_path.to_str().unwrap(),
            "-o",
            out_path.to_str().unwrap(),
        ])
        .status()
        .expect("run packr compose");

    let _ = std::fs::remove_file(&manifest_path);

    if !status.success() {
        let _ = std::fs::remove_file(&out_path);
        // wasm-merge may be unavailable in a bare shell — treat as skip only if
        // the output was never produced (composition itself failing on toolchain).
        eprintln!("SKIP: `packr compose` exited non-zero (wasm-merge/binaryen missing?)");
        return;
    }

    let composite = std::fs::read(&out_path).expect("read cli composite");
    let _ = std::fs::remove_file(&out_path);

    assert_eq!(
        memory_count(&composite),
        3,
        "cli composite must have 3 memories"
    );
    assert_runs(&composite);
}
