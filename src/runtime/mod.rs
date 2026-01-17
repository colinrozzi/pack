//! Component Runtime
//!
//! Handles component instantiation, linking, and execution.

use crate::abi::{decode, encode, Value};
use crate::wit_plus::{decode_with_schema, encode_with_schema, Type, TypeDef};
use std::io::Cursor;
use std::sync::{Arc, Mutex};
use thiserror::Error;
use wasmi::{Caller, Engine, Linker, Module, Store};

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
    /// Log messages collected from the component
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

/// The component runtime
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
        let module = Module::new(&self.engine, Cursor::new(wasm_bytes))
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
            .map_err(|e| RuntimeError::WasmError(e.to_string()))?
            .start(&mut store)
            .map_err(|e| RuntimeError::WasmError(e.to_string()))?;

        Ok(Instance { store, instance })
    }

    /// Instantiate the module with host imports
    pub fn instantiate_with_imports(
        &self,
        imports: HostImports,
    ) -> Result<InstanceWithHost, RuntimeError> {
        let state = imports.state.clone();
        let mut store = Store::new(self.engine, state.clone());
        let mut linker = Linker::<HostState>::new(self.engine);

        // Register host functions under the "host" module
        Self::register_host_functions(&mut linker)?;

        let instance = linker
            .instantiate(&mut store, &self.module)
            .map_err(|e| RuntimeError::WasmError(e.to_string()))?
            .start(&mut store)
            .map_err(|e| RuntimeError::WasmError(e.to_string()))?;

        Ok(InstanceWithHost {
            store,
            instance,
            state,
        })
    }

    fn register_host_functions(linker: &mut Linker<HostState>) -> Result<(), RuntimeError> {
        // host.log(ptr: i32, len: i32) - log a string message
        linker
            .func_wrap("host", "log", |caller: Caller<'_, HostState>, ptr: i32, len: i32| {
                let memory = caller.get_export("memory")
                    .and_then(|e| e.into_memory())
                    .expect("memory export");

                let ptr = ptr as usize;
                let len = len as usize;
                let mut buffer = vec![0u8; len];
                memory.read(&caller, ptr, &mut buffer).expect("read memory");

                if let Ok(msg) = String::from_utf8(buffer) {
                    caller.data().log_messages.lock().unwrap().push(msg);
                }
            })
            .map_err(|e| RuntimeError::WasmError(e.to_string()))?;

        // host.alloc(size: i32) -> i32 - allocate memory, returns pointer
        linker
            .func_wrap("host", "alloc", |caller: Caller<'_, HostState>, size: i32| -> i32 {
                let mut offset = caller.data().alloc_offset.lock().unwrap();
                let ptr = *offset;
                *offset += size as usize;
                // Align to 8 bytes
                *offset = (*offset + 7) & !7;
                ptr as i32
            })
            .map_err(|e| RuntimeError::WasmError(e.to_string()))?;

        Ok(())
    }
}

/// A running WASM instance
pub struct Instance<T> {
    store: Store<T>,
    instance: wasmi::Instance,
}

/// Instance with host imports - provides access to host state
pub struct InstanceWithHost {
    store: Store<HostState>,
    instance: wasmi::Instance,
    state: HostState,
}

impl InstanceWithHost {
    /// Get the host state (for reading logs, etc.)
    pub fn host_state(&self) -> &HostState {
        &self.state
    }

    /// Get all log messages from the component
    pub fn get_logs(&self) -> Vec<String> {
        self.state.get_logs()
    }

    /// Clear log messages
    pub fn clear_logs(&self) {
        self.state.clear_logs()
    }

