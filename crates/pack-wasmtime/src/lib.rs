//! # packr-wasmtime
//!
//! The native backend for [`packr_core`]: implements [`WasmEngine`] /
//! [`WasmInstance`] over `wasmtime`. This is the first concrete impl behind the
//! engine-axis traits (`docs/engine-axis.md`); the graph-ABI orchestration in
//! `packr-core` (e.g. [`packr_core::call_with_value`]) runs on top of it without
//! naming `wasmtime`.
//!
//! Covers both directions: the **execution cluster** (host → guest — compile,
//! instantiate, call exports, touch memory, epoch preemption) and the
//! **host-import cluster** (guest → host — [`WasmtimeHostCtx`] implements
//! `packr_core`'s `HostCallCtx`, so a guest's imported functions are satisfied
//! by capture-based [`HostImports`] and run through the shared
//! `dispatch_host_import` trampoline).

use std::sync::Arc;

use async_trait::async_trait;
use packr_core::backend::{EngineError, Val, WasmEngine, WasmInstance};
use packr_core::host::{dispatch_host_import, HostCallCtx, HostImports};
use packr_core::CallInterceptor;
use wasmtime::{Caller, Config, Engine, Instance, Linker, Memory, Module, Store};

/// A never-tripping epoch deadline. NOT `u64::MAX`: `set_epoch_deadline`
/// computes `current_epoch + delta`, which would overflow once the host's epoch
/// ticker has advanced. Mirrors `packr`'s `NO_EPOCH_DEADLINE`.
const NO_EPOCH_DEADLINE: u64 = u64::MAX / 2;

/// The native wasm engine, backed by `wasmtime` with async + epoch support.
pub struct WasmtimeEngine {
    engine: Engine,
}

impl WasmtimeEngine {
    /// Create a new engine. Enables async support (fibers), multi-memory (for
    /// composed modules), and epoch interruption (the runaway-guest kill switch).
    pub fn new() -> Self {
        let mut config = Config::new();
        config.async_support(true);
        config.wasm_multi_memory(true);
        config.epoch_interruption(true);
        let engine = Engine::new(&config).expect("valid wasmtime config");
        Self { engine }
    }

    /// The underlying `wasmtime::Engine` — e.g. for a host epoch ticker that
    /// calls `engine.increment_epoch()`.
    pub fn engine(&self) -> &Engine {
        &self.engine
    }
}

impl Default for WasmtimeEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl WasmEngine for WasmtimeEngine {
    type Module = Module;
    type Instance = WasmtimeInstance;

    async fn compile(&self, bytes: &[u8]) -> Result<Module, EngineError> {
        Module::new(&self.engine, bytes).map_err(|e| EngineError::Compile(e.to_string()))
    }

    async fn instantiate(
        &self,
        module: &Module,
        imports: HostImports,
    ) -> Result<WasmtimeInstance, EngineError> {
        let mut store = Store::new(&self.engine, ());
        // Epoch interruption is engine-wide; default to no deadline so the guest
        // never traps unless a caller arms it via `set_deadline`.
        store.set_epoch_deadline(NO_EPOCH_DEADLINE);

        let interceptor = imports.interceptor().cloned();

        let mut linker = Linker::new(&self.engine);
        register_default_alloc(&mut linker).map_err(|e| EngineError::Instantiate(e.to_string()))?;
        register_host_imports(&mut linker, &imports)?;

        let instance = linker
            .instantiate_async(&mut store, module)
            .await
            .map_err(|e| EngineError::Instantiate(e.to_string()))?;

        // Static-data init: PIC/self-contained modules use a start function
        // (run by wasmtime during instantiation); legacy modules may export
        // `__wasm_call_ctors`. Run it if present.
        if let Ok(ctors) = instance.get_typed_func::<(), ()>(&mut store, "__wasm_call_ctors") {
            ctors
                .call_async(&mut store, ())
                .await
                .map_err(|e| EngineError::Instantiate(e.to_string()))?;
        }

        // Cache export names now (enumerating later needs `&mut store`, but the
        // trait's `has_export`/`export_names` take `&self`).
        let exports: Vec<String> = instance
            .exports(&mut store)
            .map(|e| e.name().to_string())
            .collect();

        let memory = instance.get_memory(&mut store, "memory");

        Ok(WasmtimeInstance {
            store,
            instance,
            memory,
            exports,
            interceptor,
        })
    }
}

