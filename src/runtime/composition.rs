//! Package Composition
//!
//! Enables composing multiple packages together, wiring one package's
//! exports to another package's imports.
//!
//! # Example
//!
//! ```ignore
//! use composite::runtime::CompositionBuilder;
//! use composite::abi::Value;
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

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::abi::{decode, encode, Value};
use crate::runtime::{
    RuntimeError, INPUT_BUFFER_OFFSET, OUTPUT_BUFFER_CAPACITY, OUTPUT_BUFFER_OFFSET,
};
use wasmtime::{Caller, Engine, Linker, Module, Store};

/// Builder for creating composed packages with cross-package imports.
pub struct CompositionBuilder {
    engine: Engine,
    packages: Vec<PackageDefinition>,
}

struct PackageDefinition {
    name: String,
    wasm_bytes: Vec<u8>,
    imports: Vec<ImportWiring>,
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
        Self {
            engine: Engine::default(),
            packages: Vec::new(),
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
        let registry: Rc<RefCell<PackageRegistry>> = Rc::new(RefCell::new(PackageRegistry {
            packages: HashMap::new(),
        }));

        // Topological sort: instantiate packages without imports first
        let providers: Vec<_> = self
            .packages
            .iter()
            .filter(|p| p.imports.is_empty())
            .collect();

        let consumers: Vec<_> = self
            .packages
            .iter()
            .filter(|p| !p.imports.is_empty())
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

            registry.borrow_mut().packages.insert(
                pkg.name.clone(),
                PackageEntry {
                    store: Rc::new(RefCell::new(UntypedStore::Unit(store))),
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

            // Wire each import
            for wiring in &pkg.imports {
                let source_pkg = wiring.source_package.clone();
                let source_fn = wiring.source_function.clone();
                let reg = Rc::clone(&registry);

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
                _registry: Rc::clone(&registry),
            };
            let mut store = Store::new(&self.engine, state);

            let instance = linker
                .instantiate(&mut store, module)
                .map_err(|e| RuntimeError::WasmError(e.to_string()))?;

            // Add to registry (consumers can also be called)
            registry.borrow_mut().packages.insert(
                pkg.name.clone(),
                PackageEntry {
                    store: Rc::new(RefCell::new(UntypedStore::Composed(store))),
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

/// Handle a cross-package call
fn cross_package_call(
    caller: &mut Caller<'_, ComposedState>,
    registry: &Rc<RefCell<PackageRegistry>>,
    source_pkg: &str,
    source_fn: &str,
    in_ptr: i32,
    in_len: i32,
    out_ptr: i32,
    out_cap: i32,
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

    // Call the source package
    let result = {
        let reg = registry.borrow();
        let source = match reg.packages.get(source_pkg) {
            Some(p) => p,
            None => return -1,
        };

        let mut store_guard = source.store.borrow_mut();

        // Write input to source's memory
        let src_memory = match store_guard.get_memory(&source.instance) {
            Some(m) => m,
            None => return -1,
        };

        if store_guard
            .write_memory(&src_memory, INPUT_BUFFER_OFFSET, &input_bytes)
            .is_err()
        {
            return -1;
        }

        // Call the source function
        let func = match store_guard.get_typed_func(&source.instance, source_fn) {
            Some(f) => f,
            None => return -1,
        };

        let out_len = match store_guard.call_func(
            &func,
            INPUT_BUFFER_OFFSET as i32,
            input_bytes.len() as i32,
            OUTPUT_BUFFER_OFFSET as i32,
            OUTPUT_BUFFER_CAPACITY as i32,
        ) {
            Ok(len) => len,
            Err(_) => return -1,
        };

        if out_len < 0 {
            return -1;
        }

        // Read output from source
        let mut output_bytes = vec![0u8; out_len as usize];
        if store_guard
            .read_memory(&src_memory, OUTPUT_BUFFER_OFFSET, &mut output_bytes)
            .is_err()
        {
            return -1;
        }

        output_bytes
    };

    // Write result back to caller's memory
    if result.len() > out_cap as usize {
        return -1;
    }

    if memory.write(&mut *caller, out_ptr as usize, &result).is_err() {
        return -1;
    }

    result.len() as i32
}

impl Default for CompositionBuilder {
    fn default() -> Self {
        Self::new()
    }
}

struct ComposedState {
    _registry: Rc<RefCell<PackageRegistry>>,
}

struct PackageRegistry {
    packages: HashMap<String, PackageEntry>,
}

struct PackageEntry {
    store: Rc<RefCell<UntypedStore>>,
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

    fn call_func(
        &mut self,
        func: &wasmtime::TypedFunc<(i32, i32, i32, i32), i32>,
        a: i32,
        b: i32,
        c: i32,
        d: i32,
    ) -> Result<i32, ()> {
        match self {
            UntypedStore::Unit(store) => func.call(&mut *store, (a, b, c, d)).map_err(|_| ()),
            UntypedStore::Composed(store) => func.call(&mut *store, (a, b, c, d)).map_err(|_| ()),
        }
    }
}

/// A built composition ready for execution.
pub struct BuiltComposition {
    _engine: Engine,
    registry: Rc<RefCell<PackageRegistry>>,
}

impl BuiltComposition {
    /// Call a function on a package in the composition.
    pub fn call(
        &mut self,
        package: &str,
        function: &str,
        input: &Value,
    ) -> Result<Value, RuntimeError> {
        let reg = self.registry.borrow();
        let pkg = reg.packages.get(package).ok_or_else(|| {
            RuntimeError::ModuleNotFound(format!("Package '{}' not found", package))
        })?;

        let mut store = pkg.store.borrow_mut();

        // Encode input
        let input_bytes = encode(input).map_err(|e| RuntimeError::AbiError(e.to_string()))?;

        // Get memory and write input
        let memory = store.get_memory(&pkg.instance).ok_or_else(|| {
            RuntimeError::MemoryError("No memory export".into())
        })?;

        store
            .write_memory(&memory, INPUT_BUFFER_OFFSET, &input_bytes)
            .map_err(|_| RuntimeError::MemoryError("Failed to write input".into()))?;

        // Get and call the function
        let func = store.get_typed_func(&pkg.instance, function).ok_or_else(|| {
            RuntimeError::FunctionNotFound(function.to_string())
        })?;

        let out_len = store
            .call_func(&func,
                INPUT_BUFFER_OFFSET as i32,
                input_bytes.len() as i32,
                OUTPUT_BUFFER_OFFSET as i32,
                OUTPUT_BUFFER_CAPACITY as i32,
            )
            .map_err(|_| RuntimeError::WasmError("Function call failed".into()))?;

        if out_len < 0 {
            return Err(RuntimeError::WasmError("Function returned error".into()));
        }

        // Read output
        let mut output_bytes = vec![0u8; out_len as usize];
        store
            .read_memory(&memory, OUTPUT_BUFFER_OFFSET, &mut output_bytes)
            .map_err(|_| RuntimeError::MemoryError("Failed to read output".into()))?;

        decode(&output_bytes).map_err(|e| RuntimeError::AbiError(e.to_string()))
    }

    /// List all packages in the composition.
    pub fn packages(&self) -> Vec<String> {
        self.registry.borrow().packages.keys().cloned().collect()
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
