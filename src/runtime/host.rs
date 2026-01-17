//! Host Function Registration API
//!
//! Provides a flexible builder pattern for registering host functions with
//! namespaced interfaces, supporting both raw WASM-level access and typed
//! functions with automatic Graph ABI encoding/decoding.
//!
//! # Example
//!
//! ```ignore
//! let instance = module.instantiate_with_host(MyState::new(), |builder| {
//!     builder.interface("theater:simple/runtime")?
//!         .func_typed("log", |ctx, msg: String| {
//!             ctx.data().log(msg);
//!         })?
//!         .func_raw("alloc", |caller, size: i32| -> i32 {
//!             // direct memory access
//!             42
//!         })?;
//!     Ok(())
//! })?;
//! ```

use crate::abi::{decode, encode, Value};
use crate::runtime::RuntimeError;
use std::future::Future;
use std::marker::PhantomData;
use thiserror::Error;
use wasmtime::{Caller, Engine, Linker};

/// Errors from linker operations
#[derive(Error, Debug)]
pub enum LinkerError {
    #[error("Function registration failed: {0}")]
    FunctionRegistration(String),

    #[error("Memory error: {0}")]
    MemoryError(String),

    #[error("Encoding error: {0}")]
    EncodingError(String),

    #[error("Decoding error: {0}")]
    DecodingError(String),

    #[error("Type conversion error: {0}")]
    ConversionError(String),
}

impl From<RuntimeError> for LinkerError {
    fn from(e: RuntimeError) -> Self {
        LinkerError::MemoryError(e.to_string())
    }
}

/// Context wrapper providing ergonomic access to store data and memory.
///
/// Used by typed host functions to access state and perform memory operations.
pub struct Ctx<'a, T> {
    caller: Caller<'a, T>,
}

impl<'a, T> Ctx<'a, T> {
    /// Create a new context from a wasmi Caller
    pub fn new(caller: Caller<'a, T>) -> Self {
        Self { caller }
    }

    /// Get a reference to the store data
    pub fn data(&self) -> &T {
        self.caller.data()
    }

    /// Get a mutable reference to the store data
    pub fn data_mut(&mut self) -> &mut T {
        self.caller.data_mut()
    }

    /// Get the underlying wasmi Caller for advanced operations
    pub fn caller(&self) -> &Caller<'a, T> {
        &self.caller
    }

    /// Get the underlying wasmi Caller mutably
    pub fn caller_mut(&mut self) -> &mut Caller<'a, T> {
        &mut self.caller
    }

    /// Read a Value from WASM memory using the Graph ABI
    pub fn read_value(&mut self, ptr: i32, len: i32) -> Result<Value, LinkerError> {
        let memory = self
            .caller
            .get_export("memory")
            .and_then(|e| e.into_memory())
            .ok_or_else(|| LinkerError::MemoryError("no memory export".into()))?;

        let ptr = ptr as usize;
        let len = len as usize;
        let mut buffer = vec![0u8; len];
        memory
            .read(&self.caller, ptr, &mut buffer)
            .map_err(|e| LinkerError::MemoryError(e.to_string()))?;

        decode(&buffer).map_err(|e| LinkerError::DecodingError(e.to_string()))
    }

    /// Write a Value to WASM memory using the Graph ABI.
    /// Returns (ptr, len) of the written data.
    ///
    /// Uses a simple allocation strategy: writes to a fixed offset (16KB).
    /// For production use, integrate with a proper allocator.
    pub fn write_value(&mut self, value: &Value) -> Result<(i32, i32), LinkerError> {
        let bytes = encode(value).map_err(|e| LinkerError::EncodingError(e.to_string()))?;

        let memory = self
            .caller
            .get_export("memory")
            .and_then(|e| e.into_memory())
            .ok_or_else(|| LinkerError::MemoryError("no memory export".into()))?;

        // Write to a fixed output location (16KB offset)
        // TODO: Use proper allocation when available
        let out_ptr = 16 * 1024;
        memory
            .write(&mut self.caller, out_ptr, &bytes)
            .map_err(|e| LinkerError::MemoryError(e.to_string()))?;

        Ok((out_ptr as i32, bytes.len() as i32))
    }

    /// Read a string from WASM memory
    pub fn read_string(&mut self, ptr: i32, len: i32) -> Result<String, LinkerError> {
        let memory = self
            .caller
            .get_export("memory")
            .and_then(|e| e.into_memory())
            .ok_or_else(|| LinkerError::MemoryError("no memory export".into()))?;

        let ptr = ptr as usize;
        let len = len as usize;
        let mut buffer = vec![0u8; len];
        memory
            .read(&self.caller, ptr, &mut buffer)
            .map_err(|e| LinkerError::MemoryError(e.to_string()))?;

        String::from_utf8(buffer).map_err(|e| LinkerError::DecodingError(e.to_string()))
    }
}

