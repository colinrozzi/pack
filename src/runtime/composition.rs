//! Package Composition
//!
//! Enables composing multiple packages together, wiring one package's
//! exports to another package's imports.
//!
//! # Example
//!
//! ```ignore
//! use pack::runtime::CompositionBuilder;
//! use pack::abi::Value;
//!
//! // Load package bytes
//! let doubler_wasm = std::fs::read("doubler.wasm")?;
//! let adder_wasm = std::fs::read("adder.wasm")?;
//!
//! // Build composition
//! let mut composition = CompositionBuilder::new()
//!     .add_package("doubler", doubler_wasm)
//!     .add_package("adder", adder_wasm)
//!     .wire("adder", "math", "double", "doubler", "transform")
//!     .build()?;
//!
//! // Call the adder, which internally calls doubler
//! let result = composition.call("adder", "process", &Value::S64(5))?;
//! assert_eq!(result, Value::S64(11)); // (5 * 2) + 1
//! ```

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::abi::{decode, encode, Value};
use crate::runtime::{
    RuntimeError, INPUT_BUFFER_OFFSET, RESULT_PTR_OFFSET, RESULT_LEN_OFFSET,
};
use wasmtime::{Caller, Engine, Linker, Module, Store};

/// A host function that can be wired into compositions.
///
/// Host functions follow the guest-allocates ABI, taking encoded input bytes
/// and returning encoded output bytes.
pub type HostFn = Arc<dyn Fn(&[u8]) -> Result<Vec<u8>, String> + Send + Sync>;

/// Builder for creating composed packages with cross-package imports.
pub struct CompositionBuilder {
    engine: Engine,
    packages: Vec<PackageDefinition>,
    host_functions: Vec<HostFunctionDef>,
}

struct PackageDefinition {
    name: String,
    wasm_bytes: Vec<u8>,
    imports: Vec<ImportWiring>,
}

struct HostFunctionDef {
    module: String,
    function: String,
    handler: HostFn,
}

#[derive(Clone)]
struct ImportWiring {
    import_module: String,
    import_function: String,
    source_package: String,
    source_function: String,
}

impl CompositionBuilder {
    /// Create a new composition builder.
    pub fn new() -> Self {
        // Enable multi-memory for composed modules that may come from pack-compose
        let mut config = wasmtime::Config::new();
        config.wasm_multi_memory(true);
        let engine = Engine::new(&config).expect("failed to create engine");
        Self {
            engine,
            packages: Vec::new(),
            host_functions: Vec::new(),
        }
    }

    /// Add a package to the composition.
    pub fn add_package(mut self, name: impl Into<String>, wasm_bytes: Vec<u8>) -> Self {
        self.packages.push(PackageDefinition {
            name: name.into(),
            wasm_bytes,
            imports: Vec::new(),
        });
        self
    }

    /// Add a host function that can satisfy imports.
    ///
    /// The handler receives encoded input bytes and returns encoded output bytes.
    /// Use `pack::abi::decode` and `pack::abi::encode` to work with Values.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use pack::abi::{decode, encode, Value};
    ///
    /// builder.add_host_function("my:interface", "my-func", |input_bytes| {
    ///     let input: Value = decode(input_bytes)?;
    ///     // Process input...
    ///     let output = Value::S64(42);
    ///     encode(&output).map_err(|e| e.to_string())
    /// })
    /// ```
    pub fn add_host_function<F>(
        mut self,
        module: impl Into<String>,
        function: impl Into<String>,
        handler: F,
    ) -> Self
    where
        F: Fn(&[u8]) -> Result<Vec<u8>, String> + Send + Sync + 'static,
    {
        self.host_functions.push(HostFunctionDef {
            module: module.into(),
            function: function.into(),
            handler: Arc::new(handler),
        });
        self
    }

