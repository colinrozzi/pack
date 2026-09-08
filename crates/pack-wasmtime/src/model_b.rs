//! The pending/resume async-ABI driver on wasmtime (`docs/async-abi.md`, model B).
//!
//! Validates the whole protocol natively: host imports are registered as
//! **poll-once** `func_wrap`s (no engine suspension) that stash not-yet-ready
//! host futures under a `completion_id`; the pump ([`packr_core::drive`]) then
//! resolves those futures and re-enters the guest via `__pack_resume`. wasmtime
//! drives `__pack_resume` with an ordinary `call_async`, so this proves the
//! protocol end to end before any browser backend exists.

use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

use packr_core::abi_call::{CallError, RESULT_LEN_OFFSET, RESULT_PTR_OFFSET};
use packr_core::host::{BoxFuture, HostError, HostImports};
use packr_core::resume::{drive, CompletionRegistry, ResumeTarget, TAG_PENDING, TAG_READY};
use packr_core::Value;
use wasmtime::{Caller, Linker, Memory, Module, Store, TypedFunc};

use crate::WasmtimeEngine;

/// Per-instance store data for the model-B driver: the `completion_id` counter
/// and the futures the poll-once import handlers stashed during a guest call
/// (drained into the pump's registry after each call).
#[derive(Default)]
struct ModelBCtx {
    next_id: u32,
    /// The at-most-one host future stashed during the current guest call — actors
    /// are sequential (≤1 in-flight host call). Drained into the pump's registry
    /// after each guest call.
    new_pending: Option<(u32, BoxFuture<'static, Result<Value, HostError>>)>,
}

/// Run an actor export under the pending/resume protocol on wasmtime. Instantiate
/// with poll-once import handlers, then pump the export to completion.
pub async fn call_resume(
    engine: &WasmtimeEngine,
    module: &Module,
    imports: HostImports,
    name: &str,
    input: &Value,
) -> Result<Value, CallError> {
    let mut store = Store::new(engine.engine(), ModelBCtx::default());
    // epoch_interruption is on engine-wide; default to no deadline or the guest
    // traps immediately.
    store.set_epoch_deadline(crate::NO_EPOCH_DEADLINE);
    let mut linker = Linker::new(engine.engine());
    crate::register_default_alloc(&mut linker)
        .map_err(|e| CallError::Engine(packr_core::EngineError::Instantiate(e.to_string())))?;

    // Register each host import as a poll-once, non-suspending func_wrap.
    for imp in imports.imports() {
        let host_fn = imp.func.clone();
        linker
            .func_wrap(
                &imp.interface,
                &imp.function,
                move |mut caller: Caller<'_, ModelBCtx>,
                      in_ptr: i32,
                      in_len: i32,
                      out_ptr_slot: i32,
                      out_len_slot: i32|
                      -> i32 {
                    handle_import(
                        &mut caller,
                        &host_fn,
                        in_ptr,
                        in_len,
                        out_ptr_slot,
                        out_len_slot,
                    )
                },
            )
            .map_err(|e| CallError::Engine(packr_core::EngineError::Instantiate(e.to_string())))?;
    }

    let instance = linker
        .instantiate_async(&mut store, module)
        .await
        .map_err(|e| CallError::Engine(packr_core::EngineError::Instantiate(e.to_string())))?;
    let memory = instance
        .get_memory(&mut store, "memory")
        .ok_or(CallError::Engine(packr_core::EngineError::NoMemory))?;

    let process: TypedFunc<(i32, i32, i32, i32), i32> =
        instance.get_typed_func(&mut store, name).map_err(|e| {
            CallError::Engine(packr_core::EngineError::ExportNotFound(e.to_string()))
        })?;
    let resume: TypedFunc<(i32, i32, i32, i32, i32), i32> = instance
        .get_typed_func(&mut store, "__pack_resume")
        .map_err(|e| CallError::Engine(packr_core::EngineError::ExportNotFound(e.to_string())))?;
    let alloc: TypedFunc<i32, i32> = instance
        .get_typed_func(&mut store, "__pack_alloc")
        .map_err(|e| CallError::Engine(packr_core::EngineError::ExportNotFound(e.to_string())))?;

    let mut target = WasmtimeResume {
        store,
        memory,
        process,
        resume,
        alloc,
    };
    drive(&mut target, name, input).await
}

/// A poll-once host-import handler: read + decode the input, poll the host future
/// once. `Ready` → return the result inline (`READY`); `Pending` → stash it under
/// a fresh `completion_id`, write that id into the guest's out-ptr slot, return
/// `PENDING`. Returns `-1` on any host-side failure.
fn handle_import(
    caller: &mut Caller<'_, ModelBCtx>,
    host_fn: &packr_core::host::HostFn,
    in_ptr: i32,
    in_len: i32,
    out_ptr_slot: i32,
    out_len_slot: i32,
) -> i32 {
    let memory = match caller.get_export("memory").and_then(|e| e.into_memory()) {
        Some(m) => m,
        None => return -1,
    };
    let mut buf = vec![0u8; in_len as usize];
    if memory.read(&*caller, in_ptr as usize, &mut buf).is_err() {
        return -1;
    }
    let input = match packr_core::abi::decode(&buf) {
        Ok(v) => v,
        Err(_) => return -1,
    };

    let mut fut = host_fn(input);
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    match fut.as_mut().poll(&mut cx) {
        Poll::Ready(Ok(result)) => match write_guest_result(caller, &memory, &result) {
            Ok((ptr, len)) => {
                let _ = memory.write(&mut *caller, out_ptr_slot as usize, &ptr.to_le_bytes());
                let _ = memory.write(&mut *caller, out_len_slot as usize, &len.to_le_bytes());
                TAG_READY
            }
            Err(_) => -1,
        },
        Poll::Ready(Err(_)) => -1,
        Poll::Pending => {
            let data = caller.data_mut();
            assert!(
                data.new_pending.is_none(),
                "concurrent host call: actors are sequential (one in-flight host call)"
            );
            let id = data.next_id;
            data.next_id = data.next_id.wrapping_add(1);
            data.new_pending = Some((id, fut));
            if memory
                .write(&mut *caller, out_ptr_slot as usize, &id.to_le_bytes())
                .is_err()
            {
                return -1;
            }
            TAG_PENDING
        }
    }
}

/// Guest-allocate a buffer (sync `__pack_alloc` re-entry) and write `value` into
/// it; returns `(ptr, len)`. Used by the `READY`-inline import path.
fn write_guest_result(
    caller: &mut Caller<'_, ModelBCtx>,
    memory: &Memory,
    value: &Value,
) -> Result<(u32, u32), ()> {
    let bytes = packr_core::abi::encode(value).map_err(|_| ())?;
    let alloc = caller
        .get_export("__pack_alloc")
        .and_then(|e| e.into_func())
        .ok_or(())?;
    let alloc: TypedFunc<i32, i32> = alloc.typed(&*caller).map_err(|_| ())?;
    let ptr = alloc
        .call(&mut *caller, bytes.len() as i32)
        .map_err(|_| ())?;
    memory
        .write(&mut *caller, ptr as usize, &bytes)
        .map_err(|_| ())?;
    Ok((ptr as u32, bytes.len() as u32))
}

/// The pump's view of the guest: call `process` / `__pack_resume`, and after each
/// call drain the futures the import handlers stashed into the pump's registry.
struct WasmtimeResume {
    store: Store<ModelBCtx>,
    memory: Memory,
    process: TypedFunc<(i32, i32, i32, i32), i32>,
    resume: TypedFunc<(i32, i32, i32, i32, i32), i32>,
    alloc: TypedFunc<i32, i32>,
}

impl WasmtimeResume {
    /// Guest-allocate + write `value`; returns its ptr.
    async fn write_input(&mut self, value: &Value) -> Result<i32, CallError> {
        let bytes = packr_core::abi::encode(value).map_err(|e| CallError::Abi(format!("{e:?}")))?;
        let ptr = self
            .alloc
            .call_async(&mut self.store, bytes.len() as i32)
            .await
            .map_err(|e| werr(&e))?;
        self.memory
            .write(&mut self.store, ptr as usize, &bytes)
            .map_err(|e| CallError::Abi(e.to_string()))?;
        Ok(ptr)
    }

