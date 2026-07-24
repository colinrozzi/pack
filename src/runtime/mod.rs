//! Package Runtime
//!
//! Handles package instantiation, linking, and execution.

mod host;
pub mod interceptor;
mod interface_check;

pub use host::{
    AsyncCtx, Ctx, DefaultHostProvider, ErrorHandler, HostFunctionError, HostFunctionErrorKind,
    HostFunctionProvider, HostLinkerBuilder, InterfaceBuilder, LinkerError, INPUT_BUFFER_OFFSET,
    OUTPUT_BUFFER_CAPACITY, OUTPUT_BUFFER_OFFSET, RESULT_LEN_OFFSET, RESULT_PTR_OFFSET,
};
pub use interceptor::CallInterceptor;
pub use interface_check::{
    validate_instance_implements_interface, ExpectedSignature, InterfaceError,
};
// Re-export the wasmtime types that appear in this module's public API
// (AsyncRuntime::engine / wrap_module, AsyncCompiledModule::module) so
// callers can name them without a direct wasmtime dependency.
pub use wasmtime::{Engine, Module};

use crate::abi::{decode, encode, Value};
use crate::parser::{decode_with_schema, encode_with_schema, Interface};
use crate::types::{Type, TypeDef};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use thiserror::Error;
use wasmtime::{Config, Instance as WasmtimeInstance, Linker, Memory, Store};

#[derive(Error, Debug)]
pub enum RuntimeError {
    #[error("Module not found: {0}")]
    ModuleNotFound(String),

    #[error("Function not found: {0}")]
    FunctionNotFound(String),

    #[error("Type mismatch: {0}")]
    TypeMismatch(String),

    #[error("WASM execution error: {0}")]
    WasmError(String),

    #[error("Schema validation error: {0}")]
    SchemaError(String),

    #[error("ABI error: {0}")]
    AbiError(String),

    #[error("Memory error: {0}")]
    MemoryError(String),
}

// ============================================================================
// Host Imports
// ============================================================================

/// State accessible to host functions
#[derive(Clone)]
pub struct HostState {
    /// Log messages collected from the package
    pub log_messages: Arc<Mutex<Vec<String>>>,
    /// Simple bump allocator state (next free offset)
    alloc_offset: Arc<Mutex<usize>>,
}

impl Default for HostState {
    fn default() -> Self {
        Self {
            log_messages: Arc::new(Mutex::new(Vec::new())),
            // Start allocation at 48KB to avoid conflicts with input/output buffers
            alloc_offset: Arc::new(Mutex::new(48 * 1024)),
        }
    }
}

impl HostState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Get all log messages
    pub fn get_logs(&self) -> Vec<String> {
        self.log_messages.lock().unwrap().clone()
    }

    /// Clear log messages
    pub fn clear_logs(&self) {
        self.log_messages.lock().unwrap().clear();
    }
}

/// Builder for configuring host imports
pub struct HostImports {
    state: HostState,
}

impl HostImports {
    pub fn new() -> Self {
        Self {
            state: HostState::new(),
        }
    }

    /// Get a reference to the host state (for reading logs, etc.)
    pub fn state(&self) -> &HostState {
        &self.state
    }
}

impl Default for HostImports {
    fn default() -> Self {
        Self::new()
    }
}

/// The package runtime
pub struct Runtime {
    engine: Engine,
}

impl Runtime {
    pub fn new() -> Self {
        let mut config = Config::new();
        // Enable multi-memory so a *composed* actor (one memory per component,
        // wired by bridging shims — see `crate::compose`) loads here, not just on
        // the async path. A plain single-memory actor is unaffected.
        config.wasm_multi_memory(true);
        let engine = Engine::new(&config).expect("valid wasmtime config");
        Self { engine }
    }

    /// Load a WASM module from bytes
    pub fn load_module(&self, wasm_bytes: &[u8]) -> Result<CompiledModule<'_>, RuntimeError> {
        let module = Module::new(&self.engine, wasm_bytes)
            .map_err(|e| RuntimeError::WasmError(e.to_string()))?;
        Ok(CompiledModule {
            module,
            engine: &self.engine,
        })
    }

    pub fn decode_arg(
        &self,
        types: &[TypeDef],
        bytes: &[u8],
        ty: &Type,
    ) -> Result<Value, RuntimeError> {
        decode_with_schema(types, bytes, ty, None)
            .map_err(|err| RuntimeError::SchemaError(err.to_string()))
    }

    pub fn encode_result(&self, value: &Value) -> Result<Vec<u8>, RuntimeError> {
        encode(value).map_err(|err| RuntimeError::AbiError(err.to_string()))
    }

    pub fn encode_result_with_schema(
        &self,
        types: &[TypeDef],
        value: &Value,
        ty: &Type,
    ) -> Result<Vec<u8>, RuntimeError> {
        encode_with_schema(types, value, ty)
            .map_err(|err| RuntimeError::SchemaError(err.to_string()))
    }
}

impl Default for Runtime {
    fn default() -> Self {
        Self::new()
    }
}

pub(crate) fn werr<E: std::fmt::Display>(e: E) -> RuntimeError {
    RuntimeError::WasmError(e.to_string())
}

/// Reject a module that is not a self-contained actor before instantiating it.
///
/// A self-contained actor OWNS its memory: it exports `memory` + `__pack_alloc`
/// and imports only host functions. It must NOT import `env.memory` or
/// `env.__memory_base` — those mean "the host places me in a shared memory it
/// owns", the retired PIC contract. A PIC / pre-0.10 actor reaching this loader
/// would otherwise instantiate against a mismatched memory model and mis-marshal
/// on the first host call; this turns that into a legible failure at load, which
/// matters most for the fleet cutover (a not-yet-rebuilt actor fails at boot, not
/// on first use). Host-agnostic: it does NOT care WHICH host interfaces are
/// imported, only that the actor owns its own memory.
///
/// This gate enforces packr's OWN contract — nothing of the host's — on two axes.
/// **Memory-ownership** (the same axis compose-side `internalize` enforces): no
/// `env.memory`/`env.__memory_base` import, and an exported `memory`. **The
/// marshalling ABI**: exported `__pack_alloc`/`__pack_free`, through which every
/// arg/result/host-return flows.
/// Both are packr exports, so validating them here is coherent and stays
/// host-agnostic. Lifecycle exports (`handle-send`/`init`) and `pack.composite`
/// are the HOST's contract — validated by the host at spawn, not here. Gating at
/// boot turns two otherwise-degraded-or-late failures (a mismatched memory model
/// mis-marshalling on first call; a missing allocator silently limping on bounded
/// fallback buffers) into one legible failure at load, which matters most for the
/// fleet cutover: a not-yet-rebuilt actor fails at boot, not on first use.
pub(crate) fn assert_self_contained(module: &Module) -> Result<(), RuntimeError> {
    for imp in module.imports() {
        if imp.module() == "env" && (imp.name() == "memory" || imp.name() == "__memory_base") {
            return Err(RuntimeError::WasmError(format!(
                "not a self-contained actor (imports `env::{}`): this looks like a PIC/pre-0.10 \
                 side module. Rebuild it as a plain wasm cdylib that owns and exports its memory \
                 (`setup_guest!` links the allocator in; build with `--export-memory --no-entry`).",
                imp.name()
            )));
        }
    }
    let exports_memory = module
        .exports()
        .any(|e| e.name() == "memory" && matches!(e.ty(), wasmtime::ExternType::Memory(_)));
    if !exports_memory {
        return Err(RuntimeError::WasmError(
            "not a self-contained actor: must export its own `memory`".into(),
        ));
    }
    for f in ["__pack_alloc", "__pack_free"] {
        if !module.exports().any(|e| e.name() == f) {
            return Err(RuntimeError::WasmError(format!(
                "not a self-contained actor: must export `{f}` (packr's marshalling ABI)"
            )));
        }
    }
    Ok(())
}

// ============================================================================
// Async Runtime
// ============================================================================

/// An async-enabled package runtime.
///
/// Use this when you need to register async host functions or call WASM
/// functions asynchronously.
///
/// # Example
///
/// ```ignore
/// let runtime = AsyncRuntime::new();
/// let module = runtime.load_module(&wasm_bytes)?;
///
/// let instance = module.instantiate_with_host_async(MyState::new(), |builder| {
///     builder.interface("theater:runtime")?
///         .func_async("fetch", |ctx, url: String| {
///             Box::pin(async move {
///                 // async operation here
///                 fetch_url(&url).await
///             })
///         })?;
///     Ok(())
/// }).await?;
///
/// let result = instance.call_with_value_async("process", &input, 0).await?;
/// ```
/// A deadline delta meaning "never trap until a caller arms a real one."
///
/// NOT `u64::MAX`: `Store::set_epoch_deadline` computes `current_epoch() + delta`,
/// and once the host's epoch ticker has advanced past 0, `current + u64::MAX`
/// overflows — a panic in debug, a wrap to a garbage tiny (near-immediate) deadline
/// in release, on every instantiate. Half of `u64::MAX` is still ~4.6e18 ticks
/// (~146 billion years at 1/sec), so it never trips, and `current + this` cannot
/// overflow for any realistic epoch count.
const NO_EPOCH_DEADLINE: u64 = u64::MAX / 2;