    /// Add a typed host function that works with Values directly.
    ///
    /// This is a convenience wrapper around `add_host_function` that handles
    /// encoding/decoding automatically.
    ///
    /// # Example
    ///
    /// ```ignore
    /// builder.add_host_function_typed("my:interface", "double", |input: Value| {
    ///     match input {
    ///         Value::S64(n) => Ok(Value::S64(n * 2)),
    ///         _ => Err("expected s64".to_string()),
    ///     }
    /// })
    /// ```
    pub fn add_host_function_typed<F>(
        self,
        module: impl Into<String>,
        function: impl Into<String>,
        handler: F,
    ) -> Self
    where
        F: Fn(Value) -> Result<Value, String> + Send + Sync + 'static,
    {
        let module = module.into();
        let function = function.into();
        self.add_host_function(module, function, move |input_bytes| {
            let input = decode(input_bytes).map_err(|e| e.to_string())?;
            let output = handler(input)?;
            encode(&output).map_err(|e| e.to_string())
        })
    }

    /// Wire an import from one package to an export from another.
    ///
    /// When `target_package` calls `import_module::import_function`,
    /// it will actually call `source_package`'s `source_function`.
    pub fn wire(
        mut self,
        target_package: impl Into<String>,
        import_module: impl Into<String>,
        import_function: impl Into<String>,
        source_package: impl Into<String>,
        source_function: impl Into<String>,
    ) -> Self {
        let target = target_package.into();
        let wiring = ImportWiring {
            import_module: import_module.into(),
            import_function: import_function.into(),
            source_package: source_package.into(),
            source_function: source_function.into(),
        };

        for pkg in &mut self.packages {
            if pkg.name == target {
                pkg.imports.push(wiring);
                return self;
            }
        }

        self
    }

    /// Build the composition.
    pub fn build(self) -> Result<BuiltComposition, RuntimeError> {
        // Compile all modules first
        let mut compiled: HashMap<String, Module> = HashMap::new();
        for pkg in &self.packages {
            if !pkg.wasm_bytes.is_empty() {
                let module = Module::new(&self.engine, &pkg.wasm_bytes)
                    .map_err(|e| RuntimeError::WasmError(e.to_string()))?;
                compiled.insert(pkg.name.clone(), module);
            }
        }

        // Shared registry for cross-package calls
        let registry: Arc<Mutex<PackageRegistry>> = Arc::new(Mutex::new(PackageRegistry {
            packages: HashMap::new(),
        }));

        // Topological sort: instantiate packages without imports first
        // Note: if there are host functions defined, ALL packages need them,
        // so treat them all as consumers in that case
        let has_host_functions = !self.host_functions.is_empty();

        let providers: Vec<_> = self
            .packages
            .iter()
            .filter(|p| p.imports.is_empty() && !has_host_functions)
            .collect();

        let consumers: Vec<_> = self
            .packages
            .iter()
            .filter(|p| !p.imports.is_empty() || has_host_functions)
            .collect();

        // Instantiate providers first
        for pkg in providers {
            let module = compiled.get(&pkg.name).ok_or_else(|| {
                RuntimeError::ModuleNotFound(format!("Package '{}' not found", pkg.name))
            })?;

            let linker = Linker::<()>::new(&self.engine);
            let mut store = Store::new(&self.engine, ());

            let instance = linker
                .instantiate(&mut store, module)
                .map_err(|e| RuntimeError::WasmError(e.to_string()))?;

            registry.lock().unwrap().packages.insert(
                pkg.name.clone(),
                PackageEntry {
                    store: Arc::new(Mutex::new(UntypedStore::Unit(store))),
                    instance,
                },
            );
        }

        // Now instantiate consumers with wired imports
        for pkg in consumers {
            let module = compiled.get(&pkg.name).ok_or_else(|| {
                RuntimeError::ModuleNotFound(format!("Package '{}' not found", pkg.name))
            })?;

            let mut linker = Linker::<ComposedState>::new(&self.engine);

            // Wire host functions first
            for host_fn in &self.host_functions {
                let handler = Arc::clone(&host_fn.handler);

                linker
                    .func_wrap(
                        &host_fn.module,
                        &host_fn.function,
                        move |mut caller: Caller<'_, ComposedState>,
                              in_ptr: i32,
                              in_len: i32,
                              out_ptr_ptr: i32,
                              out_len_ptr: i32|
                              -> i32 {
                            host_function_call(
                                &mut caller,
                                &handler,
                                in_ptr,
                                in_len,
                                out_ptr_ptr,
                                out_len_ptr,
                            )
                        },
                    )
                    .map_err(|e| RuntimeError::WasmError(e.to_string()))?;
            }

            // Wire each cross-package import
            for wiring in &pkg.imports {
                let source_pkg = wiring.source_package.clone();
                let source_fn = wiring.source_function.clone();
                let reg = Arc::clone(&registry);

                linker
                    .func_wrap(
                        &wiring.import_module,
                        &wiring.import_function,
                        move |mut caller: Caller<'_, ComposedState>,
                              in_ptr: i32,
                              in_len: i32,
                              out_ptr: i32,
                              out_cap: i32|
                              -> i32 {
                            cross_package_call(
                                &mut caller,
                                &reg,
                                &source_pkg,
                                &source_fn,
                                in_ptr,
                                in_len,
                                out_ptr,
                                out_cap,
                            )
                        },
                    )
                    .map_err(|e| RuntimeError::WasmError(e.to_string()))?;
            }

            let state = ComposedState {
                _registry: Arc::clone(&registry),
            };
            let mut store = Store::new(&self.engine, state);

            let instance = linker
                .instantiate(&mut store, module)
                .map_err(|e| RuntimeError::WasmError(e.to_string()))?;

            // Add to registry (consumers can also be called)
            registry.lock().unwrap().packages.insert(
                pkg.name.clone(),
                PackageEntry {
                    store: Arc::new(Mutex::new(UntypedStore::Composed(store))),
                    instance,
                },
            );
        }

        Ok(BuiltComposition {
            _engine: self.engine,
            registry,
        })
    }
}