/// Builder for registering host functions with a Linker.
///
/// Generic over `T` which is the store data type.
pub struct HostLinkerBuilder<'a, T> {
    linker: &'a mut Linker<T>,
    engine: &'a Engine,
    _marker: PhantomData<T>,
}

impl<'a, T> HostLinkerBuilder<'a, T> {
    /// Create a new builder wrapping a wasmi Linker
    pub fn new(engine: &'a Engine, linker: &'a mut Linker<T>) -> Self {
        Self {
            linker,
            engine,
            _marker: PhantomData,
        }
    }

    /// Start defining an interface with the given name.
    ///
    /// Interface names follow WIT conventions:
    /// - Simple: `"host"`
    /// - Namespaced: `"theater:simple/runtime"`
    ///
    /// # Example
    ///
    /// ```ignore
    /// builder.interface("theater:simple/runtime")?
    ///     .func_raw("log", |caller, ptr: i32, len: i32| { ... })?;
    /// ```
    pub fn interface(&mut self, name: &str) -> Result<InterfaceBuilder<'_, 'a, T>, LinkerError> {
        Ok(InterfaceBuilder {
            linker: self,
            module_name: name.to_string(),
        })
    }

    /// Register a provider's functions.
    ///
    /// Providers implement `HostFunctionProvider` and can register
    /// multiple interfaces and functions.
    pub fn register_provider<P: HostFunctionProvider<T>>(
        &mut self,
        provider: &P,
    ) -> Result<&mut Self, LinkerError> {
        provider.register(self)?;
        Ok(self)
    }

    /// Get the underlying wasmi Linker for advanced operations
    pub fn inner(&mut self) -> &mut Linker<T> {
        self.linker
    }

    /// Get the engine reference
    pub fn engine(&self) -> &Engine {
        self.engine
    }
}

/// Builder for registering functions within a specific interface/namespace.
pub struct InterfaceBuilder<'a, 'b, T> {
    linker: &'a mut HostLinkerBuilder<'b, T>,
    module_name: String,
}