    /// Read the DONE result from the fixed result slots and decode it.
    fn read_output(&mut self) -> Result<Value, CallError> {
        let mut ptr_bytes = [0u8; 4];
        let mut len_bytes = [0u8; 4];
        self.memory
            .read(&self.store, RESULT_PTR_OFFSET, &mut ptr_bytes)
            .map_err(|e| CallError::Abi(e.to_string()))?;
        self.memory
            .read(&self.store, RESULT_LEN_OFFSET, &mut len_bytes)
            .map_err(|e| CallError::Abi(e.to_string()))?;
        let out_ptr = i32::from_le_bytes(ptr_bytes) as usize;
        let out_len = i32::from_le_bytes(len_bytes) as usize;
        let mut out = vec![0u8; out_len];
        self.memory
            .read(&self.store, out_ptr, &mut out)
            .map_err(|e| CallError::Abi(e.to_string()))?;
        packr_core::abi::decode(&out).map_err(|e| CallError::Abi(format!("{e:?}")))
    }

    /// Move the host future the handler stashed during the last guest call into
    /// the pump's registry (keyed by the id the handler already told the guest).
    fn drain_into(&mut self, registry: &mut CompletionRegistry) {
        if let Some((id, fut)) = self.store.data_mut().new_pending.take() {
            registry.insert(id, fut);
        }
    }
}

impl ResumeTarget for WasmtimeResume {
    async fn call_entry(
        &mut self,
        name: &str,
        input: &Value,
        registry: &mut CompletionRegistry,
    ) -> Result<(i32, Option<Value>), packr_core::EngineError> {
        let in_ptr = self
            .write_input(input)
            .await
            .map_err(|e| packr_core::EngineError::Other(e.to_string()))?;
        let in_len = encoded_len(input)?;
        let tag = self
            .process
            .call_async(
                &mut self.store,
                (
                    in_ptr,
                    in_len,
                    RESULT_PTR_OFFSET as i32,
                    RESULT_LEN_OFFSET as i32,
                ),
            )
            .await
            .map_err(|e| packr_core::EngineError::Call {
                name: name.to_string(),
                reason: e.to_string(),
            })?;
        self.drain_into(registry);
        finish(self, tag)
    }