/// Handle a cross-package call using the guest-allocates ABI.
///
/// # ABI
///
/// The source function has signature:
/// ```text
/// fn(in_ptr: i32, in_len: i32, out_ptr_ptr: i32, out_len_ptr: i32) -> i32
/// ```
///
/// Returns 0 on success (output ptr/len written to slots), -1 on error.
fn cross_package_call(
    caller: &mut Caller<'_, ComposedState>,
    registry: &Arc<Mutex<PackageRegistry>>,
    source_pkg: &str,
    source_fn: &str,
    in_ptr: i32,
    in_len: i32,
    out_ptr_ptr: i32,
    out_len_ptr: i32,
) -> i32 {
    // Read input from caller's memory
    let memory = match caller.get_export("memory") {
        Some(wasmtime::Extern::Memory(m)) => m,
        _ => return -1,
    };

    let mut input_bytes = vec![0u8; in_len as usize];
    if memory.read(&caller, in_ptr as usize, &mut input_bytes).is_err() {
        return -1;
    }

    // Look up the source package and get an Arc to its store
    // Important: release the registry lock before calling into WASM
    let (store_arc, instance) = {
        let reg = registry.lock().unwrap();
        let source = match reg.packages.get(source_pkg) {
            Some(p) => p,
            None => return -1,
        };
        (Arc::clone(&source.store), source.instance.clone())
    };
    // Registry lock released here

    // Call the source package - try dynamic allocation, fall back to fixed buffer
    let result = {
        let mut store_guard = store_arc.lock().unwrap();

        // Get memory
        let src_memory = match store_guard.get_memory(&instance) {
            Some(m) => m,
            None => return -1,
        };

        // Try to allocate input buffer dynamically, fall back to fixed buffer
        let (in_ptr, dynamic_input) = match store_guard.get_alloc_func(&instance) {
            Some(alloc_func) => {
                match store_guard.call_alloc(&alloc_func, input_bytes.len() as i32) {
                    Ok(ptr) if ptr != 0 => (ptr, true),
                    _ => (INPUT_BUFFER_OFFSET as i32, false),
                }
            }
            None => (INPUT_BUFFER_OFFSET as i32, false),
        };

        // Write input to buffer
        if store_guard
            .write_memory(&src_memory, in_ptr as usize, &input_bytes)
            .is_err()
        {
            return -1;
        }

        // Call the source function
        let func = match store_guard.get_typed_func(&instance, source_fn) {
            Some(f) => f,
            None => return -1,
        };

        let status = match store_guard.call_func(
            &func,
            in_ptr,
            input_bytes.len() as i32,
            RESULT_PTR_OFFSET as i32,
            RESULT_LEN_OFFSET as i32,
        ) {
            Ok(s) => s,
            Err(_) => return -1,
        };

        // Free the input buffer if dynamically allocated
        if dynamic_input {
            if let Some(free_func) = store_guard.get_free_func(&instance) {
                let _ = store_guard.call_free(&free_func, in_ptr, input_bytes.len() as i32);
            }
        }

        if status != 0 {
            // Error occurred - read error message from slots and propagate
            // For now, just return -1
            return -1;
        }

        // Read output pointer and length from the slots
        let mut ptr_bytes = [0u8; 4];
        let mut len_bytes = [0u8; 4];
        if store_guard.read_memory(&src_memory, RESULT_PTR_OFFSET, &mut ptr_bytes).is_err() {
            return -1;
        }
        if store_guard.read_memory(&src_memory, RESULT_LEN_OFFSET, &mut len_bytes).is_err() {
            return -1;
        }

        let out_ptr = i32::from_le_bytes(ptr_bytes) as usize;
        let out_len = i32::from_le_bytes(len_bytes) as usize;

        // Read output from the guest-allocated buffer
        let mut output_bytes = vec![0u8; out_len];
        if store_guard
            .read_memory(&src_memory, out_ptr, &mut output_bytes)
            .is_err()
        {
            return -1;
        }

        // Free the guest's output buffer
        if let Some(free_func) = store_guard.get_free_func(&instance) {
            let _ = store_guard.call_free(&free_func, out_ptr as i32, out_len as i32);
        }

        output_bytes
    };

    // The caller also uses guest-allocates ABI, so we need to allocate in caller's memory
    // For cross-package calls, the caller is also a guest, so we write to caller's result slots
    // Actually, the caller provided out_ptr_ptr and out_len_ptr, so we need to allocate
    // in the caller's memory and write the ptr/len there.

    // For simplicity in cross-package calls, we'll allocate in caller's heap
    // This requires the caller to have __pack_alloc exported, which may not exist.
    //
    // Alternative: Use a fixed buffer region in caller for cross-package results.
    // Let's use the same RESULT region as a data buffer for now (after the ptr/len slots).
    const CROSS_CALL_BUFFER_OFFSET: usize = RESULT_LEN_OFFSET + 4;

    if memory.write(&mut *caller, CROSS_CALL_BUFFER_OFFSET, &result).is_err() {
        return -1;
    }

    // Write the buffer location to caller's result slots
    let result_ptr = CROSS_CALL_BUFFER_OFFSET as i32;
    let result_len = result.len() as i32;

    if memory.write(&mut *caller, out_ptr_ptr as usize, &result_ptr.to_le_bytes()).is_err() {
        return -1;
    }
    if memory.write(&mut *caller, out_len_ptr as usize, &result_len.to_le_bytes()).is_err() {
        return -1;
    }

    0 // Success
}