pub struct AsyncRuntime {
    engine: Engine,
}

impl AsyncRuntime {
    /// Create a new async-enabled runtime.
    pub fn new() -> Self {
        let mut config = Config::new();
        config.async_support(true);
        // Enable multi-memory for composed modules that merge multiple WASM files
        config.wasm_multi_memory(true);
        // Epoch interruption: a runaway guest (e.g. an infinite decode loop) is
        // otherwise UNINTERRUPTIBLE and pegs a core forever, wedging the host.
        // With this on, a store can be given a deadline (`AsyncInstance::
        // set_epoch_deadline`) and the host advances epochs (`engine().
        // increment_epoch()`) on a ticker; when the deadline passes the guest
        // TRAPS and the call returns `Err` — a killable runaway. Stores default
        // to no deadline (see the instantiate paths), so behaviour is unchanged
        // until a caller opts in.
        config.epoch_interruption(true);
        let engine = Engine::new(&config).expect("failed to create async engine");
        Self { engine }
    }

    /// Load a WASM module from bytes.
    pub fn load_module(&self, wasm_bytes: &[u8]) -> Result<AsyncCompiledModule<'_>, RuntimeError> {
        let module = Module::new(&self.engine, wasm_bytes)
            .map_err(|e| RuntimeError::WasmError(e.to_string()))?;
        Ok(AsyncCompiledModule {
            module,
            engine: &self.engine,
        })
    }

    /// Wrap an already-compiled `wasmtime::Module` as an `AsyncCompiledModule`
    /// bound to this runtime's engine.
    ///
    /// Useful for callers that maintain their own compile cache: load the
    /// module once with [`Self::load_module`], extract the inner
    /// [`Module`] via [`AsyncCompiledModule::module`], cache it (the
    /// `Module` is cheap-clone and `Send + Sync`), and reconstruct an
    /// `AsyncCompiledModule` on cache hits without paying the compile
    /// cost again.
    ///
    /// # Panics
    ///
    /// Panics if `module` was compiled by a different `Engine` than this
    /// runtime's. wasmtime treats cross-engine usage as a programmer
    /// error and panics deep inside `Linker::instantiate` on the cache
    /// hit; this check moves the panic to the API boundary so the
    /// message names the bug. The comparison is an `Arc` pointer
    /// compare via [`Engine::same`] — free relative to instantiation.
    pub fn wrap_module(&self, module: Module) -> AsyncCompiledModule<'_> {
        assert!(
            Engine::same(module.engine(), &self.engine),
            "wrap_module: Module was compiled by a different Engine than this runtime"
        );
        AsyncCompiledModule {
            module,
            engine: &self.engine,
        }
    }

    /// Get a reference to the engine.
    pub fn engine(&self) -> &Engine {
        &self.engine
    }
}

impl Default for AsyncRuntime {
    fn default() -> Self {
        Self::new()
    }
}

/// Register the default, per-instance `pack:alloc` provider on a linker.
///
/// Since `setup_guest!()` makes every package import its allocator, every
/// instantiation must satisfy `pack:alloc`. This is the built-in default so
/// the `AsyncInstance` API stays stable and callers (e.g. theater) don't have
/// to supply an allocator.
///
/// Properties that matter downstream:
/// - **Raw, non-intercepted link.** Registered via `func_wrap` (not the typed
///   `HostLinkerBuilder` path), so it never routes through `CallInterceptor`
///   and never pollutes the deterministic replay log.
/// - **Per-instance state.** The linker is created per instantiation, so the
///   captured bump offset is fresh per instance — no cross-instance aliasing
///   even when the compiled module is cached.
///
/// It bump-allocates within the guest's exported memory, starting above the
/// module's initial memory (which holds all static data) and growing as
/// needed. `dealloc` is a no-op. This is the 1a proof provider; 1b replaces it
/// with an in-wasm allocator module (real free) over a shared per-instance
/// `Memory`.
pub(crate) fn register_default_alloc<T: 'static>(
    linker: &mut Linker<T>,
) -> Result<(), RuntimeError> {
    let next = std::sync::Arc::new(std::sync::Mutex::new(0usize));
    linker
        .func_wrap(
            "pack:alloc",
            "alloc",
            move |mut caller: wasmtime::Caller<'_, T>, size: i32, align: i32| -> i32 {
                let memory = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                    Some(m) => m,
                    None => return 0,
                };
                let align = align.max(1) as usize;
                let size = size.max(0) as usize;
                let mut next = next.lock().unwrap();
                // Lazily anchor the heap at the end of the module's initial
                // memory, which is guaranteed to sit above all static data.
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
        )
        .map_err(|e| RuntimeError::WasmError(e.to_string()))?;
    linker
        .func_wrap(
            "pack:alloc",
            "dealloc",
            move |_caller: wasmtime::Caller<'_, T>, _ptr: i32, _size: i32, _align: i32| {
                // Bump allocator: no-op. (1b's in-wasm module reclaims memory.)
            },
        )
        .map_err(|e| RuntimeError::WasmError(e.to_string()))?;
    Ok(())
}

/// A compiled WASM module for async execution.
pub struct AsyncCompiledModule<'a> {
    module: Module,
    engine: &'a Engine,
}

