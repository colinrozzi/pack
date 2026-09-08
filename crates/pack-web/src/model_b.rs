//! The pending/resume async-ABI on the browser (`docs/async-abi.md`, model B) —
//! the universal answer, **no JSPI**.
//!
//! Host imports are registered as **poll-once** JS functions (a wasm import
//! returns synchronously; it never suspends the stack). A not-ready host future
//! is stashed under a `completion_id`; the pump ([`packr_core::drive`]) then
//! awaits it on the JS event loop and re-enters the guest via `__pack_resume`.
//! Because the future is `!Send` (it awaits a JS promise), this path only exists
//! on wasm — enabled by the `MaybeSend` seam.

use std::cell::RefCell;
use std::rc::Rc;

use js_sys::{Array, Function, Object, Reflect, Uint8Array, WebAssembly};
use packr_core::abi_call::{CallError, RESULT_LEN_OFFSET, RESULT_PTR_OFFSET};
use packr_core::host::{BoxFuture, HostImports};
use packr_core::resume::{drive, CompletionRegistry, ResumeTarget, TAG_PENDING, TAG_READY};
use packr_core::{EngineError, Value};
use wasm_bindgen::prelude::Closure;
use wasm_bindgen::{JsCast, JsValue};

use crate::WebEngine;

/// Shared per-instance state: filled after instantiation (imports need the
/// memory/allocator, which don't exist until then), plus the completion machinery
/// the poll-once handlers write and the pump drains. Single-threaded → `Rc`.
#[derive(Default)]
struct Shared {
    memory: Option<WebAssembly::Memory>,
    alloc: Option<Function>,
    next_id: u32,
    /// The at-most-one host future stashed during the current guest call — actors
    /// are sequential (≤1 in-flight host call).
    new_pending: Option<(u32, BoxFuture<'static, Result<Value, packr_core::host::HostError>>)>,
}

type SharedRef = Rc<RefCell<Shared>>;

fn jserr(ctx: &str, e: JsValue) -> EngineError {
    EngineError::Other(format!("{ctx}: {e:?}"))
}

/// Run an actor export under pending/resume on the browser. Instantiate with
/// poll-once JS import handlers, then pump to completion.
pub async fn call_resume(
    _engine: &WebEngine,
    module: &WebAssembly::Module,
    imports: HostImports,
    name: &str,
    input: &Value,
) -> Result<Value, CallError> {
    let shared: SharedRef = Rc::new(RefCell::new(Shared::default()));

    // Build the import object: one JS closure per host import that stashes the
    // deferred host-call future (the pump runs it).
    let import_object = Object::new();
    let mut closures: Vec<Closure<dyn Fn(i32, i32, i32, i32) -> i32>> = Vec::new();
    let interceptor = imports.interceptor().cloned();
    for imp in imports.imports() {
        let host_fn = imp.func.clone();
        let shared_cl = shared.clone();
        let interceptor = interceptor.clone();
        let interface = imp.interface.clone();
        let function = imp.function.clone();
        let closure = Closure::wrap(Box::new(
            move |in_ptr: i32, in_len: i32, out_ptr_slot: i32, _out_len_slot: i32| -> i32 {
                handle_import(
                    &shared_cl,
                    &host_fn,
                    interceptor.clone(),
                    &interface,
                    &function,
                    in_ptr,
                    in_len,
                    out_ptr_slot,
                )
            },
        ) as Box<dyn Fn(i32, i32, i32, i32) -> i32>);

        let ns = ensure_namespace(&import_object, &imp.interface)
            .map_err(|e| jserr("build import namespace", e))?;
        Reflect::set(&ns, &JsValue::from_str(&imp.function), closure.as_ref().unchecked_ref())
            .map_err(|e| jserr("set import fn", e))?;
        closures.push(closure);
    }

    let instance = wasm_bindgen_futures::JsFuture::from(WebAssembly::instantiate_module(
        module,
        &import_object,
    ))
    .await
    .map_err(|e| jserr("instantiate", e))?;
    let instance: WebAssembly::Instance = instance
        .dyn_into()
        .map_err(|e| jserr("instantiate: not an Instance", e))?;
    let exports = instance.exports();

    let memory = Reflect::get(&exports, &JsValue::from_str("memory"))
        .ok()
        .and_then(|m| m.dyn_into::<WebAssembly::Memory>().ok())
        .ok_or(CallError::Engine(EngineError::NoMemory))?;
    let get_fn = |n: &str| -> Result<Function, CallError> {
        Reflect::get(&exports, &JsValue::from_str(n))
            .ok()
            .and_then(|f| f.dyn_into::<Function>().ok())
            .ok_or_else(|| CallError::Engine(EngineError::ExportNotFound(n.to_string())))
    };
    let process = get_fn(name)?;
    let resume = get_fn("__pack_resume")?;
    let alloc = get_fn("__pack_alloc")?;

    {
        let mut s = shared.borrow_mut();
        s.memory = Some(memory.clone());
        s.alloc = Some(alloc.clone());
    }

    let mut target = WebResume {
        shared: shared.clone(),
        memory,
        process,
        resume,
        alloc,
        _closures: closures,
    };
    drive(&mut target, name, input).await
}

/// Ensure `import_object[interface]` is an object and return it.
fn ensure_namespace(import_object: &Object, interface: &str) -> Result<Object, JsValue> {
    let key = JsValue::from_str(interface);
    if let Ok(existing) = Reflect::get(import_object, &key) {
        if let Ok(obj) = existing.dyn_into::<Object>() {
            return Ok(obj);
        }
    }
    let ns = Object::new();
    Reflect::set(import_object, &key, &ns)?;
    Ok(ns)
}

/// Host-import handler: decode the input, build the deferred interceptor-aware
/// host-call future, stash it under a fresh `completion_id`, write that id into
/// the out-ptr slot, and return `PENDING`. The pump runs the future (async, so
/// the interceptor hooks run correctly) and re-enters via `__pack_resume`.
#[allow(clippy::too_many_arguments)]
fn handle_import(
    shared: &SharedRef,
    host_fn: &packr_core::host::HostFn,
    interceptor: Option<std::sync::Arc<dyn packr_core::CallInterceptor>>,
    interface: &str,
    function: &str,
    in_ptr: i32,
    in_len: i32,
    out_ptr_slot: i32,
) -> i32 {
    let input = {
        let s = shared.borrow();
        let Some(mem) = &s.memory else { return -1 };
        let view = Uint8Array::new(&mem.buffer());
        let mut buf = vec![0u8; in_len as usize];
        view.subarray(in_ptr as u32, in_ptr as u32 + in_len as u32)
            .copy_to(&mut buf);
        match packr_core::abi::decode(&buf) {
            Ok(v) => v,
            Err(_) => return -1,
        }
    };

    let fut = packr_core::host::host_call_future(
        host_fn.clone(),
        interceptor,
        interface.to_string(),
        function.to_string(),
        input,
    );

    let id = {
        let mut s = shared.borrow_mut();
        assert!(
            s.new_pending.is_none(),
            "concurrent host call: actors are sequential (one in-flight host call)"
        );
        let id = s.next_id;
        s.next_id = s.next_id.wrapping_add(1);
        s.new_pending = Some((id, fut));
        id
    };
    let s = shared.borrow();
    let Some(mem) = &s.memory else { return -1 };
    let view = Uint8Array::new(&mem.buffer());
    write_u32(&view, out_ptr_slot as u32, id);
    TAG_PENDING
}

fn write_u32(view: &Uint8Array, offset: u32, value: u32) {
    let src = Uint8Array::new_with_length(4);
    src.copy_from(&value.to_le_bytes());
    view.set(src.as_ref(), offset);
}

fn read_u32(view: &Uint8Array, offset: u32) -> u32 {
    let mut b = [0u8; 4];
    view.subarray(offset, offset + 4).copy_to(&mut b);
    u32::from_le_bytes(b)
}

struct WebResume {
    shared: SharedRef,
    memory: WebAssembly::Memory,
    process: Function,
    resume: Function,
    alloc: Function,
    _closures: Vec<Closure<dyn Fn(i32, i32, i32, i32) -> i32>>,
}

impl WebResume {
    fn write_input(&self, value: &Value) -> Result<(i32, i32), CallError> {
        let bytes = packr_core::abi::encode(value).map_err(|e| CallError::Abi(format!("{e:?}")))?;
        let ptr = self
            .alloc
            .call1(&JsValue::NULL, &JsValue::from_f64(bytes.len() as f64))
            .map_err(|e| jserr("alloc", e))?
            .as_f64()
            .ok_or_else(|| CallError::Abi("alloc returned non-number".into()))? as i32;
        let view = Uint8Array::new(&self.memory.buffer());
        let src = Uint8Array::new_with_length(bytes.len() as u32);
        src.copy_from(&bytes);
        view.set(&src, ptr as u32);
        Ok((ptr, bytes.len() as i32))
    }