/// A live wasmtime instance. Store data is `()` — the capture-based host-import
/// model means host state lives in the host-fn closures, not a typed store.
pub struct WasmtimeInstance {
    store: Store<()>,
    instance: Instance,
    interceptor: Option<Arc<dyn CallInterceptor>>,
    /// The guest's exported "memory". `None` only for exotic modules with no
    /// exported memory (not yet supported by the execution-cluster path).
    memory: Option<Memory>,
    exports: Vec<String>,
}

impl WasmtimeInstance {
    fn memory(&self) -> Result<Memory, EngineError> {
        self.memory.ok_or(EngineError::NoMemory)
    }
}

impl WasmInstance for WasmtimeInstance {
    async fn call(&mut self, name: &str, args: &[Val]) -> Result<Vec<Val>, EngineError> {
        let func = self
            .instance
            .get_func(&mut self.store, name)
            .ok_or_else(|| EngineError::ExportNotFound(name.to_string()))?;

        let params: Vec<wasmtime::Val> = args
            .iter()
            .map(|v| match v {
                Val::I32(x) => wasmtime::Val::I32(*x),
                Val::I64(x) => wasmtime::Val::I64(*x),
            })
            .collect();

        let n_results = func.ty(&self.store).results().len();
        let mut results = vec![wasmtime::Val::I32(0); n_results];

        func.call_async(&mut self.store, &params, &mut results)
            .await
            .map_err(|e| EngineError::Call {
                name: name.to_string(),
                reason: e.to_string(),
            })?;

        let mut out = Vec::with_capacity(results.len());
        for v in &results {
            out.push(match v {
                wasmtime::Val::I32(x) => Val::I32(*x),
                wasmtime::Val::I64(x) => Val::I64(*x),
                _ => {
                    return Err(EngineError::Other(format!(
                        "unexpected non-integer wasm result from '{name}'"
                    )))
                }
            });
        }
        Ok(out)
    }

    fn read_memory(&self, offset: usize, buf: &mut [u8]) -> Result<(), EngineError> {
        let mem = self.memory()?;
        mem.read(&self.store, offset, buf)
            .map_err(|_| EngineError::MemoryOutOfBounds {
                offset,
                len: buf.len(),
            })
    }

    fn write_memory(&mut self, offset: usize, data: &[u8]) -> Result<(), EngineError> {
        let mem = self.memory()?;
        let len = data.len();
        mem.write(&mut self.store, offset, data)
            .map_err(|_| EngineError::MemoryOutOfBounds { offset, len })
    }

    fn memory_size(&self) -> usize {
        self.memory
            .map(|m| m.data_size(&self.store))
            .unwrap_or(0)
    }

    fn grow_memory(&mut self, delta_pages: u64) -> Result<(), EngineError> {
        let mem = self.memory()?;
        mem.grow(&mut self.store, delta_pages)
            .map(|_| ())
            .map_err(|e| EngineError::Other(e.to_string()))
    }

    fn has_export(&self, name: &str) -> bool {
        self.exports.iter().any(|n| n == name)
    }

    fn export_names(&self) -> Vec<String> {
        self.exports.clone()
    }

    fn set_deadline(&mut self, ticks_until_trap: u64) {
        self.store.set_epoch_deadline(ticks_until_trap);
    }

    fn interceptor(&self) -> Option<&Arc<dyn CallInterceptor>> {
        self.interceptor.as_ref()
    }
}

/// Register the default, per-instance `pack:alloc` provider on a linker.
///
/// A raw, non-intercepted `func_wrap` link (never routes through an interceptor
/// / chain log). Per-instance because the linker is built per instantiation, so
/// the captured bump offset is fresh — no cross-instance aliasing even with a
/// cached module.
///
/// This is the bump-allocator proof provider (no real free); self-contained
/// actors bundle their own in-wasm allocator and never use it, and it satisfies
/// legacy `pack:alloc`-importing modules for the execution-cluster bring-up.
fn register_default_alloc(linker: &mut Linker<()>) -> Result<(), wasmtime::Error> {
    let next = std::sync::Arc::new(std::sync::Mutex::new(0usize));
    linker.func_wrap(
        "pack:alloc",
        "alloc",
        move |mut caller: Caller<'_, ()>, size: i32, align: i32| -> i32 {
            let memory = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                Some(m) => m,
                None => return 0,
            };
            let align = align.max(1) as usize;
            let size = size.max(0) as usize;
            let mut next = next.lock().unwrap();
            // Lazily anchor the heap above the module's static data.
            if *next == 0 {
                *next = memory.data_size(&caller);
            }
            let base = (*next + align - 1) & !(align - 1);
            let end = base + size;
            let cur = memory.data_size(&caller);
            if end > cur {
                let pages = ((end - cur + 0xffff) >> 16) as u64;
                if memory.grow(&mut caller, pages).is_err() {
                    return 0;
                }
            }
            *next = end;
            base as i32
        },
    )?;
    linker.func_wrap(
        "pack:alloc",
        "dealloc",
        move |_caller: Caller<'_, ()>, _ptr: i32, _size: i32, _align: i32| {
            // Bump allocator: no-op.
        },
    )?;
    Ok(())
}