impl<'a, 'b, T: 'static> InterfaceBuilder<'a, 'b, T> {
    /// Register a raw host function with direct WASM-level parameters.
    ///
    /// Use this for functions that need direct memory access or don't
    /// use the Graph ABI (like allocators).
    ///
    /// # Example
    ///
    /// ```ignore
    /// interface.func_raw("alloc", |caller: Caller<'_, MyState>, size: i32| -> i32 {
    ///     let mut offset = caller.data().alloc_offset.lock().unwrap();
    ///     let ptr = *offset;
    ///     *offset += size as usize;
    ///     ptr as i32
    /// })?;
    /// ```
    pub fn func_raw<Params, Results>(
        &mut self,
        name: &str,
        func: impl wasmtime::IntoFunc<T, Params, Results>,
    ) -> Result<&mut Self, LinkerError> {
        self.linker
            .linker
            .func_wrap(&self.module_name, name, func)
            .map_err(|e| LinkerError::FunctionRegistration(e.to_string()))?;
        Ok(self)
    }

    /// Register a typed host function with automatic Graph ABI encode/decode.
    ///
    /// The parameter type `P` must implement `TryFrom<Value>` and the return
    /// type `R` must implement `Into<Value>`. Use `#[derive(GraphValue)]` to
    /// automatically implement these traits.
    ///
    /// The WASM function signature is `(ptr: i32, len: i32) -> i64` where:
    /// - Input: ptr/len point to Graph ABI encoded input value
    /// - Output: packed i64 containing (out_len << 32 | out_ptr)
    ///
    /// # Example
    ///
    /// ```ignore
    /// #[derive(GraphValue)]
    /// struct Point { x: i64, y: i64 }
    ///
    /// interface.func_typed("translate", |ctx: &mut Ctx<'_, MyState>, point: Point| {
    ///     Point { x: point.x + 10, y: point.y + 10 }
    /// })?;
    /// ```
    pub fn func_typed<P, R, F>(&mut self, name: &str, func: F) -> Result<&mut Self, LinkerError>
    where
        P: TryFrom<Value> + 'static,
        <P as TryFrom<Value>>::Error: std::fmt::Debug,
        R: Into<Value> + 'static,
        F: Fn(&mut Ctx<'_, T>, P) -> R + Send + Sync + 'static,
    {
        let func = std::sync::Arc::new(func);

        self.linker
            .linker
            .func_wrap(
                &self.module_name,
                name,
                move |caller: Caller<'_, T>, ptr: i32, len: i32| -> i64 {
                    let func = func.clone();

                    // Create context - we keep ownership throughout
                    let mut ctx = Ctx::new(caller);

                    // Read and decode input
                    let input_value = match ctx.read_value(ptr, len) {
                        Ok(v) => v,
                        Err(_) => return 0,
                    };

                    // Convert to user type
                    let input: P = match P::try_from(input_value) {
                        Ok(p) => p,
                        Err(_) => return 0,
                    };

                    // Call user function
                    let output: R = func(&mut ctx, input);

                    // Convert result to Value
                    let output_value: Value = output.into();

                    // Write output and return packed pointer/length
                    match ctx.write_value(&output_value) {
                        Ok((out_ptr, out_len)) => {
                            ((out_len as i64) << 32) | (out_ptr as i64 & 0xFFFFFFFF)
                        }
                        Err(_) => 0,
                    }
                },
            )
            .map_err(|e| LinkerError::FunctionRegistration(e.to_string()))?;

        Ok(self)
    }

    /// Register a typed host function that returns a Result.
    ///
    /// Both the success and error types must implement `Into<Value>`.
    /// The result is encoded as a WIT result type:
    /// - `Ok(value)` → `Variant { tag: 0, payload: Some(value) }`
    /// - `Err(error)` → `Variant { tag: 1, payload: Some(error) }`
    ///
    /// # Example
    ///
    /// ```ignore
    /// interface.func_typed_result("parse", |ctx, input: String| -> Result<SExpr, String> {
    ///     parse_sexpr(&input).map_err(|e| e.to_string())
    /// })?;
    /// ```
    pub fn func_typed_result<P, R, E, F>(
        &mut self,
        name: &str,
        func: F,
    ) -> Result<&mut Self, LinkerError>
    where
        P: TryFrom<Value> + 'static,
        <P as TryFrom<Value>>::Error: std::fmt::Debug,
        R: Into<Value> + 'static,
        E: Into<Value> + 'static,
        F: Fn(&mut Ctx<'_, T>, P) -> Result<R, E> + Send + Sync + 'static,
    {
        let func = std::sync::Arc::new(func);

        self.linker
            .linker
            .func_wrap(
                &self.module_name,
                name,
                move |caller: Caller<'_, T>, ptr: i32, len: i32| -> i64 {
                    let func = func.clone();

                    let mut ctx = Ctx::new(caller);

                    // Read and decode input
                    let input_value = match ctx.read_value(ptr, len) {
                        Ok(v) => v,
                        Err(_) => return 0,
                    };

                    // Convert to user type
                    let input: P = match P::try_from(input_value) {
                        Ok(p) => p,
                        Err(_) => return 0,
                    };

                    // Call user function
                    let result = func(&mut ctx, input);

                    // Encode result as WIT result variant
                    let output_value: Value = match result {
                        Ok(value) => Value::Variant {
                            tag: 0,
                            payload: Some(Box::new(value.into())),
                        },
                        Err(error) => Value::Variant {
                            tag: 1,
                            payload: Some(Box::new(error.into())),
                        },
                    };

                    // Write output
                    match ctx.write_value(&output_value) {
                        Ok((out_ptr, out_len)) => {
                            ((out_len as i64) << 32) | (out_ptr as i64 & 0xFFFFFFFF)
                        }
                        Err(_) => 0,
                    }
                },
            )
            .map_err(|e| LinkerError::FunctionRegistration(e.to_string()))?;

        Ok(self)
    }

    /// Get the module/interface name
    pub fn name(&self) -> &str {
        &self.module_name
    }
}

// ============================================================================
// Async Host Functions (require T: Send)
// ============================================================================

