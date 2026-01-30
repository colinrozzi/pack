//! Package Runtime
//!
//! Handles package instantiation, linking, and execution.

mod composition;
mod host;
mod interface_check;

pub use composition::{BuiltComposition, CompositionBuilder};
pub use host::{
    AsyncCtx, Ctx, DefaultHostProvider, ErrorHandler, HostFunctionError, HostFunctionErrorKind,
    HostFunctionProvider, HostLinkerBuilder, InterfaceBuilder, LinkerError,
    INPUT_BUFFER_OFFSET, OUTPUT_BUFFER_CAPACITY, OUTPUT_BUFFER_OFFSET,
    RESULT_PTR_OFFSET, RESULT_LEN_OFFSET,
};
pub use interface_check::{
    validate_instance_implements_interface, ExpectedSignature, InterfaceError,
};

use crate::abi::{decode, encode, Value};
use crate::wit_plus::{decode_with_schema, encode_with_schema, Interface, Type, TypeDef};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use thiserror::Error;
use wasmtime::{Config, Engine, Instance as WasmtimeInstance, Linker, Memory, Module, Store};

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
        Self {
            engine: Engine::default(),
        }
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
pub struct AsyncRuntime {
    engine: Engine,
}

impl AsyncRuntime {
    /// Create a new async-enabled runtime.
    pub fn new() -> Self {
        let mut config = Config::new();
        config.async_support(true);
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

/// A compiled WASM module for async execution.
pub struct AsyncCompiledModule<'a> {
    module: Module,
    engine: &'a Engine,
}

impl<'a> AsyncCompiledModule<'a> {
    /// Instantiate the module with no imports (async).
    pub async fn instantiate_async(&self) -> Result<AsyncInstance<()>, RuntimeError> {
        let mut store = Store::new(self.engine, ());
        let linker = Linker::<()>::new(self.engine);

        let instance = linker
            .instantiate_async(&mut store, &self.module)
            .await
            .map_err(|e| RuntimeError::WasmError(e.to_string()))?;

        Ok(AsyncInstance { store, instance })
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
        let mut linker = Linker::new(self.engine);
        let mut builder = HostLinkerBuilder::new(self.engine, &mut linker);
        configure(&mut builder).map_err(|e| RuntimeError::WasmError(e.to_string()))?;

        let mut store = Store::new(self.engine, state);
        let instance = linker
            .instantiate_async(&mut store, &self.module)
            .await
            .map_err(|e| RuntimeError::WasmError(e.to_string()))?;

        Ok(AsyncInstance { store, instance })
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
}

impl<T: Send> AsyncInstance<T> {
    /// Validate that this instance implements the given interface.
    pub fn validate_interface(&mut self, interface: &Interface) -> Result<(), InterfaceError> {
        validate_instance_implements_interface(&mut self.store, &self.instance, interface)
    }

    /// Get the exported memory (assumes it's named "memory").
    fn get_memory(&mut self) -> Result<Memory, RuntimeError> {
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
        // Encode input
        let input_bytes = encode(input).map_err(|e| RuntimeError::AbiError(e.to_string()))?;

        // Try to allocate input buffer dynamically, fall back to fixed buffer
        let (in_ptr, dynamic_input) = match self.call_pack_alloc_async(input_bytes.len()).await {
            Ok(ptr) => (ptr, true),
            Err(_) => (INPUT_BUFFER_OFFSET, false),
        };

        // Write input to buffer
        let memory = self.get_memory()?;
        memory.write(&mut self.store, in_ptr, &input_bytes)
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
            self.call_pack_free_async(in_ptr, input_bytes.len()).await.ok();
        }

        // Read result ptr/len from slots
        let memory = self.get_memory()?;
        let mut ptr_bytes = [0u8; 4];
        let mut len_bytes = [0u8; 4];
        memory.read(&self.store, RESULT_PTR_OFFSET, &mut ptr_bytes)
            .map_err(|_| RuntimeError::MemoryError("Failed to read result ptr".into()))?;
        memory.read(&self.store, RESULT_LEN_OFFSET, &mut len_bytes)
            .map_err(|_| RuntimeError::MemoryError("Failed to read result len".into()))?;

