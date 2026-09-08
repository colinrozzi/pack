//! # packr-web
//!
//! The browser backend for [`packr_core`]: implements `WasmEngine` /
//! `WasmInstance` over the JS `WebAssembly` API via `js-sys` /
//! `wasm-bindgen-futures`. Same traits as `packr-wasmtime`, different substrate —
//! the graph-ABI orchestration in `packr-core` runs on top unchanged.
//!
//! Scope (v1): the **execution cluster** (host → guest) for self-contained
//! actors — compile, instantiate, call exports, touch memory. Host imports and
//! the default `pack:alloc` need async wasm imports (JSPI) and are the next
//! slice.

use std::sync::Arc;

use js_sys::{Array, Function, Object, Reflect, Uint8Array, WebAssembly};
use packr_core::backend::{EngineError, Val, WasmEngine, WasmInstance};
use packr_core::host::HostImports;
use packr_core::CallInterceptor;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

/// The pending/resume async-ABI driver (model B) for the browser — no JSPI.
pub mod model_b;
pub use model_b::call_resume;

fn jserr(ctx: &str, e: JsValue) -> EngineError {
    EngineError::Other(format!("{ctx}: {e:?}"))
}

/// The browser wasm engine (JS `WebAssembly`).
pub struct WebEngine;

impl WebEngine {
    pub fn new() -> Self {
        WebEngine
    }
}

impl Default for WebEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl WasmEngine for WebEngine {
    type Module = WebAssembly::Module;
    type Instance = WebInstance;

    async fn compile(&self, bytes: &[u8]) -> Result<WebAssembly::Module, EngineError> {
        let arr = Uint8Array::new_with_length(bytes.len() as u32);
        arr.copy_from(bytes);
        let module = JsFuture::from(WebAssembly::compile(&arr))
            .await
            .map_err(|e| jserr("compile", e))?;
        module
            .dyn_into::<WebAssembly::Module>()
            .map_err(|e| jserr("compile: result is not a Module", e))
    }

    async fn instantiate(
        &self,
        module: &WebAssembly::Module,
        imports: HostImports,
    ) -> Result<WebInstance, EngineError> {
        // Execution cluster: self-contained actors import nothing → empty import
        // object. Host imports / default pack:alloc need async wasm imports
        // (JSPI) and are the next slice.
        if !imports.imports().is_empty() {
            return Err(EngineError::Other(
                "packr-web: host imports not yet supported (needs JSPI)".to_string(),
            ));
        }

        let import_object = Object::new();
        let instance = JsFuture::from(WebAssembly::instantiate_module(module, &import_object))
            .await
            .map_err(|e| jserr("instantiate", e))?;
        let instance: WebAssembly::Instance = instance
            .dyn_into()
            .map_err(|e| jserr("instantiate: result is not an Instance", e))?;

        let exports = instance.exports();
        let memory = Reflect::get(&exports, &JsValue::from_str("memory"))
            .ok()
            .and_then(|m| m.dyn_into::<WebAssembly::Memory>().ok())
            .ok_or(EngineError::NoMemory)?;

        let export_names = Object::keys(&exports)
            .to_vec()
            .into_iter()
            .filter_map(|k| k.as_string())
            .collect();

        Ok(WebInstance {
            exports,
            memory,
            export_names,
            interceptor: imports.interceptor().cloned(),
        })
    }
}

/// A live JS `WebAssembly.Instance`.
pub struct WebInstance {
    exports: Object,
    memory: WebAssembly::Memory,
    export_names: Vec<String>,
    interceptor: Option<Arc<dyn CallInterceptor>>,
}

impl WebInstance {
    /// A fresh `Uint8Array` view over the current memory buffer. Re-created each
    /// access because the buffer detaches on `memory.grow`.
    fn mem_view(&self) -> Uint8Array {
        Uint8Array::new(&self.memory.buffer())
    }
}

impl WasmInstance for WebInstance {
    async fn call(&mut self, name: &str, args: &[Val]) -> Result<Vec<Val>, EngineError> {
        let func = Reflect::get(&self.exports, &JsValue::from_str(name))
            .ok()
            .and_then(|f| f.dyn_into::<Function>().ok())
            .ok_or_else(|| EngineError::ExportNotFound(name.to_string()))?;

        let js_args = Array::new();
        for a in args {
            let n = match a {
                Val::I32(x) => *x as f64,
                Val::I64(x) => *x as f64,
            };
            js_args.push(&JsValue::from_f64(n));
        }

        // wasm calls are synchronous in JS.
        let result = func.apply(&JsValue::NULL, &js_args).map_err(|e| EngineError::Call {
            name: name.to_string(),
            reason: format!("{e:?}"),
        })?;

        // A single numeric return, or undefined (no return → empty).
        let mut out = Vec::new();
        if let Some(n) = result.as_f64() {
            out.push(Val::I32(n as i32));
        }
        Ok(out)
    }

    fn read_memory(&self, offset: usize, buf: &mut [u8]) -> Result<(), EngineError> {
        let view = self.mem_view();
        let end = offset + buf.len();
        if end as u32 > view.length() {
            return Err(EngineError::MemoryOutOfBounds {
                offset,
                len: buf.len(),
            });
        }
        view.subarray(offset as u32, end as u32).copy_to(buf);
        Ok(())
    }

    fn write_memory(&mut self, offset: usize, data: &[u8]) -> Result<(), EngineError> {
        let view = self.mem_view();
        if (offset + data.len()) as u32 > view.length() {
            return Err(EngineError::MemoryOutOfBounds {
                offset,
                len: data.len(),
            });
        }
        let src = Uint8Array::new_with_length(data.len() as u32);
        src.copy_from(data);
        view.set(&src, offset as u32);
        Ok(())
    }

    fn memory_size(&self) -> usize {
        self.mem_view().length() as usize
    }

    fn grow_memory(&mut self, delta_pages: u64) -> Result<(), EngineError> {
        self.memory.grow(delta_pages as u32);
        Ok(())
    }

    fn has_export(&self, name: &str) -> bool {
        self.export_names.iter().any(|n| n == name)
    }

    fn export_names(&self) -> Vec<String> {
        self.export_names.clone()
    }

    fn set_deadline(&mut self, _ticks_until_trap: u64) {
        // Browser preemption is Worker.terminate (driver-level), not
        // per-instance — a documented no-op here.
    }

    fn interceptor(&self) -> Option<&Arc<dyn CallInterceptor>> {
        self.interceptor.as_ref()
    }
}
