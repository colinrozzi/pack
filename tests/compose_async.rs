//! Milestone 3 acceptance test: **async component composition**.
//!
//! The M1/M2 bridging shim is plain SYNCHRONOUS wasm (alloc → copy → call →
//! copy → free). This test proves the M3 hypothesis: when a *provider* component
//! suspends on an ASYNC host import, the existing sync shim works UNCHANGED,
//! because wasmtime's async support suspends the entire fiber — the consumer
//! frame, the shim frame, the provider frame, and all guest memory — at the
//! async host-import boundary, and resumes it later. The suspend happens
//! transparently *below* the shim.
//!
//! Topology:
//!   - `comp-app` (entry): imports `math.double`, exports `run(n) = double(n)`.
//!   - `comp-async-math` (provider): exports `math.double`, but calls the HOST
//!     import `host.tick` before returning `n * 2`.
//!
//! Composed with one link (`app.math.double ← provider.double`), the `math`
//! import is satisfied internally by the sync shim, while `host.tick` survives
//! as a RESIDUAL import that the runtime supplies at instantiate. We supply
//! `tick` as a genuinely async fn (it `await`s `tokio::task::yield_now()`), so
//! the guest fiber actually suspends inside `double`, underneath the shim.
//!
//! Then we call `run(21)` on the composite and assert:
//!   - the result is `42` — the cross-component call completed correctly THROUGH
//!     an async suspend, and
//!   - the composite has exactly TWO memories (the components stayed isolated),
//!     and
//!   - the async host fn actually ran (so the fiber genuinely suspended).

use packr::abi::Value;
use packr::compose::{compose, Component, GraphLink};
use packr::AsyncRuntime;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

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

/// Build the two fixtures, or return `None` (test skips) if the toolchain is
/// unavailable.
fn build_all() -> Option<(Vec<u8>, Vec<u8>)> {
    let app = build_component("comp-app")?;
    let provider = build_component("comp-async-math")?;
    Some((
        std::fs::read(&app).expect("read app wasm"),
        std::fs::read(&provider).expect("read provider wasm"),
    ))
}

#[tokio::test]
async fn compose_async_provider_suspends_under_sync_shim() {
    let (app, provider) = match build_all() {
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
            wasm: provider,
            entry: false,
        },
    ];
    // `comp-app` imports `math.double`; `comp-async-math` exports it under the
    // bare name `double`. The `host.tick` import of the provider is NOT linked —
    // it survives as residual surface, provided by the runtime at instantiate.
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

    // Isolation proof: the composite has exactly two memories.
    assert_eq!(
        memory_count(&composite),
        2,
        "composite must keep the two components in separate memories"
    );

    // Load + instantiate via the ASYNC runtime, providing `host.tick` as a
    // genuinely async host fn. `tick: func()` marshals over the standard Graph
    // ABI `(i32,i32,i32,i32)->i32`: input is an empty tuple, no return. We
    // register it with `func_async` (Value -> Value), await `yield_now()` so the
    // guest fiber actually suspends, and count the calls to prove it ran.
    let runtime = AsyncRuntime::new();
    let module = runtime
        .load_module(&composite)
        .expect("load composite module");

    let tick_count = Arc::new(AtomicUsize::new(0));
    let tick_count_for_host = tick_count.clone();

    let mut instance = module
        .instantiate_with_host_async(tick_count_for_host, |builder| {
            builder.interface("host")?.func_async(
                "tick",
                |ctx: packr::AsyncCtx<Arc<AtomicUsize>>, _input: Value| async move {
                    // Genuinely async: force the guest fiber to suspend here.
                    tokio::task::yield_now().await;
                    ctx.data().fetch_add(1, Ordering::SeqCst);
                    // `tick: func()` returns nothing → an empty tuple.
                    Value::Tuple(vec![])
                },
            )?;
            Ok(())
        })
        .await
        .expect("instantiate composite (async, with residual host.tick)");

    let result = instance
        .call_with_value_async("run", &Value::S64(21))
        .await
        .expect("call run on composite");

    assert_eq!(
        result,
        Value::S64(42),
        "run(21) must return 42 (double(21), marshalled across the memory gap \
         THROUGH an async suspend inside the provider's host call)"
    );

    // The async host fn must have actually run — proof the fiber suspended and
    // resumed, rather than the shim somehow short-circuiting the host call.
    assert_eq!(
        tick_count.load(Ordering::SeqCst),
        1,
        "the async host `tick` must have been invoked exactly once (the guest \
         fiber genuinely suspended on it)"
    );
}