        let out_ptr = i32::from_le_bytes(ptr_bytes) as usize;
        let out_len = i32::from_le_bytes(len_bytes) as usize;

        // Check for error
        if status != 0 {
            // Read error message
            let mut err_bytes = vec![0u8; out_len];
            memory.read(&self.store, out_ptr, &mut err_bytes)
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
        if let Ok(free_func) = self.instance.get_typed_func::<(i32, i32), ()>(&mut self.store, "__pack_free") {
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
}

/// Type alias for async host function return type.
pub type AsyncHostFnResult<R> = Pin<Box<dyn Future<Output = R> + Send + 'static>>;

/// A compiled WASM module, ready to be instantiated
pub struct CompiledModule<'a> {
    module: Module,
    engine: &'a Engine,
}

impl<'a> CompiledModule<'a> {
    /// Instantiate the module with no imports
    pub fn instantiate(&self) -> Result<Instance<()>, RuntimeError> {
        let mut store = Store::new(self.engine, ());
        let linker = Linker::<()>::new(self.engine);

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
    pub fn call_with_value(
        &mut self,
        name: &str,
        input: &Value,
    ) -> Result<Value, RuntimeError> {
        // Encode input
        let input_bytes = encode(input).map_err(|e| RuntimeError::AbiError(e.to_string()))?;

        // Try to allocate input buffer dynamically, fall back to fixed buffer
        let (in_ptr, dynamic_input) = match self.call_pack_alloc(input_bytes.len()) {
            Ok(ptr) => (ptr, true),
            Err(_) => (INPUT_BUFFER_OFFSET, false),
        };

        // Write input to buffer
        let memory = self.get_memory()?;
        memory.write(&mut self.store, in_ptr, &input_bytes)
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
        memory.read(&self.store, RESULT_PTR_OFFSET, &mut ptr_bytes)
            .map_err(|_| RuntimeError::MemoryError("Failed to read result ptr".into()))?;
        memory.read(&self.store, RESULT_LEN_OFFSET, &mut len_bytes)
            .map_err(|_| RuntimeError::MemoryError("Failed to read result len".into()))?;

        let out_ptr = i32::from_le_bytes(ptr_bytes) as usize;
        let out_len = i32::from_le_bytes(len_bytes) as usize;

        // Check for error
        if status != 0 {
            // Read error message
            let mut err_bytes = vec![0u8; out_len];
            memory.read(&self.store, out_ptr, &mut err_bytes)
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
        if let Ok(free_func) = self.instance.get_typed_func::<(i32, i32), ()>(&mut self.store, "__pack_free") {
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
    pub fn types(&mut self) -> Result<crate::metadata::PackageMetadata, crate::metadata::MetadataError> {
        let types_func = self
            .instance
            .get_typed_func::<(i32, i32), i32>(&mut self.store, "__pack_types")
            .map_err(|_| crate::metadata::MetadataError::NotFound)?;

        let status = types_func
            .call(&mut self.store, (RESULT_PTR_OFFSET as i32, RESULT_LEN_OFFSET as i32))
            .map_err(|e| crate::metadata::MetadataError::CallFailed(e.to_string()))?;

        if status != 0 {
            return Err(crate::metadata::MetadataError::CallFailed(
                "non-zero status from __pack_types".into(),
            ));
        }

        let memory = self.get_memory()
            .map_err(|e| crate::metadata::MetadataError::CallFailed(e.to_string()))?;
        let mut ptr_bytes = [0u8; 4];
        let mut len_bytes = [0u8; 4];
        memory.read(&self.store, RESULT_PTR_OFFSET, &mut ptr_bytes)
            .map_err(|e| crate::metadata::MetadataError::CallFailed(e.to_string()))?;
        memory.read(&self.store, RESULT_LEN_OFFSET, &mut len_bytes)
            .map_err(|e| crate::metadata::MetadataError::CallFailed(e.to_string()))?;

        let out_ptr = i32::from_le_bytes(ptr_bytes) as usize;
        let out_len = i32::from_le_bytes(len_bytes) as usize;

        // Read metadata bytes (static data, no __pack_free needed)
        let mut metadata_bytes = vec![0u8; out_len];
        memory.read(&self.store, out_ptr, &mut metadata_bytes)
            .map_err(|e| crate::metadata::MetadataError::CallFailed(e.to_string()))?;

        crate::metadata::decode_metadata(&metadata_bytes)
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
    pub fn call_with_value(
        &mut self,
        name: &str,
        input: &Value,
    ) -> Result<Value, RuntimeError> {
        // Encode input
        let input_bytes = encode(input).map_err(|e| RuntimeError::AbiError(e.to_string()))?;

        // Try to allocate input buffer dynamically, fall back to fixed buffer
        let (in_ptr, dynamic_input) = match self.call_pack_alloc(input_bytes.len()) {
            Ok(ptr) => (ptr, true),
            Err(_) => (INPUT_BUFFER_OFFSET, false),
        };

        // Write input to buffer
        let memory = self.get_memory()?;
        memory.write(&mut self.store, in_ptr, &input_bytes)
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
        memory.read(&self.store, RESULT_PTR_OFFSET, &mut ptr_bytes)
            .map_err(|_| RuntimeError::MemoryError("Failed to read result ptr".into()))?;
        memory.read(&self.store, RESULT_LEN_OFFSET, &mut len_bytes)
            .map_err(|_| RuntimeError::MemoryError("Failed to read result len".into()))?;

        let out_ptr = i32::from_le_bytes(ptr_bytes) as usize;
        let out_len = i32::from_le_bytes(len_bytes) as usize;

        // Check for error
        if status != 0 {
            // Read error message
            let mut err_bytes = vec![0u8; out_len];
            memory.read(&self.store, out_ptr, &mut err_bytes)
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
        if let Ok(free_func) = self.instance.get_typed_func::<(i32, i32), ()>(&mut self.store, "__pack_free") {
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
    pub fn types(&mut self) -> Result<crate::metadata::PackageMetadata, crate::metadata::MetadataError> {
        let types_func = self
            .instance
            .get_typed_func::<(i32, i32), i32>(&mut self.store, "__pack_types")
            .map_err(|_| crate::metadata::MetadataError::NotFound)?;

        let status = types_func
            .call(&mut self.store, (RESULT_PTR_OFFSET as i32, RESULT_LEN_OFFSET as i32))
            .map_err(|e| crate::metadata::MetadataError::CallFailed(e.to_string()))?;

        if status != 0 {
            return Err(crate::metadata::MetadataError::CallFailed(
                "non-zero status from __pack_types".into(),
            ));
        }

        let memory = self.get_memory()
            .map_err(|e| crate::metadata::MetadataError::CallFailed(e.to_string()))?;
        let mut ptr_bytes = [0u8; 4];
        let mut len_bytes = [0u8; 4];
        memory.read(&self.store, RESULT_PTR_OFFSET, &mut ptr_bytes)
            .map_err(|e| crate::metadata::MetadataError::CallFailed(e.to_string()))?;
        memory.read(&self.store, RESULT_LEN_OFFSET, &mut len_bytes)
            .map_err(|e| crate::metadata::MetadataError::CallFailed(e.to_string()))?;

        let out_ptr = i32::from_le_bytes(ptr_bytes) as usize;
        let out_len = i32::from_le_bytes(len_bytes) as usize;

        // Read metadata bytes (static data, no __pack_free needed)
        let mut metadata_bytes = vec![0u8; out_len];
        memory.read(&self.store, out_ptr, &mut metadata_bytes)
            .map_err(|e| crate::metadata::MetadataError::CallFailed(e.to_string()))?;

        crate::metadata::decode_metadata(&metadata_bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::abi::Value;
    use crate::wit_plus::{parse_interface, Type};

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
            .decode_arg(&interface.types, &bytes, &Type::Named("node".to_string()))
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
            .decode_arg(&interface.types, &bytes, &Type::Named("node".to_string()))
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
            .encode_result_with_schema(
                &interface.types,
                &value,
                &Type::Named("config".to_string()),
            )
            .expect_err("expected error");

        match err {
            RuntimeError::SchemaError(_) => {}
            _ => panic!("unexpected error: {err:?}"),
        }
    }
}