impl<'a, 'b, T: Send + 'static> InterfaceBuilder<'a, 'b, T> {
    /// Register an async host function with automatic Graph ABI encode/decode.
    ///
    /// The closure receives parameters decoded from the Graph ABI and should
    /// return a pinned, boxed future that resolves to the return value.
    ///
    /// **Important**: This requires an async-enabled runtime (`AsyncRuntime`).
    ///
    /// # Example
    ///
    /// ```ignore
    /// builder.interface("theater:runtime")?
    ///     .func_async("fetch", |ctx, url: String| {
    ///         Box::pin(async move {
    ///             let response = fetch_url(&url).await;
    ///             response.body
    ///         })
    ///     })?;
    /// ```
    pub fn func_async<P, R, F, Fut>(
        &mut self,
        name: &str,
        func: F,
    ) -> Result<&mut Self, LinkerError>
    where
        P: TryFrom<Value> + Send + 'static,
        <P as TryFrom<Value>>::Error: std::fmt::Debug,
        R: Into<Value> + Send + 'static,
        F: Fn(AsyncCtx<T>, P) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
    {
        let func = std::sync::Arc::new(func);

        self.linker
            .linker
            .func_wrap_async(
                &self.module_name,
                name,
                move |mut caller: Caller<'_, T>, (ptr, len): (i32, i32)| {
                    let func = func.clone();

                    Box::new(async move {
                        // Read memory for input
                        let memory = caller
                            .get_export("memory")
                            .and_then(|e| e.into_memory())
                            .expect("memory export");

                        let mut buffer = vec![0u8; len as usize];
                        memory
                            .read(&caller, ptr as usize, &mut buffer)
                            .expect("read memory");

                        // Decode input
                        let input_value = decode(&buffer).expect("decode input");
                        let input: P = P::try_from(input_value).ok().expect("convert input");

                        // Create async context (captures data we need)
                        let ctx = AsyncCtx {
                            _marker: std::marker::PhantomData,
                        };

                        // Call async function
                        let output: R = func(ctx, input).await;

                        // Encode output
                        let output_value: Value = output.into();
                        let bytes = encode(&output_value).expect("encode output");

                        // Write output to memory
                        let out_ptr = 16 * 1024; // Fixed output location
                        memory
                            .write(&mut caller, out_ptr, &bytes)
                            .expect("write memory");

                        // Return packed pointer/length
                        ((bytes.len() as i64) << 32) | (out_ptr as i64 & 0xFFFFFFFF)
                    })
                },
            )
            .map_err(|e| LinkerError::FunctionRegistration(e.to_string()))?;

        Ok(self)
    }

    /// Register an async host function that returns a Result.
    ///
    /// Both success and error types are encoded as WIT result variants.
    ///
    /// # Example
    ///
    /// ```ignore
    /// builder.interface("theater:runtime")?
    ///     .func_async_result("fetch", |ctx, url: String| {
    ///         Box::pin(async move {
    ///             fetch_url(&url).await.map_err(|e| e.to_string())
    ///         })
    ///     })?;
    /// ```
    pub fn func_async_result<P, R, E, F, Fut>(
        &mut self,
        name: &str,
        func: F,
    ) -> Result<&mut Self, LinkerError>
    where
        P: TryFrom<Value> + Send + 'static,
        <P as TryFrom<Value>>::Error: std::fmt::Debug,
        R: Into<Value> + Send + 'static,
        E: Into<Value> + Send + 'static,
        F: Fn(AsyncCtx<T>, P) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<R, E>> + Send + 'static,
    {
        let func = std::sync::Arc::new(func);

        self.linker
            .linker
            .func_wrap_async(
                &self.module_name,
                name,
                move |mut caller: Caller<'_, T>, (ptr, len): (i32, i32)| {
                    let func = func.clone();

                    Box::new(async move {
                        // Read memory for input
                        let memory = caller
                            .get_export("memory")
                            .and_then(|e| e.into_memory())
                            .expect("memory export");

                        let mut buffer = vec![0u8; len as usize];
                        memory
                            .read(&caller, ptr as usize, &mut buffer)
                            .expect("read memory");

                        // Decode input
                        let input_value = decode(&buffer).expect("decode input");
                        let input: P = P::try_from(input_value).ok().expect("convert input");

                        // Create async context
                        let ctx = AsyncCtx {
                            _marker: std::marker::PhantomData,
                        };

                        // Call async function
                        let result = func(ctx, input).await;

                        // Encode result as WIT result variant
                        let output_value: Value = match result {
                            Ok(value) => Value::Variant {
                                tag: 0,
                                payload: Some(Box::new(value.into())),
                            },
                            Err(error) => Value::Variant {
                                tag: 1,
                                payload: Some(Box::new(error.into())),
                            },
                        };

                        let bytes = encode(&output_value).expect("encode output");

                        // Write output to memory
                        let out_ptr = 16 * 1024;
                        memory
                            .write(&mut caller, out_ptr, &bytes)
                            .expect("write memory");

                        // Return packed pointer/length
                        ((bytes.len() as i64) << 32) | (out_ptr as i64 & 0xFFFFFFFF)
                    })
                },
            )
            .map_err(|e| LinkerError::FunctionRegistration(e.to_string()))?;

        Ok(self)
    }
}

