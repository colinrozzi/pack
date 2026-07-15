//! Back-pressure regression: when an interceptor's `after_import` awaits on a
//! full bounded channel, the next host-function call from wasm must wait for
//! the channel to drain before returning to the guest. This is the property
//! theater's chain-subscription redesign (PR #105) depends on to gate the
//! producer rate by the slowest subscriber.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use packr::abi::Value;
use packr::{AsyncCtx, AsyncRuntime, CallInterceptor};
use tokio::sync::mpsc;

struct BackPressureInterceptor {
    sender: mpsc::Sender<Value>,
    started: Arc<AtomicU64>,
    finished: Arc<AtomicU64>,
}

#[async_trait]
impl CallInterceptor for BackPressureInterceptor {
    async fn before_import(&self, _: &str, _: &str, _: &Value) -> Option<Value> {
        None
    }

    async fn after_import(&self, _: &str, _: &str, _: &Value, output: &Value) {
        self.started.fetch_add(1, Ordering::SeqCst);
        self.sender.send(output.clone()).await.expect("send");
        self.finished.fetch_add(1, Ordering::SeqCst);
    }

    async fn before_export(&self, _: &str, _: &Value) -> Option<Value> {
        None
    }

    async fn after_export(&self, _: &str, _: &Value, _: &Value) {}
}

/// Wasm exports `tick_twice`, which calls the host import `test::tick` two
/// times back-to-back. The interceptor records each call via a bounded
/// channel (capacity 1). The second `after_import` must block on `send` until
/// the test drains the channel — and so must the second wasm-side call.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn after_import_back_pressure_blocks_next_host_call() {
    let module_wat = r#"
    (module
        (import "test" "tick" (func $tick (param i32 i32 i32 i32) (result i32)))
        (memory (export "memory") 1)

        (global $r_ptr i32 (i32.const 16384))
        (global $r_len i32 (i32.const 16388))
        (global $out_off i32 (i32.const 16392))

        (func $tick_twice
            (param $in_ptr i32) (param $in_len i32)
            (param $out_ptr_ptr i32) (param $out_len_ptr i32)
            (result i32)
            (local $st i32)

            ;; first call
            (local.set $st
                (call $tick
                    (local.get $in_ptr) (local.get $in_len)
                    (global.get $r_ptr) (global.get $r_len)))
            (if (i32.ne (local.get $st) (i32.const 0))
                (then (return (local.get $st))))

            ;; second call (reuse same input)
            (local.set $st
                (call $tick
                    (local.get $in_ptr) (local.get $in_len)
                    (global.get $r_ptr) (global.get $r_len)))
            (if (i32.ne (local.get $st) (i32.const 0))
                (then (return (local.get $st))))

            ;; copy second result into our own output area so we control the pointer
            (memory.copy
                (global.get $out_off)
                (i32.load (global.get $r_ptr))
                (i32.load (global.get $r_len)))

            (i32.store (local.get $out_ptr_ptr) (global.get $out_off))
            (i32.store (local.get $out_len_ptr) (i32.load (global.get $r_len)))

            (i32.const 0)
        )

        (export "tick_twice" (func $tick_twice))
    )
    "#;

    let wasm_bytes = wat::parse_str(module_wat).expect("parse WAT");
    let runtime = AsyncRuntime::new();
    let module = runtime.load_module(&wasm_bytes).expect("load module");

    let (tx, mut rx) = mpsc::channel::<Value>(1);
    let started = Arc::new(AtomicU64::new(0));
    let finished = Arc::new(AtomicU64::new(0));

    let interceptor: Arc<dyn CallInterceptor> = Arc::new(BackPressureInterceptor {
        sender: tx,
        started: started.clone(),
        finished: finished.clone(),
    });

    let mut instance = module
        .instantiate_with_host_and_interceptor_async((), Some(interceptor), |b| {
            b.interface("test")?
                .func_async("tick", |_: AsyncCtx<()>, v: Value| async move { v })?;
            Ok(())
        })
        .await
        .expect("instantiate");

    let input = Value::S64(7);
    let wasm =
        tokio::spawn(async move { instance.call_with_value_async("tick_twice", &input).await });

    // Give the wasm task time to make both host calls and block on the second
    // after_import. The channel has capacity 1, so:
    //   call #1 -> after_import sends ok (buffer fills)
    //   call #2 -> after_import sends, blocks until rx drains
    tokio::time::sleep(Duration::from_millis(200)).await;

    assert_eq!(
        started.load(Ordering::SeqCst),
        2,
        "both after_import futures should have started"
    );
    assert_eq!(
        finished.load(Ordering::SeqCst),
        1,
        "second after_import should still be blocked on full channel"
    );
    assert!(
        !wasm.is_finished(),
        "wasm call must not return while interceptor.send is awaiting back-pressure"
    );

    // Drain — this lets the second send complete; wasm then unblocks.
    let _first = rx.recv().await.expect("first event");
    let _second = rx.recv().await.expect("second event");

    let result = wasm.await.expect("join").expect("call_with_value_async");
    assert_eq!(result, Value::S64(7));
    assert_eq!(finished.load(Ordering::SeqCst), 2);
}
