//! Regression test for the `#[import_from]` `Result`/`Option` decode fix.
//!
//! `comp-result-app` declares `#[import_from("mathr")] fn checked(n: i64) ->
//! Result<i64, String>` — a `Result`-returning PACKAGE import. Under the old
//! macro (which decoded the import result via `result.try_into()` /
//! `TryFrom<Value>`) that fixture would not even COMPILE, because the composite
//! ABI implements `FromValue` for `Result`/`Option` but not `TryFrom<Value>`;
//! every such consumer needed a hand-written newtype shim (mesh-client hit this).
//! With the macro decoding via `FromValue` (like `#[import]`/`#[export]`), it
//! compiles and round-trips.
//!
//! So building `comp-result-app` at all exercises the fix; composing + running it
//! against `math-result` proves the `Result` actually decodes end-to-end.

use packr::abi::Value;
use packr::compose::{compose, Component, GraphLink};
use packr::runtime::Runtime;
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

#[test]
fn import_from_decodes_a_result_return_without_a_shim() {
    // `math-result` (a plain `#[export]` provider) compiles under both the old and
    // fixed macro, so it is our toolchain probe. `comp-result-app` compiles ONLY
    // under the fixed macro. So: both build → run the test; probe builds but the
    // import-side fixture does NOT → hard FAILURE (the regression, not a missing
    // toolchain); neither builds → toolchain absent → skip.
    let prov = build_component("math-result");
    let app = build_component("comp-result-app");
    let (app, prov) = match (app, prov) {
        (Some(app), Some(prov)) => (app, prov),
        (None, Some(_)) => panic!(
            "REGRESSION: comp-result-app failed to compile while math-result built — \
             `#[import_from]` cannot decode a `Result` return. It must decode via \
             `FromValue`, not `TryFrom<Value>`."
        ),
        _ => {
            eprintln!("SKIP: wasm toolchain unavailable (math-result did not build).");
            return;
        }
    };

    let components = vec![
        Component {
            name: "app".to_string(),
            wasm: std::fs::read(app).expect("read app wasm"),
            entry: true,
        },
        Component {
            name: "mathr".to_string(),
            wasm: std::fs::read(prov).expect("read provider wasm"),
            entry: false,
        },
    ];
    let links = vec![GraphLink {
        consumer: "app".to_string(),
        import_module: "mathr".to_string(),
        import_name: "checked".to_string(),
        provider: "mathr".to_string(),
        export_name: "checked".to_string(),
    }];

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

    let runtime = Runtime::new();
    let module = runtime.load_module(&composite).expect("load composite");
    let mut instance = module.instantiate().expect("instantiate composite");

    // run(21) -> checked(21) -> Ok(42): the Result decoded across the import.
    let ok = instance
        .call_with_value("run", &Value::S64(21))
        .expect("call run(21)");
    match ok {
        Value::Result { value: Ok(v), .. } => assert_eq!(*v, Value::S64(42)),
        other => panic!("run(21) must be Ok(42), got {other:?}"),
    }

    // run(-1) -> checked(-1) -> Err("negative"): the Err arm decodes too.
    let err = instance
        .call_with_value("run", &Value::S64(-1))
        .expect("call run(-1)");
    match err {
        Value::Result { value: Err(e), .. } => {
            assert_eq!(*e, Value::String("negative".to_string()))
        }
        other => panic!("run(-1) must be Err, got {other:?}"),
    }
}