/// Handle a host function call using the guest-allocates ABI.
///
/// Similar to cross_package_call but invokes a host-provided function
/// instead of another WASM package.
fn host_function_call(
    caller: &mut Caller<'_, ComposedState>,
    handler: &HostFn,
    in_ptr: i32,
    in_len: i32,
    out_ptr_ptr: i32,
    out_len_ptr: i32,
) -> i32 {
    // Read input from caller's memory
    let memory = match caller.get_export("memory") {
        Some(wasmtime::Extern::Memory(m)) => m,
        _ => return -1,
    };

    let mut input_bytes = vec![0u8; in_len as usize];
    if memory.read(&caller, in_ptr as usize, &mut input_bytes).is_err() {
        return -1;
    }

    // Call the host function
    let result = match handler(&input_bytes) {
        Ok(bytes) => bytes,
        Err(_) => return -1,
    };

    // Write result to caller's memory using a fixed buffer region
    // (same approach as cross_package_call)
    const HOST_CALL_BUFFER_OFFSET: usize = RESULT_LEN_OFFSET + 4;

    if memory.write(&mut *caller, HOST_CALL_BUFFER_OFFSET, &result).is_err() {
        return -1;
    }

    // Write the buffer location to caller's result slots
    let result_ptr = HOST_CALL_BUFFER_OFFSET as i32;
    let result_len = result.len() as i32;

    if memory.write(&mut *caller, out_ptr_ptr as usize, &result_ptr.to_le_bytes()).is_err() {
        return -1;
    }
    if memory.write(&mut *caller, out_len_ptr as usize, &result_len.to_le_bytes()).is_err() {
        return -1;
    }

    0 // Success
}

impl Default for CompositionBuilder {
    fn default() -> Self {
        Self::new()
    }
}