    fn read_output(&self) -> Result<Value, CallError> {
        let view = Uint8Array::new(&self.memory.buffer());
        let out_ptr = read_u32(&view, RESULT_PTR_OFFSET as u32);
        let out_len = read_u32(&view, RESULT_LEN_OFFSET as u32);
        let mut out = vec![0u8; out_len as usize];
        view.subarray(out_ptr, out_ptr + out_len).copy_to(&mut out);
        packr_core::abi::decode(&out).map_err(|e| CallError::Abi(format!("{e:?}")))
    }

    fn drain_into(&self, registry: &mut CompletionRegistry) {
        if let Some((id, fut)) = self.shared.borrow_mut().new_pending.take() {
            registry.insert(id, fut);
        }
    }

    fn finish(&self, tag: JsValue) -> Result<(i32, Option<Value>), EngineError> {
        let tag = tag.as_f64().unwrap_or(-1.0) as i32;
        if tag == TAG_READY {
            let out = self
                .read_output()
                .map_err(|e| EngineError::Other(e.to_string()))?;
            Ok((TAG_READY, Some(out)))
        } else if tag == TAG_PENDING {
            Ok((TAG_PENDING, None))
        } else {
            Err(EngineError::Other(format!("guest returned error tag {tag}")))
        }
    }
}

impl ResumeTarget for WebResume {
    async fn call_entry(
        &mut self,
        name: &str,
        input: &Value,
        registry: &mut CompletionRegistry,
    ) -> Result<(i32, Option<Value>), EngineError> {
        let (in_ptr, in_len) = self
            .write_input(input)
            .map_err(|e| EngineError::Other(e.to_string()))?;
        let args = Array::of4(
            &JsValue::from_f64(in_ptr as f64),
            &JsValue::from_f64(in_len as f64),
            &JsValue::from_f64(RESULT_PTR_OFFSET as f64),
            &JsValue::from_f64(RESULT_LEN_OFFSET as f64),
        );
        let tag = self
            .process
            .apply(&JsValue::NULL, &args)
            .map_err(|e| EngineError::Call {
                name: name.to_string(),
                reason: format!("{e:?}"),
            })?;
        self.drain_into(registry);
        self.finish(tag)
    }

    async fn resume(
        &mut self,
        completion_id: u32,
        result: Value,
        registry: &mut CompletionRegistry,
    ) -> Result<(i32, Option<Value>), EngineError> {
        let (res_ptr, res_len) = self
            .write_input(&result)
            .map_err(|e| EngineError::Other(e.to_string()))?;
        let args = Array::of5(
            &JsValue::from_f64(completion_id as f64),
            &JsValue::from_f64(res_ptr as f64),
            &JsValue::from_f64(res_len as f64),
            &JsValue::from_f64(RESULT_PTR_OFFSET as f64),
            &JsValue::from_f64(RESULT_LEN_OFFSET as f64),
        );
        let tag = self
            .resume
            .apply(&JsValue::NULL, &args)
            .map_err(|e| EngineError::Call {
                name: "__pack_resume".to_string(),
                reason: format!("{e:?}"),
            })?;
        self.drain_into(registry);
        self.finish(tag)
    }
}