    async fn resume(
        &mut self,
        completion_id: u32,
        result: Value,
        registry: &mut CompletionRegistry,
    ) -> Result<(i32, Option<Value>), packr_core::EngineError> {
        let res_ptr = self
            .write_input(&result)
            .await
            .map_err(|e| packr_core::EngineError::Other(e.to_string()))?;
        let res_len = encoded_len(&result)?;
        let tag = self
            .resume
            .call_async(
                &mut self.store,
                (
                    completion_id as i32,
                    res_ptr,
                    res_len,
                    RESULT_PTR_OFFSET as i32,
                    RESULT_LEN_OFFSET as i32,
                ),
            )
            .await
            .map_err(|e| packr_core::EngineError::Call {
                name: "__pack_resume".to_string(),
                reason: e.to_string(),
            })?;
        self.drain_into(registry);
        finish(self, tag)
    }
}

/// Interpret a guest tag: `READY` → read + return the DONE output; `PENDING` →
/// no output yet.
fn finish(
    target: &mut WasmtimeResume,
    tag: i32,
) -> Result<(i32, Option<Value>), packr_core::EngineError> {
    if tag == TAG_READY {
        let out = target
            .read_output()
            .map_err(|e| packr_core::EngineError::Other(e.to_string()))?;
        Ok((TAG_READY, Some(out)))
    } else if tag == TAG_PENDING {
        Ok((TAG_PENDING, None))
    } else {
        Err(packr_core::EngineError::Other(format!(
            "guest returned error tag {tag}"
        )))
    }
}

fn encoded_len(value: &Value) -> Result<i32, packr_core::EngineError> {
    let bytes = packr_core::abi::encode(value)
        .map_err(|e| packr_core::EngineError::Other(format!("{e:?}")))?;
    Ok(bytes.len() as i32)
}

fn werr(e: &wasmtime::Error) -> CallError {
    CallError::Engine(packr_core::EngineError::Other(e.to_string()))
}

fn noop_waker() -> Waker {
    fn no_op(_: *const ()) {}
    fn clone(_: *const ()) -> RawWaker {
        RawWaker::new(std::ptr::null(), &VTABLE)
    }
    static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, no_op, no_op, no_op);
    unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) }
}