struct ComposedState {
    _registry: Arc<Mutex<PackageRegistry>>,
}

struct PackageRegistry {
    packages: HashMap<String, PackageEntry>,
}

struct PackageEntry {
    store: Arc<Mutex<UntypedStore>>,
    instance: wasmtime::Instance,
}

/// Wrapper to handle stores with different state types
enum UntypedStore {
    Unit(Store<()>),
    Composed(Store<ComposedState>),
}

impl UntypedStore {
    fn get_memory(&mut self, instance: &wasmtime::Instance) -> Option<wasmtime::Memory> {
        match self {
            UntypedStore::Unit(store) => instance.get_memory(&mut *store, "memory"),
            UntypedStore::Composed(store) => instance.get_memory(&mut *store, "memory"),
        }
    }

    fn write_memory(
        &mut self,
        memory: &wasmtime::Memory,
        offset: usize,
        data: &[u8],
    ) -> Result<(), ()> {
        match self {
            UntypedStore::Unit(store) => memory.write(&mut *store, offset, data).map_err(|_| ()),
            UntypedStore::Composed(store) => {
                memory.write(&mut *store, offset, data).map_err(|_| ())
            }
        }
    }

    fn read_memory(
        &mut self,
        memory: &wasmtime::Memory,
        offset: usize,
        data: &mut [u8],
    ) -> Result<(), ()> {
        match self {
            UntypedStore::Unit(store) => memory.read(&*store, offset, data).map_err(|_| ()),
            UntypedStore::Composed(store) => memory.read(&*store, offset, data).map_err(|_| ()),
        }
    }

    fn get_typed_func(
        &mut self,
        instance: &wasmtime::Instance,
        name: &str,
    ) -> Option<wasmtime::TypedFunc<(i32, i32, i32, i32), i32>> {
        match self {
            UntypedStore::Unit(store) => instance.get_typed_func(&mut *store, name).ok(),
            UntypedStore::Composed(store) => instance.get_typed_func(&mut *store, name).ok(),
        }
    }

    fn get_free_func(
        &mut self,
        instance: &wasmtime::Instance,
    ) -> Option<wasmtime::TypedFunc<(i32, i32), ()>> {
        match self {
            UntypedStore::Unit(store) => instance.get_typed_func(&mut *store, "__pack_free").ok(),
            UntypedStore::Composed(store) => instance.get_typed_func(&mut *store, "__pack_free").ok(),
        }
    }

    fn get_alloc_func(
        &mut self,
        instance: &wasmtime::Instance,
    ) -> Option<wasmtime::TypedFunc<i32, i32>> {
        match self {
            UntypedStore::Unit(store) => instance.get_typed_func(&mut *store, "__pack_alloc").ok(),
            UntypedStore::Composed(store) => instance.get_typed_func(&mut *store, "__pack_alloc").ok(),
        }
    }

    fn call_alloc(
        &mut self,
        func: &wasmtime::TypedFunc<i32, i32>,
        size: i32,
    ) -> Result<i32, ()> {
        match self {
            UntypedStore::Unit(store) => func.call(&mut *store, size).map_err(|_| ()),
            UntypedStore::Composed(store) => func.call(&mut *store, size).map_err(|_| ()),
        }
    }

    fn call_func(
        &mut self,
        func: &wasmtime::TypedFunc<(i32, i32, i32, i32), i32>,
        a: i32,
        b: i32,
        c: i32,
        d: i32,
    ) -> Result<i32, ()> {
        match self {
            UntypedStore::Unit(store) => func.call(&mut *store, (a, b, c, d)).map_err(|e| {
                eprintln!("[PACK DEBUG] call_func error: {:?}", e);
                ()
            }),
            UntypedStore::Composed(store) => func.call(&mut *store, (a, b, c, d)).map_err(|e| {
                eprintln!("[PACK DEBUG] call_func error: {:?}", e);
                ()
            }),
        }
    }

    fn call_free(
        &mut self,
        func: &wasmtime::TypedFunc<(i32, i32), ()>,
        ptr: i32,
        len: i32,
    ) -> Result<(), ()> {
        match self {
            UntypedStore::Unit(store) => func.call(&mut *store, (ptr, len)).map_err(|_| ()),
            UntypedStore::Composed(store) => func.call(&mut *store, (ptr, len)).map_err(|_| ()),
        }
    }