impl AsyncCompiledModule<'_> {
    /// Reference to the underlying compiled module.
    ///
    /// Lets callers extract the `Module` for storage in an external
    /// compile cache; pair with [`AsyncRuntime::wrap_module`] to
    /// reconstruct an `AsyncCompiledModule` on a cache hit.
    pub fn module(&self) -> &Module {
        &self.module
    }

    /// Instantiate the module with no imports (async).
    pub async fn instantiate_async(&self) -> Result<AsyncInstance<()>, RuntimeError> {
        let mut store = Store::new(self.engine, ());
        // epoch_interruption is enabled engine-wide; default to no deadline so
        // the guest never traps unless a caller arms it via set_epoch_deadline.
        store.set_epoch_deadline(NO_EPOCH_DEADLINE);
        let mut linker = Linker::<()>::new(self.engine);
        register_default_alloc(&mut linker)?;

        let instance = linker
            .instantiate_async(&mut store, &self.module)
            .await
            .map_err(|e| RuntimeError::WasmError(e.to_string()))?;

        Ok(AsyncInstance {
            store,
            instance,
            interceptor: None,
            memory: None,
        })
    }

    /// Instantiate the module with a builder function for configuring host functions (async).
    ///
    /// This is the recommended method for async Theater-style integration.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let instance = module.instantiate_with_host_async(MyState::new(), |builder| {
    ///     builder.interface("theater:runtime")?
    ///         .func_async("fetch", |ctx, url: String| {
    ///             Box::pin(async move { fetch(&url).await })
    ///         })?;
    ///     Ok(())
    /// }).await?;
    /// ```
    pub async fn instantiate_with_host_async<T, F>(
        &self,
        state: T,
        configure: F,
    ) -> Result<AsyncInstance<T>, RuntimeError>
    where
        T: Send + 'static,
        F: FnOnce(&mut HostLinkerBuilder<'_, T>) -> Result<(), LinkerError>,
    {
        self.instantiate_with_host_and_interceptor_async(state, None, configure)
            .await
    }

    /// Instantiate the module with host functions and a call interceptor (async).
    ///
    /// The interceptor is set on the `HostLinkerBuilder` (to intercept import calls)
    /// and on the resulting `AsyncInstance` (to intercept export calls).
    pub async fn instantiate_with_host_and_interceptor_async<T, F>(
        &self,
        state: T,
        interceptor: Option<Arc<dyn CallInterceptor>>,
        configure: F,
    ) -> Result<AsyncInstance<T>, RuntimeError>
    where
        T: Send + 'static,
        F: FnOnce(&mut HostLinkerBuilder<'_, T>) -> Result<(), LinkerError>,
    {
        // A self-contained actor owns its memory + allocator and imports only
        // host functions. Reject anything else BEFORE we build the store.
        assert_self_contained(&self.module)?;

        let mut store = Store::new(self.engine, state);
        // epoch_interruption is enabled engine-wide; default to no deadline so
        // the guest never traps unless the caller arms it via set_epoch_deadline.
        store.set_epoch_deadline(NO_EPOCH_DEADLINE);

        // Only the caller's host functions to wire — the actor provides its own
        // memory and `__pack_alloc`, so there is no shared memory, allocator side
        // module, or PIC linkage to set up. Host functions resolve the actor's
        // EXPORTED memory from the caller at call time, so no `with_memory` is
        // needed (and it does not exist yet — the actor defines it).
        let mut linker = Linker::new(self.engine);
        let mut builder = HostLinkerBuilder::new(self.engine, &mut linker);
        if let Some(ref interceptor) = interceptor {
            builder.set_interceptor(interceptor.clone());
        }
        configure(&mut builder).map_err(|e| RuntimeError::WasmError(e.to_string()))?;

        let instance = linker
            .instantiate_async(&mut store, &self.module)
            .await
            .map_err(werr)?;

        // Grab the actor's exported memory for host-side marshalling.
        let memory = instance
            .get_export(&mut store, "memory")
            .and_then(|e| e.into_memory());

        // Static data is at absolute addresses (no relocation) — there is no
        // `__wasm_apply_data_relocs` to call. Run ctors for any static init.
        if let Ok(ctors) = instance.get_typed_func::<(), ()>(&mut store, "__wasm_call_ctors") {
            ctors.call_async(&mut store, ()).await.map_err(werr)?;
        }

        Ok(AsyncInstance {
            store,
            instance,
            interceptor,
            memory,
        })
    }

    /// Get a reference to the engine.
    pub fn engine(&self) -> &Engine {
        self.engine
    }
}

/// An async WASM instance.
pub struct AsyncInstance<T> {
    store: Store<T>,
    instance: WasmtimeInstance,
    interceptor: Option<Arc<dyn CallInterceptor>>,
    /// Host-owned shared memory for PIC packages (which don't export memory).
    /// `None` for legacy modules that export their own memory.
    memory: Option<Memory>,
}

impl<T: Send> AsyncInstance<T> {
    /// Validate that this instance implements the given interface.
    pub fn validate_interface(&mut self, interface: &Interface) -> Result<(), InterfaceError> {
        validate_instance_implements_interface(&mut self.store, &self.instance, interface)
    }

    /// Arm the runaway-guest kill switch: the next guest call traps once
    /// `ticks_until_trap` epochs have elapsed. Pair with a host ticker that
    /// calls [`AsyncRuntime::engine`]`().increment_epoch()` at a fixed cadence
    /// — e.g. `set_epoch_deadline(30)` + one tick/second ⇒ a ~30s deadline.
    ///
    /// The deadline is relative to the engine's current epoch and re-arms every
    /// call (each call may set its own). On expiry the guest gets an epoch trap
    /// and the call returns `Err`, so a stuck decode/loop fails cleanly instead
    /// of pegging a core forever. Stores default to a never-tripping deadline, so
    /// a store that's never armed simply never traps. Do NOT pass `u64::MAX` to
    /// "disable" — it's added to the current epoch and overflows once the host's
    /// epoch ticker has advanced (see `NO_EPOCH_DEADLINE`); pass a large finite
    /// delta if you need to widen it.
    pub fn set_epoch_deadline(&mut self, ticks_until_trap: u64) {
        self.store.set_epoch_deadline(ticks_until_trap);
    }

    /// The guest memory: the host-owned one (PIC), else the exported "memory".
    fn get_memory(&mut self) -> Result<Memory, RuntimeError> {
        if let Some(mem) = self.memory {
            return Ok(mem);
        }
        self.instance
            .get_memory(&mut self.store, "memory")
            .ok_or_else(|| RuntimeError::MemoryError("no exported memory named 'memory'".into()))
    }

    /// Write bytes to the instance's memory at the given offset.
    pub fn write_memory(&mut self, offset: usize, data: &[u8]) -> Result<(), RuntimeError> {
        let memory = self.get_memory()?;
        memory
            .write(&mut self.store, offset, data)
            .map_err(|e| RuntimeError::MemoryError(e.to_string()))
    }

    /// Read bytes from the instance's memory.
    pub fn read_memory(&mut self, offset: usize, len: usize) -> Result<Vec<u8>, RuntimeError> {
        let memory = self.get_memory()?;
        let mut buffer = vec![0u8; len];
        memory
            .read(&self.store, offset, &mut buffer)
            .map_err(|e| RuntimeError::MemoryError(e.to_string()))?;
        Ok(buffer)
    }

    /// Get the current memory size in bytes.
    pub fn memory_size(&mut self) -> Result<usize, RuntimeError> {
        let memory = self.get_memory()?;
        Ok(memory.data_size(&self.store))
    }

    /// Encode a Value and write it to memory at the given offset.
    pub fn write_value(&mut self, offset: usize, value: &Value) -> Result<usize, RuntimeError> {
        let bytes = encode(value).map_err(|e| RuntimeError::AbiError(e.to_string()))?;
        self.write_memory(offset, &bytes)?;
        Ok(bytes.len())
    }

    /// Read bytes from memory and decode them as a Value.
    pub fn read_value(&mut self, offset: usize, len: usize) -> Result<Value, RuntimeError> {
        let bytes = self.read_memory(offset, len)?;
        decode(&bytes).map_err(|e| RuntimeError::AbiError(e.to_string()))
    }

    /// Set a call interceptor for recording/replaying export function calls.
    pub fn set_interceptor(&mut self, interceptor: Arc<dyn CallInterceptor>) {
        self.interceptor = Some(interceptor);
    }

    /// Get the current interceptor, if any.
    pub fn interceptor(&self) -> Option<&Arc<dyn CallInterceptor>> {
        self.interceptor.as_ref()
    }

    /// Call a function using the Pack ABI (async).
    ///
    /// If the guest exports `__pack_alloc`, input is dynamically allocated.
    /// Otherwise, falls back to a fixed input buffer at INPUT_BUFFER_OFFSET.
    ///
    /// The WASM function signature is `(in_ptr, in_len, out_ptr_ptr, out_len_ptr) -> status`:
    /// - Returns: 0 on success, -1 on error (error message in ptr/len)
    pub async fn call_with_value_async(
        &mut self,
        name: &str,
        input: &Value,
    ) -> Result<Value, RuntimeError> {
        // Check interceptor for short-circuit (replay)
        if let Some(ref interceptor) = self.interceptor {
            if let Some(recorded_output) = interceptor.before_export(name, input).await {
                interceptor
                    .after_export(name, input, &recorded_output)
                    .await;
                return Ok(recorded_output);
            }
        }

        // Encode input
        let input_bytes = encode(input).map_err(|e| RuntimeError::AbiError(e.to_string()))?;

        // Try to allocate input buffer dynamically, fall back to fixed buffer
        let (in_ptr, dynamic_input) = match self.call_pack_alloc_async(input_bytes.len()).await {
            Ok(ptr) => (ptr, true),
            Err(_) => (INPUT_BUFFER_OFFSET, false),
        };

        // Write input to buffer
        let memory = self.get_memory()?;
        memory
            .write(&mut self.store, in_ptr, &input_bytes)
            .map_err(|_| RuntimeError::MemoryError("Failed to write input".into()))?;

        // Call the function
        let func = self
            .instance
            .get_typed_func::<(i32, i32, i32, i32), i32>(&mut self.store, name)
            .map_err(|e| RuntimeError::FunctionNotFound(e.to_string()))?;

        let status = func
            .call_async(
                &mut self.store,
                (
                    in_ptr as i32,
                    input_bytes.len() as i32,
                    RESULT_PTR_OFFSET as i32,
                    RESULT_LEN_OFFSET as i32,
                ),
            )
            .await
            .map_err(|e| RuntimeError::WasmError(e.to_string()))?;

        // Free the input buffer if dynamically allocated
        if dynamic_input {
            self.call_pack_free_async(in_ptr, input_bytes.len())
                .await
                .ok();
        }

        // Read result ptr/len from slots
        let memory = self.get_memory()?;
        let mut ptr_bytes = [0u8; 4];
        let mut len_bytes = [0u8; 4];
        memory
            .read(&self.store, RESULT_PTR_OFFSET, &mut ptr_bytes)
            .map_err(|_| RuntimeError::MemoryError("Failed to read result ptr".into()))?;
        memory
            .read(&self.store, RESULT_LEN_OFFSET, &mut len_bytes)
            .map_err(|_| RuntimeError::MemoryError("Failed to read result len".into()))?;

        let out_ptr = i32::from_le_bytes(ptr_bytes) as usize;
        let out_len = i32::from_le_bytes(len_bytes) as usize;

        // Check for error
        if status != 0 {
            // Read error message
            let mut err_bytes = vec![0u8; out_len];
            memory
                .read(&self.store, out_ptr, &mut err_bytes)
                .map_err(|_| RuntimeError::MemoryError("Failed to read error".into()))?;

            // Free the error buffer
            self.call_pack_free_async(out_ptr, out_len).await.ok();

            let err_msg = String::from_utf8_lossy(&err_bytes).to_string();
            return Err(RuntimeError::WasmError(format!(
                "function '{}' returned error: {}",
                name, err_msg
            )));
        }

        // Read output value
        let result = self.read_value(out_ptr, out_len)?;

        // Free the guest's output buffer if guest has __pack_free
        self.call_pack_free_async(out_ptr, out_len).await.ok();

        // Notify interceptor of completed export call
        if let Some(ref interceptor) = self.interceptor {
            interceptor.after_export(name, input, &result).await;
        }

        Ok(result)
    }

    /// Call __pack_alloc to allocate a buffer in guest memory (async).
    async fn call_pack_alloc_async(&mut self, size: usize) -> Result<usize, RuntimeError> {
        let alloc_func = self
            .instance
            .get_typed_func::<i32, i32>(&mut self.store, "__pack_alloc")
            .map_err(|_| RuntimeError::FunctionNotFound("__pack_alloc not found".into()))?;

        let ptr = alloc_func
            .call_async(&mut self.store, size as i32)
            .await
            .map_err(|e| RuntimeError::WasmError(e.to_string()))?;

        if ptr == 0 {
            return Err(RuntimeError::MemoryError("Guest allocation failed".into()));
        }

        Ok(ptr as usize)
    }

    /// Call __pack_free to free a guest-allocated buffer (async).
    async fn call_pack_free_async(&mut self, ptr: usize, len: usize) -> Result<(), RuntimeError> {
        if let Ok(free_func) = self
            .instance
            .get_typed_func::<(i32, i32), ()>(&mut self.store, "__pack_free")
        {
            free_func
                .call_async(&mut self.store, (ptr as i32, len as i32))
                .await
                .map_err(|e| RuntimeError::WasmError(e.to_string()))?;
        }
        Ok(())
    }

    /// Call an exported function that takes two i32s and returns an i32 (async).
    pub async fn call_i32_i32_to_i32_async(
        &mut self,
        name: &str,
        a: i32,
        b: i32,
    ) -> Result<i32, RuntimeError> {
        let func = self
            .instance
            .get_typed_func::<(i32, i32), i32>(&mut self.store, name)
            .map_err(|e| RuntimeError::FunctionNotFound(e.to_string()))?;

        func.call_async(&mut self.store, (a, b))
            .await
            .map_err(|e| RuntimeError::WasmError(e.to_string()))
    }

    /// Read embedded type metadata from the package (async).
    ///
    /// Calls the `__pack_types` export to get CGRF-encoded metadata describing
    /// the package's imports and exports. Returns `Err(MetadataError::NotFound)`
    /// if the package doesn't export `__pack_types`.
    pub async fn types(
        &mut self,
    ) -> Result<crate::metadata::PackageMetadata, crate::metadata::MetadataError> {
        let types_func = self
            .instance
            .get_typed_func::<(i32, i32), i32>(&mut self.store, "__pack_types")
            .map_err(|_| crate::metadata::MetadataError::NotFound)?;

        let status = types_func
            .call_async(
                &mut self.store,
                (RESULT_PTR_OFFSET as i32, RESULT_LEN_OFFSET as i32),
            )
            .await
            .map_err(|e| crate::metadata::MetadataError::CallFailed(e.to_string()))?;

        if status != 0 {
            return Err(crate::metadata::MetadataError::CallFailed(
                "non-zero status from __pack_types".into(),
            ));
        }

        let memory = self
            .get_memory()
            .map_err(|e| crate::metadata::MetadataError::CallFailed(e.to_string()))?;
        let mut ptr_bytes = [0u8; 4];
        let mut len_bytes = [0u8; 4];
        memory
            .read(&self.store, RESULT_PTR_OFFSET, &mut ptr_bytes)
            .map_err(|e| crate::metadata::MetadataError::CallFailed(e.to_string()))?;
        memory
            .read(&self.store, RESULT_LEN_OFFSET, &mut len_bytes)
            .map_err(|e| crate::metadata::MetadataError::CallFailed(e.to_string()))?;

        let out_ptr = i32::from_le_bytes(ptr_bytes) as usize;
        let out_len = i32::from_le_bytes(len_bytes) as usize;

        // Read metadata bytes (static data, no __pack_free needed)
        let mut metadata_bytes = vec![0u8; out_len];
        memory
            .read(&self.store, out_ptr, &mut metadata_bytes)
            .map_err(|e| crate::metadata::MetadataError::CallFailed(e.to_string()))?;

        crate::metadata::decode_metadata(&metadata_bytes)
    }

    /// Read embedded type metadata with interface hashes from the package (async).
    ///
    /// Like `types()`, but also decodes interface hashes for compatibility checking.
    pub async fn types_with_hashes(
        &mut self,
    ) -> Result<crate::metadata::MetadataWithHashes, crate::metadata::MetadataError> {
        let types_func = self
            .instance
            .get_typed_func::<(i32, i32), i32>(&mut self.store, "__pack_types")
            .map_err(|_| crate::metadata::MetadataError::NotFound)?;

        let status = types_func
            .call_async(
                &mut self.store,
                (RESULT_PTR_OFFSET as i32, RESULT_LEN_OFFSET as i32),
            )
            .await
            .map_err(|e| crate::metadata::MetadataError::CallFailed(e.to_string()))?;

        if status != 0 {
            return Err(crate::metadata::MetadataError::CallFailed(
                "non-zero status from __pack_types".into(),
            ));
        }

        let memory = self
            .get_memory()
            .map_err(|e| crate::metadata::MetadataError::CallFailed(e.to_string()))?;
        let mut ptr_bytes = [0u8; 4];
        let mut len_bytes = [0u8; 4];
        memory
            .read(&self.store, RESULT_PTR_OFFSET, &mut ptr_bytes)
            .map_err(|e| crate::metadata::MetadataError::CallFailed(e.to_string()))?;
        memory
            .read(&self.store, RESULT_LEN_OFFSET, &mut len_bytes)
            .map_err(|e| crate::metadata::MetadataError::CallFailed(e.to_string()))?;

        let out_ptr = i32::from_le_bytes(ptr_bytes) as usize;
        let out_len = i32::from_le_bytes(len_bytes) as usize;

        // Read metadata bytes (static data, no __pack_free needed)
        let mut metadata_bytes = vec![0u8; out_len];
        memory
            .read(&self.store, out_ptr, &mut metadata_bytes)
            .map_err(|e| crate::metadata::MetadataError::CallFailed(e.to_string()))?;

        crate::metadata::decode_metadata_with_hashes(&metadata_bytes)
    }
}

/// Type alias for async host function return type.
pub type AsyncHostFnResult<R> = Pin<Box<dyn Future<Output = R> + Send + 'static>>;

/// A compiled WASM module, ready to be instantiated
pub struct CompiledModule<'a> {
    module: Module,
    engine: &'a Engine,
}