/// Async context for async host functions.
///
/// Provides access to state in async contexts. Note that due to Rust's
/// borrowing rules with async, direct memory access is limited. For
/// complex async operations, capture needed data before the async block.
pub struct AsyncCtx<T> {
    _marker: std::marker::PhantomData<T>,
}

impl<T> AsyncCtx<T> {
    /// Create a new async context.
    pub fn new() -> Self {
        Self {
            _marker: std::marker::PhantomData,
        }
    }
}

impl<T> Default for AsyncCtx<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// Trait for types that provide host functions.
///
/// Implement this to create reusable sets of host functions that can
/// be registered with multiple instances.
///
/// # Example
///
/// ```ignore
/// struct LoggingProvider;
///
/// impl HostFunctionProvider<MyState> for LoggingProvider {
///     fn register(&self, builder: &mut HostLinkerBuilder<'_, MyState>) -> Result<(), LinkerError> {
///         builder.interface("logging")?
///             .func_raw("log", |caller, ptr, len| { ... })?;
///         Ok(())
///     }
/// }
/// ```
pub trait HostFunctionProvider<T> {
    /// Register this provider's functions with the linker builder.
    fn register(&self, builder: &mut HostLinkerBuilder<'_, T>) -> Result<(), LinkerError>;
}

// ============================================================================
// Default Host Provider (backward compatibility)
// ============================================================================

use crate::runtime::HostState;

/// Default host function provider for backward compatibility.
///
/// Provides the "host" module with:
/// - `log(ptr, len)` - Log a string message
/// - `alloc(size) -> ptr` - Bump allocate memory
///
/// This provider is used internally by `instantiate_with_imports()` to maintain
/// compatibility with existing code.
pub struct DefaultHostProvider;

impl HostFunctionProvider<HostState> for DefaultHostProvider {
    fn register(&self, builder: &mut HostLinkerBuilder<'_, HostState>) -> Result<(), LinkerError> {
        builder
            .interface("host")?
            .func_raw(
                "log",
                |mut caller: Caller<'_, HostState>, ptr: i32, len: i32| {
                    let memory = caller
                        .get_export("memory")
                        .and_then(|e| e.into_memory())
                        .expect("memory export");

                    let ptr = ptr as usize;
                    let len = len as usize;
                    let mut buffer = vec![0u8; len];
                    memory.read(&caller, ptr, &mut buffer).expect("read memory");

                    if let Ok(msg) = String::from_utf8(buffer) {
                        caller.data().log_messages.lock().unwrap().push(msg);
                    }
                },
            )?
            .func_raw(
                "alloc",
                |caller: Caller<'_, HostState>, size: i32| -> i32 {
                    let mut offset = caller.data().alloc_offset.lock().unwrap();
                    let ptr = *offset;
                    *offset += size as usize;
                    // Align to 8 bytes
                    *offset = (*offset + 7) & !7;
                    ptr as i32
                },
            )?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_interface_builder_creation() {
        let engine = Engine::default();
        let mut linker = Linker::<()>::new(&engine);
        let mut builder = HostLinkerBuilder::new(&engine, &mut linker);

        // Should accept various interface name formats
        assert!(builder.interface("host").is_ok());
        assert!(builder.interface("theater:simple/runtime").is_ok());
        assert!(builder.interface("wasi:cli/args").is_ok());
    }

    #[test]
    fn test_func_raw_registration() -> Result<(), LinkerError> {
        let engine = Engine::default();
        let mut linker = Linker::<()>::new(&engine);
        let mut builder = HostLinkerBuilder::new(&engine, &mut linker);

        builder
            .interface("test")?
            .func_raw("add", |_caller: Caller<'_, ()>, a: i32, b: i32| a + b)?;

        Ok(())
    }

    struct TestProvider;

    impl HostFunctionProvider<()> for TestProvider {
        fn register(&self, builder: &mut HostLinkerBuilder<'_, ()>) -> Result<(), LinkerError> {
            builder
                .interface("test")?
                .func_raw("noop", |_: Caller<'_, ()>| {})?;
            Ok(())
        }
    }

    #[test]
    fn test_provider_registration() {
        let engine = Engine::default();
        let mut linker = Linker::<()>::new(&engine);
        let mut builder = HostLinkerBuilder::new(&engine, &mut linker);

        let result = builder.register_provider(&TestProvider);
        assert!(result.is_ok());
    }
}