    fn get_types_func(
        &mut self,
        instance: &wasmtime::Instance,
    ) -> Option<wasmtime::TypedFunc<(i32, i32), i32>> {
        match self {
            UntypedStore::Unit(store) => {
                instance.get_typed_func(&mut *store, "__pack_types").ok()
            }
            UntypedStore::Composed(store) => {
                instance.get_typed_func(&mut *store, "__pack_types").ok()
            }
        }
    }

    fn call_types_func(
        &mut self,
        func: &wasmtime::TypedFunc<(i32, i32), i32>,
        a: i32,
        b: i32,
    ) -> Result<i32, ()> {
        match self {
            UntypedStore::Unit(store) => func.call(&mut *store, (a, b)).map_err(|_| ()),
            UntypedStore::Composed(store) => func.call(&mut *store, (a, b)).map_err(|_| ()),
        }
    }
}

/// A built composition ready for execution.
pub struct BuiltComposition {
    _engine: Engine,
    registry: Arc<Mutex<PackageRegistry>>,
}

impl BuiltComposition {
    /// Call a function on a package in the composition.
    pub fn call(
        &mut self,
        package: &str,
        function: &str,
        input: &Value,
    ) -> Result<Value, RuntimeError> {
        // Look up package and clone Arc references before releasing lock
        // This is important to avoid deadlock when WASM calls back into registry
        let (store_arc, instance) = {
            let reg = self.registry.lock().unwrap();
            let pkg = reg.packages.get(package).ok_or_else(|| {
                RuntimeError::ModuleNotFound(format!("Package '{}' not found", package))
            })?;
            (Arc::clone(&pkg.store), pkg.instance.clone())
        };
        // Registry lock released here

        let mut store = store_arc.lock().unwrap();

        // Encode input
        let input_bytes = encode(input).map_err(|e| RuntimeError::AbiError(e.to_string()))?;

        // Get memory
        let memory = store.get_memory(&instance).ok_or_else(|| {
            RuntimeError::MemoryError("No memory export".into())
        })?;

        // Try to allocate input buffer dynamically, fall back to fixed buffer
        let (in_ptr, dynamic_input) = match store.get_alloc_func(&instance) {
            Some(alloc_func) => {
                match store.call_alloc(&alloc_func, input_bytes.len() as i32) {
                    Ok(ptr) if ptr != 0 => (ptr, true),
                    _ => (INPUT_BUFFER_OFFSET as i32, false),
                }
            }
            None => (INPUT_BUFFER_OFFSET as i32, false),
        };

        // Write input to buffer
        store
            .write_memory(&memory, in_ptr as usize, &input_bytes)
            .map_err(|_| RuntimeError::MemoryError("Failed to write input".into()))?;

        // Get and call the function
        let func = store.get_typed_func(&instance, function).ok_or_else(|| {
            RuntimeError::FunctionNotFound(function.to_string())
        })?;

        let status = store
            .call_func(&func,
                in_ptr,
                input_bytes.len() as i32,
                RESULT_PTR_OFFSET as i32,
                RESULT_LEN_OFFSET as i32,
            )
            .map_err(|e| RuntimeError::WasmError(format!("Function call failed: {:?}", e)))?;

        // Free the input buffer if dynamically allocated
        if dynamic_input {
            if let Some(free_func) = store.get_free_func(&instance) {
                let _ = store.call_free(&free_func, in_ptr, input_bytes.len() as i32);
            }
        }

        if status != 0 {
            // Error - read error message from result slots
            let mut ptr_bytes = [0u8; 4];
            let mut len_bytes = [0u8; 4];
            let _ = store.read_memory(&memory, RESULT_PTR_OFFSET, &mut ptr_bytes);
            let _ = store.read_memory(&memory, RESULT_LEN_OFFSET, &mut len_bytes);
            let err_ptr = i32::from_le_bytes(ptr_bytes) as usize;
            let err_len = i32::from_le_bytes(len_bytes) as usize;

            let mut err_bytes = vec![0u8; err_len];
            if store.read_memory(&memory, err_ptr, &mut err_bytes).is_ok() {
                if let Ok(err_msg) = String::from_utf8(err_bytes) {
                    // Free the error buffer
                    if let Some(free_func) = store.get_free_func(&instance) {
                        let _ = store.call_free(&free_func, err_ptr as i32, err_len as i32);
                    }
                    return Err(RuntimeError::WasmError(err_msg));
                }
            }
            return Err(RuntimeError::WasmError("Function returned error".into()));
        }

        // Read output pointer and length from result slots
        let mut ptr_bytes = [0u8; 4];
        let mut len_bytes = [0u8; 4];
        store
            .read_memory(&memory, RESULT_PTR_OFFSET, &mut ptr_bytes)
            .map_err(|_| RuntimeError::MemoryError("Failed to read result ptr".into()))?;
        store
            .read_memory(&memory, RESULT_LEN_OFFSET, &mut len_bytes)
            .map_err(|_| RuntimeError::MemoryError("Failed to read result len".into()))?;

        let out_ptr = i32::from_le_bytes(ptr_bytes) as usize;
        let out_len = i32::from_le_bytes(len_bytes) as usize;

        // Read output from guest-allocated buffer
        let mut output_bytes = vec![0u8; out_len];
        store
            .read_memory(&memory, out_ptr, &mut output_bytes)
            .map_err(|_| RuntimeError::MemoryError("Failed to read output".into()))?;

        // Free the guest's buffer
        if let Some(free_func) = store.get_free_func(&instance) {
            let _ = store.call_free(&free_func, out_ptr as i32, out_len as i32);
        }

        decode(&output_bytes).map_err(|e| RuntimeError::AbiError(e.to_string()))
    }