impl CompiledModule<'_> {
    /// Instantiate the module with no imports
    pub fn instantiate(&self) -> Result<Instance<()>, RuntimeError> {
        let mut store = Store::new(self.engine, ());
        let mut linker = Linker::<()>::new(self.engine);
        register_default_alloc(&mut linker)?;

        let instance = linker
            .instantiate(&mut store, &self.module)
            .map_err(|e| RuntimeError::WasmError(e.to_string()))?;

        Ok(Instance { store, instance })
    }

    /// Instantiate the module with host imports (backward compatible API)
    ///
    /// This method provides the default "host" module with `log` and `alloc` functions.
    /// For custom host functions, use `instantiate_with_host()` instead.
    pub fn instantiate_with_imports(
        &self,
        imports: HostImports,
    ) -> Result<InstanceWithHost, RuntimeError> {
        let state = imports.state.clone();
        let mut linker = Linker::<HostState>::new(self.engine);
        register_default_alloc(&mut linker)?;

        // Use the new provider-based registration
        let mut builder = HostLinkerBuilder::new(self.engine, &mut linker);
        DefaultHostProvider
            .register(&mut builder)
            .map_err(|e| RuntimeError::WasmError(e.to_string()))?;

        let mut store = Store::new(self.engine, state.clone());
        let instance = linker
            .instantiate(&mut store, &self.module)
            .map_err(|e| RuntimeError::WasmError(e.to_string()))?;

        Ok(InstanceWithHost {
            store,
            instance,
            state,
        })
    }

    /// Instantiate the module with a pre-configured linker.
    ///
    /// This is the most flexible instantiation method, allowing full control
    /// over the linker configuration.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let mut linker = Linker::new(&engine);
    /// let mut builder = HostLinkerBuilder::new(&engine, &mut linker);
    ///
    /// builder.interface("my:api/v1")?
    ///     .func_raw("process", |caller, ptr, len| { ... })?;
    ///
    /// let instance = module.instantiate_with_linker(linker, MyState::new())?;
    /// ```
    pub fn instantiate_with_linker<T: 'static>(
        &self,
        linker: Linker<T>,
        state: T,
    ) -> Result<Instance<T>, RuntimeError> {
        let mut store = Store::new(self.engine, state);

        let instance = linker
            .instantiate(&mut store, &self.module)
            .map_err(|e| RuntimeError::WasmError(e.to_string()))?;

        Ok(Instance { store, instance })
    }

    /// Instantiate the module with a builder function for configuring host functions.
    ///
    /// This is the recommended method for Theater-style integration, providing
    /// an ergonomic API for registering namespaced interfaces.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let instance = module.instantiate_with_host(MyState::new(), |builder| {
    ///     builder.interface("theater:simple/runtime")?
    ///         .func_raw("log", |caller, ptr, len| { ... })?;
    ///     Ok(())
    /// })?;
    /// ```
    pub fn instantiate_with_host<T, F>(
        &self,
        state: T,
        configure: F,
    ) -> Result<Instance<T>, RuntimeError>
    where
        T: 'static,
        F: FnOnce(&mut HostLinkerBuilder<'_, T>) -> Result<(), LinkerError>,
    {
        let mut linker = Linker::new(self.engine);
        register_default_alloc(&mut linker)?;
        let mut builder = HostLinkerBuilder::new(self.engine, &mut linker);
        configure(&mut builder).map_err(|e| RuntimeError::WasmError(e.to_string()))?;

        self.instantiate_with_linker(linker, state)
    }

    /// Get a reference to the engine
    pub fn engine(&self) -> &Engine {
        self.engine
    }
}

