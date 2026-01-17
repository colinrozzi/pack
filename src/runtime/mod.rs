//! Component Runtime
//!
//! Handles component instantiation, linking, and execution.

use crate::abi::{encode, Value};
use crate::wit_plus::{decode_with_schema, encode_with_schema, Type, TypeDef};
use std::io::Cursor;
use thiserror::Error;
use wasmi::{Engine, Linker, Module, Store};

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
    pub fn instantiate(&self) -> Result<Instance, RuntimeError> {
        let mut store = Store::new(self.engine, ());
        let linker = Linker::<()>::new(self.engine);

        let instance = linker
            .instantiate(&mut store, &self.module)
            .map_err(|e| RuntimeError::WasmError(e.to_string()))?
            .start(&mut store)
            .map_err(|e| RuntimeError::WasmError(e.to_string()))?;

        Ok(Instance { store, instance })
    }
}

/// A running WASM instance
pub struct Instance {
    store: Store<()>,
    instance: wasmi::Instance,
}

impl Instance {
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