    /// List all packages in the composition.
    pub fn packages(&self) -> Vec<String> {
        self.registry.lock().unwrap().packages.keys().cloned().collect()
    }

    /// Read embedded type metadata from a package in the composition.
    ///
    /// Returns `Err(MetadataError::NotFound)` if the package doesn't exist
    /// or doesn't export `__pack_types`.
    pub fn types(
        &mut self,
        package: &str,
    ) -> Result<crate::metadata::PackageMetadata, crate::metadata::MetadataError> {
        let (store_arc, instance) = {
            let reg = self.registry.lock().unwrap();
            let pkg = reg
                .packages
                .get(package)
                .ok_or(crate::metadata::MetadataError::NotFound)?;
            (Arc::clone(&pkg.store), pkg.instance.clone())
        };

        let mut store = store_arc.lock().unwrap();

        let types_func = store
            .get_types_func(&instance)
            .ok_or(crate::metadata::MetadataError::NotFound)?;

        let status = store
            .call_types_func(
                &types_func,
                RESULT_PTR_OFFSET as i32,
                RESULT_LEN_OFFSET as i32,
            )
            .map_err(|_| {
                crate::metadata::MetadataError::CallFailed("call failed".into())
            })?;

        if status != 0 {
            return Err(crate::metadata::MetadataError::CallFailed(
                "non-zero status from __pack_types".into(),
            ));
        }

        let memory = store.get_memory(&instance).ok_or_else(|| {
            crate::metadata::MetadataError::CallFailed("no memory".into())
        })?;
        let mut ptr_bytes = [0u8; 4];
        let mut len_bytes = [0u8; 4];
        store
            .read_memory(&memory, RESULT_PTR_OFFSET, &mut ptr_bytes)
            .map_err(|_| {
                crate::metadata::MetadataError::CallFailed("read ptr failed".into())
            })?;
        store
            .read_memory(&memory, RESULT_LEN_OFFSET, &mut len_bytes)
            .map_err(|_| {
                crate::metadata::MetadataError::CallFailed("read len failed".into())
            })?;

        let out_ptr = i32::from_le_bytes(ptr_bytes) as usize;
        let out_len = i32::from_le_bytes(len_bytes) as usize;

        let mut metadata_bytes = vec![0u8; out_len];
        store
            .read_memory(&memory, out_ptr, &mut metadata_bytes)
            .map_err(|_| {
                crate::metadata::MetadataError::CallFailed("read metadata failed".into())
            })?;

        crate::metadata::decode_metadata(&metadata_bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_composition_builder_api() {
        let _builder = CompositionBuilder::new()
            .add_package("doubler", vec![])
            .add_package("adder", vec![])
            .wire("adder", "math", "double", "doubler", "transform");
    }
}