/// A running WASM instance
pub struct Instance<T> {
    store: Store<T>,
    instance: WasmtimeInstance,
}

/// Instance with host imports - provides access to host state
pub struct InstanceWithHost {
    store: Store<HostState>,
    instance: WasmtimeInstance,
    state: HostState,
}

impl InstanceWithHost {
    /// Validate that this instance implements the given interface
    ///
    /// Checks that all required functions exist with correct signatures.
    pub fn validate_interface(&mut self, interface: &Interface) -> Result<(), InterfaceError> {
        validate_instance_implements_interface(&mut self.store, &self.instance, interface)
    }

    /// Get the host state (for reading logs, etc.)
    pub fn host_state(&self) -> &HostState {
        &self.state
    }

    /// Get all log messages from the package
    pub fn get_logs(&self) -> Vec<String> {
        self.state.get_logs()
    }

    /// Clear log messages
    pub fn clear_logs(&self) {
        self.state.clear_logs()
    }

    /// Get the exported memory (assumes it's named "memory")
    fn get_memory(&mut self) -> Result<Memory, RuntimeError> {
        self.instance
            .get_memory(&mut self.store, "memory")
            .ok_or_else(|| RuntimeError::MemoryError("no exported memory named 'memory'".into()))
    }

    /// Write bytes to the instance's memory at the given offset
    pub fn write_memory(&mut self, offset: usize, data: &[u8]) -> Result<(), RuntimeError> {
        let memory = self.get_memory()?;
        memory
            .write(&mut self.store, offset, data)
            .map_err(|e| RuntimeError::MemoryError(e.to_string()))
    }

    /// Read bytes from the instance's memory
    pub fn read_memory(&mut self, offset: usize, len: usize) -> Result<Vec<u8>, RuntimeError> {
        let memory = self.get_memory()?;
        let mut buffer = vec![0u8; len];
        memory
            .read(&self.store, offset, &mut buffer)
            .map_err(|e| RuntimeError::MemoryError(e.to_string()))?;
        Ok(buffer)
    }

    /// Get the current memory size in bytes
    pub fn memory_size(&mut self) -> Result<usize, RuntimeError> {
        let memory = self.get_memory()?;
        Ok(memory.data_size(&self.store))
    }

    /// Call an exported function that takes two i32s and returns an i32
    pub fn call_i32_i32_to_i32(&mut self, name: &str, a: i32, b: i32) -> Result<i32, RuntimeError> {
        let func = self
            .instance
            .get_typed_func::<(i32, i32), i32>(&mut self.store, name)
            .map_err(|e| RuntimeError::FunctionNotFound(e.to_string()))?;

        func.call(&mut self.store, (a, b))
            .map_err(|e| RuntimeError::WasmError(e.to_string()))
    }

    /// Call an exported function that takes two i64s and returns an i64
    pub fn call_i64_i64_to_i64(&mut self, name: &str, a: i64, b: i64) -> Result<i64, RuntimeError> {
        let func = self
            .instance
            .get_typed_func::<(i64, i64), i64>(&mut self.store, name)
            .map_err(|e| RuntimeError::FunctionNotFound(e.to_string()))?;

        func.call(&mut self.store, (a, b))
            .map_err(|e| RuntimeError::WasmError(e.to_string()))
    }

    /// Call an exported function that takes two i32s and returns nothing
    pub fn call_i32_i32(&mut self, name: &str, a: i32, b: i32) -> Result<(), RuntimeError> {
        let func = self
            .instance
            .get_typed_func::<(i32, i32), ()>(&mut self.store, name)
            .map_err(|e| RuntimeError::FunctionNotFound(e.to_string()))?;

        func.call(&mut self.store, (a, b))
            .map_err(|e| RuntimeError::WasmError(e.to_string()))
    }

    /// Encode a Value and write it to memory at the given offset.
    pub fn write_value(&mut self, offset: usize, value: &Value) -> Result<usize, RuntimeError> {
        let bytes = encode(value).map_err(|e| RuntimeError::AbiError(e.to_string()))?;
        self.write_memory(offset, &bytes)?;
        Ok(bytes.len())
    }

    /// Read bytes from memory and decode them as a Value.
    pub fn read_value(&mut self, offset: usize, len: usize) -> Result<Value, RuntimeError> {
        let bytes = self.read_memory(offset, len)?;
        decode(&bytes).map_err(|e| RuntimeError::AbiError(e.to_string()))
    }

    /// Call a function using the Pack ABI.
    ///
    /// If the guest exports `__pack_alloc`, input is dynamically allocated.
    /// Otherwise, falls back to a fixed input buffer at INPUT_BUFFER_OFFSET.
    ///
    /// The WASM function signature is `(in_ptr, in_len, out_ptr_ptr, out_len_ptr) -> status`:
    /// - Returns: 0 on success, -1 on error (error message in ptr/len)
    pub fn call_with_value(&mut self, name: &str, input: &Value) -> Result<Value, RuntimeError> {
        // Encode input
        let input_bytes = encode(input).map_err(|e| RuntimeError::AbiError(e.to_string()))?;

        // Try to allocate input buffer dynamically, fall back to fixed buffer
        let (in_ptr, dynamic_input) = match self.call_pack_alloc(input_bytes.len()) {
            Ok(ptr) => (ptr, true),
            Err(_) => (INPUT_BUFFER_OFFSET, false),
        };

        // Write input to buffer
        let memory = self.get_memory()?;
        memory
            .write(&mut self.store, in_ptr, &input_bytes)
            .map_err(|_| RuntimeError::MemoryError("Failed to write input".into()))?;

        // Call the function
        let func = self
            .instance
            .get_typed_func::<(i32, i32, i32, i32), i32>(&mut self.store, name)
            .map_err(|e| RuntimeError::FunctionNotFound(e.to_string()))?;

        let status = func
            .call(
                &mut self.store,
                (
                    in_ptr as i32,
                    input_bytes.len() as i32,
                    RESULT_PTR_OFFSET as i32,
                    RESULT_LEN_OFFSET as i32,
                ),
            )
            .map_err(|e| RuntimeError::WasmError(e.to_string()))?;

        // Free the input buffer if dynamically allocated
        if dynamic_input {
            self.call_pack_free(in_ptr, input_bytes.len()).ok();
        }

        // Read result ptr/len from slots
        let memory = self.get_memory()?;
        let mut ptr_bytes = [0u8; 4];
        let mut len_bytes = [0u8; 4];
        memory
            .read(&self.store, RESULT_PTR_OFFSET, &mut ptr_bytes)
            .map_err(|_| RuntimeError::MemoryError("Failed to read result ptr".into()))?;
        memory
            .read(&self.store, RESULT_LEN_OFFSET, &mut len_bytes)
            .map_err(|_| RuntimeError::MemoryError("Failed to read result len".into()))?;

        let out_ptr = i32::from_le_bytes(ptr_bytes) as usize;
        let out_len = i32::from_le_bytes(len_bytes) as usize;

        // Check for error
        if status != 0 {
            // Read error message
            let mut err_bytes = vec![0u8; out_len];
            memory
                .read(&self.store, out_ptr, &mut err_bytes)
                .map_err(|_| RuntimeError::MemoryError("Failed to read error".into()))?;

            // Free the error buffer
            self.call_pack_free(out_ptr, out_len).ok();

            let err_msg = String::from_utf8_lossy(&err_bytes).to_string();
            return Err(RuntimeError::WasmError(format!(
                "function '{}' returned error: {}",
                name, err_msg
            )));
        }

        // Read output value
        let result = self.read_value(out_ptr, out_len)?;

        // Free the guest's output buffer if guest has __pack_free
        self.call_pack_free(out_ptr, out_len).ok();

        Ok(result)
    }

