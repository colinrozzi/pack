//! Composition of a THEATER-shaped actor: the entry component exports
//! `theater:simple/actor.init` (a real actor lifecycle export) and imports
//! `math.double` from a provider component plus the residual host import
//! `theater:simple/runtime.log`.
//!
//! Composed against `math-real`, the `math.double` import is internalized by a
//! bridging shim while `runtime.log` survives as residual host surface. We load
//! the composite in the packr runtime, provide `log` as a host fn, and call
//! `theater:simple/actor.init`, asserting the returned state carries
//! `doubled == 42` — i.e. the cross-component call executed inside an
//! actor-lifecycle export. This is the packr-side mirror of the real-theater
//! e2e (which loads the same composite through theater's own PackInstance).

use packr::abi::Value;
use packr::compose::{compose, Component, GraphLink};
use packr::AsyncRuntime;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

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

fn memory_count(wasm: &[u8]) -> usize {
    walrus::Module::from_buffer(wasm)
        .expect("composite parses")
        .memories
        .iter()
        .count()
}

#[tokio::test]
async fn compose_theater_actor_init_drives_cross_component_call() {
    let app = match build_component("comp-actor") {
        Some(p) => std::fs::read(p).expect("read comp-actor wasm"),
        None => {
            eprintln!("SKIP: could not build comp-actor (wasm target / cargo unavailable).");
            return;
        }
    };
    let math = match build_component("math-real") {
        Some(p) => std::fs::read(p).expect("read math-real wasm"),
        None => {
            eprintln!("SKIP: could not build math-real (wasm target / cargo unavailable).");
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
    ];
    let links = vec![GraphLink {
        consumer: "app".to_string(),
        import_module: "math".to_string(),
        import_name: "double".to_string(),
        provider: "math".to_string(),
        export_name: "double".to_string(),
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

    // Isolation proof: two components, two memories.
    assert_eq!(memory_count(&composite), 2, "composite keeps two memories");

    let runtime = AsyncRuntime::new();
    let module = runtime
        .load_module(&composite)
        .expect("load composite module");

    // Provide the residual host import `theater:simple/runtime.log`.
    let log_count = Arc::new(AtomicUsize::new(0));
    let mut instance = module
        .instantiate_with_host_async(log_count.clone(), |builder| {
            builder.interface("theater:simple/runtime")?.func_async(
                "log",
                |ctx: packr::AsyncCtx<Arc<AtomicUsize>>, _input: Value| async move {
                    ctx.data().fetch_add(1, Ordering::SeqCst);
                    Value::Tuple(vec![])
                },
            )?;
            Ok(())
        })
        .await
        .expect("instantiate composite with residual host.log");

    // Drive the actor lifecycle export. init ignores its input state.
    let result = instance
        .call_with_value_async("theater:simple/actor.init", &Value::Tuple(vec![]))
        .await
        .expect("call theater:simple/actor.init");

    // init returns result<value, string> = Ok(record { doubled: 42 }).
    let state = match result {
        Value::Result { value: Ok(v), .. } => *v,
        other => panic!("init must return Ok(state), got {other:?}"),
    };
    let doubled = match &state {
        Value::Record { fields, .. } => fields
            .iter()
            .find(|(k, _)| k == "doubled")
            .map(|(_, v)| v.clone())
            .expect("state has a `doubled` field"),
        other => panic!("init state must be a record, got {other:?}"),
    };
    assert_eq!(
        doubled,
        Value::S64(42),
        "init must compute double(21)=42 via the cross-component call"
    );

    // The residual host log must have fired (proof the host import was wired).
    assert!(
        log_count.load(Ordering::SeqCst) >= 1,
        "init must have called the residual host.log at least once"
    );
}