    /// Get the exported memory (assumes it's named "memory")
    fn get_memory(&self) -> Result<wasmi::Memory, RuntimeError> {
        self.instance
            .get_memory(&self.store, "memory")
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
    pub fn read_memory(&self, offset: usize, len: usize) -> Result<Vec<u8>, RuntimeError> {
        let memory = self.get_memory()?;
        let mut buffer = vec![0u8; len];
        memory
            .read(&self.store, offset, &mut buffer)
            .map_err(|e| RuntimeError::MemoryError(e.to_string()))?;
        Ok(buffer)
    }

    /// Get the current memory size in bytes
    pub fn memory_size(&self) -> Result<usize, RuntimeError> {
        let memory = self.get_memory()?;
        Ok(memory.current_pages(&self.store).to_bytes().unwrap_or(0))
    }

    /// Call an exported function that takes two i32s and returns an i32
    pub fn call_i32_i32_to_i32(&mut self, name: &str, a: i32, b: i32) -> Result<i32, RuntimeError> {
        let func = self
            .instance
            .get_typed_func::<(i32, i32), i32>(&self.store, name)
            .map_err(|e| RuntimeError::FunctionNotFound(e.to_string()))?;

        func.call(&mut self.store, (a, b))
            .map_err(|e| RuntimeError::WasmError(e.to_string()))
    }

    /// Call an exported function that takes two i64s and returns an i64
    pub fn call_i64_i64_to_i64(&mut self, name: &str, a: i64, b: i64) -> Result<i64, RuntimeError> {
        let func = self
            .instance
            .get_typed_func::<(i64, i64), i64>(&self.store, name)
            .map_err(|e| RuntimeError::FunctionNotFound(e.to_string()))?;

        func.call(&mut self.store, (a, b))
            .map_err(|e| RuntimeError::WasmError(e.to_string()))
    }

    /// Call an exported function that takes two i32s and returns nothing
    pub fn call_i32_i32(&mut self, name: &str, a: i32, b: i32) -> Result<(), RuntimeError> {
        let func = self
            .instance
            .get_typed_func::<(i32, i32), ()>(&self.store, name)
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
    pub fn read_value(&self, offset: usize, len: usize) -> Result<Value, RuntimeError> {
        let bytes = self.read_memory(offset, len)?;
        decode(&bytes).map_err(|e| RuntimeError::AbiError(e.to_string()))
    }

    /// Call a function that takes (in_ptr, in_len) and returns (out_ptr, out_len).
    pub fn call_with_value(
        &mut self,
        name: &str,
        input: &Value,
        input_offset: usize,
    ) -> Result<Value, RuntimeError> {
        let input_len = self.write_value(input_offset, input)?;

        let func = self
            .instance
            .get_typed_func::<(i32, i32), i64>(&self.store, name)
            .map_err(|e| RuntimeError::FunctionNotFound(e.to_string()))?;

        let result = func
            .call(&mut self.store, (input_offset as i32, input_len as i32))
            .map_err(|e| RuntimeError::WasmError(e.to_string()))?;

        let out_ptr = (result & 0xFFFFFFFF) as usize;
        let out_len = ((result >> 32) & 0xFFFFFFFF) as usize;

        self.read_value(out_ptr, out_len)
    }
}

// Implement Instance methods for both () and HostState
impl<T> Instance<T> {
    /// Get the exported memory (assumes it's named "memory")
    fn get_memory(&self) -> Result<wasmi::Memory, RuntimeError> {
        self.instance
            .get_memory(&self.store, "memory")
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
    pub fn read_memory(&self, offset: usize, len: usize) -> Result<Vec<u8>, RuntimeError> {
        let memory = self.get_memory()?;
        let mut buffer = vec![0u8; len];
        memory
            .read(&self.store, offset, &mut buffer)
            .map_err(|e| RuntimeError::MemoryError(e.to_string()))?;
        Ok(buffer)
    }

    /// Get the current memory size in bytes
    pub fn memory_size(&self) -> Result<usize, RuntimeError> {
        let memory = self.get_memory()?;
        // wasmi returns size in pages (64KB each)
        Ok(memory.current_pages(&self.store).to_bytes().unwrap_or(0))
    }

    /// Call an exported function that takes two i32s and returns an i32
    pub fn call_i32_i32_to_i32(&mut self, name: &str, a: i32, b: i32) -> Result<i32, RuntimeError> {
        let func = self
            .instance
            .get_typed_func::<(i32, i32), i32>(&self.store, name)
            .map_err(|e| RuntimeError::FunctionNotFound(e.to_string()))?;

        func.call(&mut self.store, (a, b))
            .map_err(|e| RuntimeError::WasmError(e.to_string()))
    }

    /// Call an exported function that takes two i64s and returns an i64
    pub fn call_i64_i64_to_i64(&mut self, name: &str, a: i64, b: i64) -> Result<i64, RuntimeError> {
        let func = self
            .instance
            .get_typed_func::<(i64, i64), i64>(&self.store, name)
            .map_err(|e| RuntimeError::FunctionNotFound(e.to_string()))?;

        func.call(&mut self.store, (a, b))
            .map_err(|e| RuntimeError::WasmError(e.to_string()))
    }

    /// Call an exported function that takes two i32s and returns nothing
    pub fn call_i32_i32(&mut self, name: &str, a: i32, b: i32) -> Result<(), RuntimeError> {
        let func = self
            .instance
            .get_typed_func::<(i32, i32), ()>(&self.store, name)
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
    pub fn read_value(&self, offset: usize, len: usize) -> Result<Value, RuntimeError> {
        let bytes = self.read_memory(offset, len)?;
        decode(&bytes).map_err(|e| RuntimeError::AbiError(e.to_string()))
    }

    /// Call a function that takes (in_ptr, in_len) and returns (out_ptr, out_len).
    /// Writes the input value to memory, calls the function, reads the output value.
    pub fn call_with_value(
        &mut self,
        name: &str,
        input: &Value,
        input_offset: usize,
    ) -> Result<Value, RuntimeError> {
        // Encode and write input
        let input_len = self.write_value(input_offset, input)?;

        // Call the function - expects (ptr, len) -> (out_ptr, out_len) packed as i64
        // We'll use a convention: function returns two i32s packed in an i64
        // low 32 bits = out_ptr, high 32 bits = out_len
        let func = self
            .instance
            .get_typed_func::<(i32, i32), i64>(&self.store, name)
            .map_err(|e| RuntimeError::FunctionNotFound(e.to_string()))?;

        let result = func
            .call(&mut self.store, (input_offset as i32, input_len as i32))
            .map_err(|e| RuntimeError::WasmError(e.to_string()))?;

        // Unpack result: low 32 bits = ptr, high 32 bits = len
        let out_ptr = (result & 0xFFFFFFFF) as usize;
        let out_len = ((result >> 32) & 0xFFFFFFFF) as usize;

        // Read and decode output
        self.read_value(out_ptr, out_len)
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
            tag: 0,
            payload: Some(Box::new(Value::S64(7))),
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

        let value = Value::Record(vec![("wrong".to_string(), Value::String("x".to_string()))]);
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