    /// Call __pack_alloc to allocate a buffer in guest memory.
    fn call_pack_alloc(&mut self, size: usize) -> Result<usize, RuntimeError> {
        let alloc_func = self
            .instance
            .get_typed_func::<i32, i32>(&mut self.store, "__pack_alloc")
            .map_err(|_| RuntimeError::FunctionNotFound("__pack_alloc not found".into()))?;

        let ptr = alloc_func
            .call(&mut self.store, size as i32)
            .map_err(|e| RuntimeError::WasmError(e.to_string()))?;

        if ptr == 0 {
            return Err(RuntimeError::MemoryError("Guest allocation failed".into()));
        }

        Ok(ptr as usize)
    }

    /// Call __pack_free to free a guest-allocated buffer.
    fn call_pack_free(&mut self, ptr: usize, len: usize) -> Result<(), RuntimeError> {
        if let Ok(free_func) = self
            .instance
            .get_typed_func::<(i32, i32), ()>(&mut self.store, "__pack_free")
        {
            free_func
                .call(&mut self.store, (ptr as i32, len as i32))
                .map_err(|e| RuntimeError::WasmError(e.to_string()))?;
        }
        Ok(())
    }

    /// Read embedded type metadata from the package.
    ///
    /// Calls the `__pack_types` export to get CGRF-encoded metadata describing
    /// the package's imports and exports. Returns `Err(MetadataError::NotFound)`
    /// if the package doesn't export `__pack_types`.
    pub fn types(
        &mut self,
    ) -> Result<crate::metadata::PackageMetadata, crate::metadata::MetadataError> {
        let types_func = self
            .instance
            .get_typed_func::<(i32, i32), i32>(&mut self.store, "__pack_types")
            .map_err(|_| crate::metadata::MetadataError::NotFound)?;

        let status = types_func
            .call(
                &mut self.store,
                (RESULT_PTR_OFFSET as i32, RESULT_LEN_OFFSET as i32),
            )
            .map_err(|e| crate::metadata::MetadataError::CallFailed(e.to_string()))?;

        if status != 0 {
            return Err(crate::metadata::MetadataError::CallFailed(
                "non-zero status from __pack_types".into(),
            ));
        }

        let memory = self
            .get_memory()
            .map_err(|e| crate::metadata::MetadataError::CallFailed(e.to_string()))?;
        let mut ptr_bytes = [0u8; 4];
        let mut len_bytes = [0u8; 4];
        memory
            .read(&self.store, RESULT_PTR_OFFSET, &mut ptr_bytes)
            .map_err(|e| crate::metadata::MetadataError::CallFailed(e.to_string()))?;
        memory
            .read(&self.store, RESULT_LEN_OFFSET, &mut len_bytes)
            .map_err(|e| crate::metadata::MetadataError::CallFailed(e.to_string()))?;

        let out_ptr = i32::from_le_bytes(ptr_bytes) as usize;
        let out_len = i32::from_le_bytes(len_bytes) as usize;

        // Read metadata bytes (static data, no __pack_free needed)
        let mut metadata_bytes = vec![0u8; out_len];
        memory
            .read(&self.store, out_ptr, &mut metadata_bytes)
            .map_err(|e| crate::metadata::MetadataError::CallFailed(e.to_string()))?;

        crate::metadata::decode_metadata(&metadata_bytes)
    }

    /// Read embedded type metadata with interface hashes from the package.
    ///
    /// Like `types()`, but also decodes interface hashes for compatibility checking.
    pub fn types_with_hashes(
        &mut self,
    ) -> Result<crate::metadata::MetadataWithHashes, crate::metadata::MetadataError> {
        let types_func = self
            .instance
            .get_typed_func::<(i32, i32), i32>(&mut self.store, "__pack_types")
            .map_err(|_| crate::metadata::MetadataError::NotFound)?;

        let status = types_func
            .call(
                &mut self.store,
                (RESULT_PTR_OFFSET as i32, RESULT_LEN_OFFSET as i32),
            )
            .map_err(|e| crate::metadata::MetadataError::CallFailed(e.to_string()))?;

        if status != 0 {
            return Err(crate::metadata::MetadataError::CallFailed(
                "non-zero status from __pack_types".into(),
            ));
        }

        let memory = self
            .get_memory()
            .map_err(|e| crate::metadata::MetadataError::CallFailed(e.to_string()))?;
        let mut ptr_bytes = [0u8; 4];
        let mut len_bytes = [0u8; 4];
        memory
            .read(&self.store, RESULT_PTR_OFFSET, &mut ptr_bytes)
            .map_err(|e| crate::metadata::MetadataError::CallFailed(e.to_string()))?;
        memory
            .read(&self.store, RESULT_LEN_OFFSET, &mut len_bytes)
            .map_err(|e| crate::metadata::MetadataError::CallFailed(e.to_string()))?;

        let out_ptr = i32::from_le_bytes(ptr_bytes) as usize;
        let out_len = i32::from_le_bytes(len_bytes) as usize;

        // Read metadata bytes (static data, no __pack_free needed)
        let mut metadata_bytes = vec![0u8; out_len];
        memory
            .read(&self.store, out_ptr, &mut metadata_bytes)
            .map_err(|e| crate::metadata::MetadataError::CallFailed(e.to_string()))?;

        crate::metadata::decode_metadata_with_hashes(&metadata_bytes)
    }
}

// Implement Instance methods for both () and HostState
impl<T> Instance<T> {
    /// Validate that this instance implements the given interface
    ///
    /// Checks that all required functions exist with correct signatures.
    pub fn validate_interface(&mut self, interface: &Interface) -> Result<(), InterfaceError> {
        validate_instance_implements_interface(&mut self.store, &self.instance, interface)
    }

    /// Get the exported memory (assumes it's named "memory")
    fn get_memory(&mut self) -> Result<Memory, RuntimeError> {
        self.instance
            .get_memory(&mut self.store, "memory")
            .ok_or_else(|| RuntimeError::MemoryError("no exported memory named 'memory'".into()))
    }

    /// Write bytes to the instance's memory at the given offset
    pub fn write_memory(&mut self, offset: usize, data: &[u8]) -> Result<(), RuntimeError> {
        let memory = self.get_memory()?;
        memory
            .write(&mut self.store, offset, data)
            .map_err(|e| RuntimeError::MemoryError(e.to_string()))
    }

    /// Read bytes from the instance's memory
    pub fn read_memory(&mut self, offset: usize, len: usize) -> Result<Vec<u8>, RuntimeError> {
        let memory = self.get_memory()?;
        let mut buffer = vec![0u8; len];
        memory
            .read(&self.store, offset, &mut buffer)
            .map_err(|e| RuntimeError::MemoryError(e.to_string()))?;
        Ok(buffer)
    }

    /// Get the current memory size in bytes
    pub fn memory_size(&mut self) -> Result<usize, RuntimeError> {
        let memory = self.get_memory()?;
        Ok(memory.data_size(&self.store))
    }

    /// Call an exported function that takes two i32s and returns an i32
    pub fn call_i32_i32_to_i32(&mut self, name: &str, a: i32, b: i32) -> Result<i32, RuntimeError> {
        let func = self
            .instance
            .get_typed_func::<(i32, i32), i32>(&mut self.store, name)
            .map_err(|e| RuntimeError::FunctionNotFound(e.to_string()))?;

        func.call(&mut self.store, (a, b))
            .map_err(|e| RuntimeError::WasmError(e.to_string()))
    }