/// Register the capture-based [`HostImports`] on the linker. Each becomes a
/// `func_wrap_async` with the pack host-import signature
/// `(in_ptr, in_len, out_ptr_slot, out_len_slot) -> status`; it builds a
/// [`WasmtimeHostCtx`] over the live caller and runs the shared
/// `dispatch_host_import` trampoline — so the marshalling logic lives once, in
/// packr-core, not per backend.
fn register_host_imports(linker: &mut Linker<()>, imports: &HostImports) -> Result<(), EngineError> {
    let interceptor = imports.interceptor().cloned();
    for imp in imports.imports() {
        let func = imp.func.clone();
        let interceptor = interceptor.clone();
        let interface = imp.interface.clone();
        let function = imp.function.clone();
        linker
            .func_wrap_async(
                &imp.interface,
                &imp.function,
                move |mut caller: Caller<'_, ()>,
                      (in_ptr, in_len, out_ptr, out_len): (i32, i32, i32, i32)| {
                    let func = func.clone();
                    let interceptor = interceptor.clone();
                    let interface = interface.clone();
                    let function = function.clone();
                    Box::new(async move {
                        let mut ctx = WasmtimeHostCtx {
                            caller: &mut caller,
                        };
                        dispatch_host_import(
                            &mut ctx,
                            &func,
                            interceptor.as_ref(),
                            &interface,
                            &function,
                            in_ptr as u32,
                            in_len as u32,
                            out_ptr as u32,
                            out_len as u32,
                        )
                        .await
                    })
                },
            )
            .map_err(|e| EngineError::Instantiate(e.to_string()))?;
    }
    Ok(())
}

/// The per-call guest access `packr-core` needs while a host function runs:
/// read/write the caller's exported memory and re-enter the guest allocator via
/// `__pack_alloc` / `__pack_free`. This is the single backend-specific seam in
/// the guest → host path.
struct WasmtimeHostCtx<'a, 'c> {
    caller: &'a mut Caller<'c, ()>,
}

impl WasmtimeHostCtx<'_, '_> {
    fn memory(&mut self) -> Result<Memory, EngineError> {
        self.caller
            .get_export("memory")
            .and_then(|e| e.into_memory())
            .ok_or(EngineError::NoMemory)
    }
}

#[async_trait]
impl HostCallCtx for WasmtimeHostCtx<'_, '_> {
    fn read(&mut self, offset: usize, buf: &mut [u8]) -> Result<(), EngineError> {
        let mem = self.memory()?;
        mem.read(&*self.caller, offset, buf)
            .map_err(|_| EngineError::MemoryOutOfBounds {
                offset,
                len: buf.len(),
            })
    }

    fn write(&mut self, offset: usize, data: &[u8]) -> Result<(), EngineError> {
        let mem = self.memory()?;
        let len = data.len();
        mem.write(&mut *self.caller, offset, data)
            .map_err(|_| EngineError::MemoryOutOfBounds { offset, len })
    }

    async fn alloc(&mut self, size: usize) -> Result<usize, EngineError> {
        let f = self
            .caller
            .get_export("__pack_alloc")
            .and_then(|e| e.into_func())
            .ok_or_else(|| EngineError::ExportNotFound("__pack_alloc".to_string()))?;
        let mut results = [wasmtime::Val::I32(0)];
        f.call_async(
            &mut *self.caller,
            &[wasmtime::Val::I32(size as i32)],
            &mut results,
        )
        .await
        .map_err(|e| EngineError::Other(e.to_string()))?;
        match results[0] {
            wasmtime::Val::I32(p) if p != 0 => Ok(p as usize),
            _ => Err(EngineError::AllocFailed(size)),
        }
    }

    async fn free(&mut self, ptr: usize, size: usize) -> Result<(), EngineError> {
        if let Some(f) = self
            .caller
            .get_export("__pack_free")
            .and_then(|e| e.into_func())
        {
            f.call_async(
                &mut *self.caller,
                &[
                    wasmtime::Val::I32(ptr as i32),
                    wasmtime::Val::I32(size as i32),
                ],
                &mut [],
            )
            .await
            .map_err(|e| EngineError::Other(e.to_string()))?;
        }
        Ok(())
    }
}