    /// Call an exported function that takes two i64s and returns an i64
    pub fn call_i64_i64_to_i64(&mut self, name: &str, a: i64, b: i64) -> Result<i64, RuntimeError> {
        let func = self
            .instance
            .get_typed_func::<(i64, i64), i64>(&mut self.store, name)
            .map_err(|e| RuntimeError::FunctionNotFound(e.to_string()))?;

        func.call(&mut self.store, (a, b))
            .map_err(|e| RuntimeError::WasmError(e.to_string()))
    }

    /// Call an exported function that takes two i32s and returns nothing
    pub fn call_i32_i32(&mut self, name: &str, a: i32, b: i32) -> Result<(), RuntimeError> {
        let func = self
            .instance
            .get_typed_func::<(i32, i32), ()>(&mut self.store, name)
            .map_err(|e| RuntimeError::FunctionNotFound(e.to_string()))?;

        func.call(&mut self.store, (a, b))
            .map_err(|e| RuntimeError::WasmError(e.to_string()))
    }

    // ========================================================================
    // Graph ABI helpers
    // ========================================================================

    /// Encode a Value and write it to memory at the given offset.
    /// Returns the number of bytes written.
    pub fn write_value(&mut self, offset: usize, value: &Value) -> Result<usize, RuntimeError> {
        let bytes = encode(value).map_err(|e| RuntimeError::AbiError(e.to_string()))?;
        self.write_memory(offset, &bytes)?;
        Ok(bytes.len())
    }

    /// Read bytes from memory and decode them as a Value.
    pub fn read_value(&mut self, offset: usize, len: usize) -> Result<Value, RuntimeError> {
        let bytes = self.read_memory(offset, len)?;
        decode(&bytes).map_err(|e| RuntimeError::AbiError(e.to_string()))
    }

    /// Call a function using the Pack ABI.
    ///
    /// If the guest exports `__pack_alloc`, input is dynamically allocated.
    /// Otherwise, falls back to a fixed input buffer at INPUT_BUFFER_OFFSET.
    ///
    /// The WASM function signature is `(in_ptr, in_len, out_ptr_ptr, out_len_ptr) -> status`:
    /// - Returns: 0 on success, -1 on error (error message in ptr/len)
    pub fn call_with_value(&mut self, name: &str, input: &Value) -> Result<Value, RuntimeError> {
        // Encode input
        let input_bytes = encode(input).map_err(|e| RuntimeError::AbiError(e.to_string()))?;

        // Try to allocate input buffer dynamically, fall back to fixed buffer
        let (in_ptr, dynamic_input) = match self.call_pack_alloc(input_bytes.len()) {
            Ok(ptr) => (ptr, true),
            Err(_) => (INPUT_BUFFER_OFFSET, false),
        };

        // Write input to buffer
        let memory = self.get_memory()?;
        memory
            .write(&mut self.store, in_ptr, &input_bytes)
            .map_err(|_| RuntimeError::MemoryError("Failed to write input".into()))?;

        // Call the function
        let func = self
            .instance
            .get_typed_func::<(i32, i32, i32, i32), i32>(&mut self.store, name)
            .map_err(|e| RuntimeError::FunctionNotFound(e.to_string()))?;

        let status = func
            .call(
                &mut self.store,
                (
                    in_ptr as i32,
                    input_bytes.len() as i32,
                    RESULT_PTR_OFFSET as i32,
                    RESULT_LEN_OFFSET as i32,
                ),
            )
            .map_err(|e| RuntimeError::WasmError(e.to_string()))?;

        // Free the input buffer if dynamically allocated
        if dynamic_input {
            self.call_pack_free(in_ptr, input_bytes.len()).ok();
        }

        // Read result ptr/len from slots
        let memory = self.get_memory()?;
        let mut ptr_bytes = [0u8; 4];
        let mut len_bytes = [0u8; 4];
        memory
            .read(&self.store, RESULT_PTR_OFFSET, &mut ptr_bytes)
            .map_err(|_| RuntimeError::MemoryError("Failed to read result ptr".into()))?;
        memory
            .read(&self.store, RESULT_LEN_OFFSET, &mut len_bytes)
            .map_err(|_| RuntimeError::MemoryError("Failed to read result len".into()))?;

        let out_ptr = i32::from_le_bytes(ptr_bytes) as usize;
        let out_len = i32::from_le_bytes(len_bytes) as usize;

        // Check for error
        if status != 0 {
            // Read error message
            let mut err_bytes = vec![0u8; out_len];
            memory
                .read(&self.store, out_ptr, &mut err_bytes)
                .map_err(|_| RuntimeError::MemoryError("Failed to read error".into()))?;

            // Free the error buffer
            self.call_pack_free(out_ptr, out_len).ok();

            let err_msg = String::from_utf8_lossy(&err_bytes).to_string();
            return Err(RuntimeError::WasmError(format!(
                "function '{}' returned error: {}",
                name, err_msg
            )));
        }

        // Read and decode output
        let result = self.read_value(out_ptr, out_len)?;

        // Free the guest's output buffer if guest has __pack_free
        self.call_pack_free(out_ptr, out_len).ok();

        Ok(result)
    }

    /// Call __pack_alloc to allocate a buffer in guest memory.
    fn call_pack_alloc(&mut self, size: usize) -> Result<usize, RuntimeError> {
        let alloc_func = self
            .instance
            .get_typed_func::<i32, i32>(&mut self.store, "__pack_alloc")
            .map_err(|_| RuntimeError::FunctionNotFound("__pack_alloc not found".into()))?;

        let ptr = alloc_func
            .call(&mut self.store, size as i32)
            .map_err(|e| RuntimeError::WasmError(e.to_string()))?;

        if ptr == 0 {
            return Err(RuntimeError::MemoryError("Guest allocation failed".into()));
        }

        Ok(ptr as usize)
    }

    /// Call __pack_free to free a guest-allocated buffer.
    fn call_pack_free(&mut self, ptr: usize, len: usize) -> Result<(), RuntimeError> {
        if let Ok(free_func) = self
            .instance
            .get_typed_func::<(i32, i32), ()>(&mut self.store, "__pack_free")
        {
            free_func
                .call(&mut self.store, (ptr as i32, len as i32))
                .map_err(|e| RuntimeError::WasmError(e.to_string()))?;
        }
        Ok(())
    }

    /// Read embedded type metadata from the package.
    ///
    /// Calls the `__pack_types` export to get CGRF-encoded metadata describing
    /// the package's imports and exports. Returns `Err(MetadataError::NotFound)`
    /// if the package doesn't export `__pack_types`.
    pub fn types(
        &mut self,
    ) -> Result<crate::metadata::PackageMetadata, crate::metadata::MetadataError> {
        let types_func = self
            .instance
            .get_typed_func::<(i32, i32), i32>(&mut self.store, "__pack_types")
            .map_err(|_| crate::metadata::MetadataError::NotFound)?;

        let status = types_func
            .call(
                &mut self.store,
                (RESULT_PTR_OFFSET as i32, RESULT_LEN_OFFSET as i32),
            )
            .map_err(|e| crate::metadata::MetadataError::CallFailed(e.to_string()))?;

        if status != 0 {
            return Err(crate::metadata::MetadataError::CallFailed(
                "non-zero status from __pack_types".into(),
            ));
        }

        let memory = self
            .get_memory()
            .map_err(|e| crate::metadata::MetadataError::CallFailed(e.to_string()))?;
        let mut ptr_bytes = [0u8; 4];
        let mut len_bytes = [0u8; 4];
        memory
            .read(&self.store, RESULT_PTR_OFFSET, &mut ptr_bytes)
            .map_err(|e| crate::metadata::MetadataError::CallFailed(e.to_string()))?;
        memory
            .read(&self.store, RESULT_LEN_OFFSET, &mut len_bytes)
            .map_err(|e| crate::metadata::MetadataError::CallFailed(e.to_string()))?;

        let out_ptr = i32::from_le_bytes(ptr_bytes) as usize;
        let out_len = i32::from_le_bytes(len_bytes) as usize;

        // Read metadata bytes (static data, no __pack_free needed)
        let mut metadata_bytes = vec![0u8; out_len];
        memory
            .read(&self.store, out_ptr, &mut metadata_bytes)
            .map_err(|e| crate::metadata::MetadataError::CallFailed(e.to_string()))?;

        crate::metadata::decode_metadata(&metadata_bytes)
    }

    /// Read embedded type metadata with interface hashes from the package.
    ///
    /// Like `types()`, but also decodes interface hashes for compatibility checking.
    pub fn types_with_hashes(
        &mut self,
    ) -> Result<crate::metadata::MetadataWithHashes, crate::metadata::MetadataError> {
        let types_func = self
            .instance
            .get_typed_func::<(i32, i32), i32>(&mut self.store, "__pack_types")
            .map_err(|_| crate::metadata::MetadataError::NotFound)?;

        let status = types_func
            .call(
                &mut self.store,
                (RESULT_PTR_OFFSET as i32, RESULT_LEN_OFFSET as i32),
            )
            .map_err(|e| crate::metadata::MetadataError::CallFailed(e.to_string()))?;

        if status != 0 {
            return Err(crate::metadata::MetadataError::CallFailed(
                "non-zero status from __pack_types".into(),
            ));
        }

        let memory = self
            .get_memory()
            .map_err(|e| crate::metadata::MetadataError::CallFailed(e.to_string()))?;
        let mut ptr_bytes = [0u8; 4];
        let mut len_bytes = [0u8; 4];
        memory
            .read(&self.store, RESULT_PTR_OFFSET, &mut ptr_bytes)
            .map_err(|e| crate::metadata::MetadataError::CallFailed(e.to_string()))?;
        memory
            .read(&self.store, RESULT_LEN_OFFSET, &mut len_bytes)
            .map_err(|e| crate::metadata::MetadataError::CallFailed(e.to_string()))?;

        let out_ptr = i32::from_le_bytes(ptr_bytes) as usize;
        let out_len = i32::from_le_bytes(len_bytes) as usize;

        // Read metadata bytes (static data, no __pack_free needed)
        let mut metadata_bytes = vec![0u8; out_len];
        memory
            .read(&self.store, out_ptr, &mut metadata_bytes)
            .map_err(|e| crate::metadata::MetadataError::CallFailed(e.to_string()))?;

        crate::metadata::decode_metadata_with_hashes(&metadata_bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::abi::Value;
    use crate::parser::parse_interface;
    use crate::types::Type;

    #[test]
    fn decode_arg_roundtrip() {
        let src = r#"
            interface api {
                variant node { leaf(s64), list(list<node>) }
            }
        "#;
        let interface = parse_interface(src).expect("parse");
        let runtime = Runtime::new();

        let value = Value::Variant {
            type_name: "node".to_string(),
            case_name: "leaf".to_string(),
            tag: 0,
            payload: vec![Value::S64(7)],
        };

        let bytes = encode(&value).expect("encode");
        let decoded = runtime
            .decode_arg(&interface.types, &bytes, &Type::named("node".to_string()))
            .expect("decode");

        assert_eq!(decoded, value);
    }

    #[test]
    fn decode_arg_rejects_mismatch() {
        let src = r#"
            interface api {
                variant node { leaf(s64), list(list<node>) }
            }
        "#;
        let interface = parse_interface(src).expect("parse");
        let runtime = Runtime::new();

        let value = Value::String("bad".to_string());
        let bytes = encode(&value).expect("encode");

        let err = runtime
            .decode_arg(&interface.types, &bytes, &Type::named("node".to_string()))
            .expect_err("expected error");

        match err {
            RuntimeError::SchemaError(_) => {}
            _ => panic!("unexpected error: {err:?}"),
        }
    }

    #[test]
    fn encode_result_rejects_mismatch() {
        let src = r#"
            interface api {
                record config { name: string }
            }
        "#;
        let interface = parse_interface(src).expect("parse");
        let runtime = Runtime::new();

        let value = Value::Record {
            type_name: "config".to_string(),
            fields: vec![("wrong".to_string(), Value::String("x".to_string()))],
        };
        let err = runtime
            .encode_result_with_schema(&interface.types, &value, &Type::named("config".to_string()))
            .expect_err("expected error");

        match err {
            RuntimeError::SchemaError(_) => {}
            _ => panic!("unexpected error: {err:?}"),
        }
    }

    /// Exercises the exact cache pattern theater depends on:
    ///   load_module → module().clone() → wrap_module → instantiate.
    /// Confirms the wrapped instance is usable end-to-end.
    #[tokio::test]
    async fn wrap_module_roundtrip_through_cache_pattern() {
        let wasm = wat::parse_str(
            r#"
            (module
                (func (export "answer") (result i32)
                    i32.const 42))
            "#,
        )
        .expect("wat");

        let runtime = AsyncRuntime::new();

        let compiled = runtime.load_module(&wasm).expect("load_module");
        let cached_module: Module = compiled.module().clone();

        let wrapped = runtime.wrap_module(cached_module);
        let mut instance = wrapped
            .instantiate_async()
            .await
            .expect("instantiate_async from wrapped module");

        // Drive a typed call so we know the engine genuinely owns this
        // instance (instantiation alone could pass via metadata stored
        // on the Module without ever touching the engine's allocator).
        let func = instance
            .instance
            .get_typed_func::<(), i32>(&mut instance.store, "answer")
            .expect("typed func");
        let answer = func
            .call_async(&mut instance.store, ())
            .await
            .expect("call");
        assert_eq!(answer, 42);
    }

    /// The runaway-guest kill switch: a guest stuck in an infinite loop must
    /// TRAP once its epoch deadline passes, so the call returns `Err` instead of
    /// pegging a core forever (the mail-spine decode-loop failure class). The
    /// epoch is advanced from a separate OS thread (the guest fiber occupies the
    /// executor while it spins), and the whole test is bounded by a timeout so a
    /// BROKEN guardrail fails loudly instead of hanging.
    #[tokio::test]
    async fn runaway_guest_traps_on_epoch_deadline() {
        let wasm = wat::parse_str(
            r#"
            (module
                (func (export "spin") (loop br 0)))
            "#,
        )
        .expect("wat");

        let runtime = AsyncRuntime::new();
        let compiled = runtime.load_module(&wasm).expect("load_module");
        let mut instance = compiled
            .instantiate_async()
            .await
            .expect("instantiate_async");

        // Arm the kill switch: trap one epoch tick from now.
        instance.set_epoch_deadline(1);

        // Advance the engine epoch from another OS thread while the guest spins.
        let engine = runtime.engine().clone();
        std::thread::spawn(move || {
            for _ in 0..20 {
                std::thread::sleep(std::time::Duration::from_millis(20));
                engine.increment_epoch();
            }
        });

        let func = instance
            .instance
            .get_typed_func::<(), ()>(&mut instance.store, "spin")
            .expect("typed func");

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            func.call_async(&mut instance.store, ()),
        )
        .await
        .expect("guardrail FAILED: runaway guest was not interrupted within 5s");

        // The guest trapped on the epoch deadline rather than looping forever.
        assert!(result.is_err(), "expected an epoch-deadline trap, got Ok");
    }

    /// Regression: the host advances the engine epoch on a ticker, so by the time
    /// an actor is instantiated the epoch is already > 0. A "no deadline" default
    /// of `u64::MAX` then overflows (`current_epoch() + u64::MAX`) — panicking in
    /// debug, wrapping to a garbage tiny deadline (immediate trap) in release — on
    /// EVERY spawn. Instantiating after an advanced epoch, then arming a real
    /// deadline, must not overflow.
    #[tokio::test]
    async fn epoch_deadline_survives_advanced_epoch() {
        let runtime = AsyncRuntime::new();
        // Simulate the host's 1/sec ticker having run before any actor spawns.
        let engine = runtime.engine().clone();
        for _ in 0..10 {
            engine.increment_epoch();
        }

        let wasm = wat::parse_str(r#"(module (func (export "noop")))"#).expect("wat");
        let compiled = runtime.load_module(&wasm).expect("load");
        // instantiate arms the default deadline internally — must not overflow.
        let mut instance = compiled.instantiate_async().await.expect("instantiate");
        // theater arms a real per-call deadline before each call — also must not.
        instance.set_epoch_deadline(60);

        let func = instance
            .instance
            .get_typed_func::<(), ()>(&mut instance.store, "noop")
            .expect("func");
        func.call_async(&mut instance.store, ())
            .await
            .expect("call");
    }

    /// A `Module` compiled by Engine A must not be wrappable into a
    /// runtime on Engine B — wasmtime would panic deep inside
    /// `Linker::instantiate` on the cache hit; we want the panic at the
    /// API boundary with a message that names the bug.
    #[test]
    #[should_panic(expected = "different Engine")]
    fn wrap_module_panics_on_cross_engine() {
        let wasm = wat::parse_str("(module)").expect("wat");

        let runtime_a = AsyncRuntime::new();
        let runtime_b = AsyncRuntime::new();

        let compiled_on_a = runtime_a.load_module(&wasm).expect("load_module");
        let module_from_a: Module = compiled_on_a.module().clone();

        // This is the misuse the assert! is there to catch.
        let _ = runtime_b.wrap_module(module_from_a);
    }
}
