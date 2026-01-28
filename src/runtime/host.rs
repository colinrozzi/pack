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
use std::sync::Arc;
use thiserror::Error;
use wasmtime::{Caller, Engine, Linker};

// ============================================================================
// Calling Convention Constants
// ============================================================================

/// Default input buffer offset (0-16KB)
pub const INPUT_BUFFER_OFFSET: usize = 0;

/// Offset for result pointer slot (4 bytes at 16KB)
/// Used by new guest-allocates ABI: guest writes output ptr here
pub const RESULT_PTR_OFFSET: usize = 16 * 1024;

/// Offset for result length slot (4 bytes at 16KB + 4)
/// Used by new guest-allocates ABI: guest writes output len here
pub const RESULT_LEN_OFFSET: usize = 16 * 1024 + 4;

// Legacy constants - kept for backward compatibility during transition
/// Default output buffer offset (16KB) - DEPRECATED: use RESULT_PTR_OFFSET
pub const OUTPUT_BUFFER_OFFSET: usize = 16 * 1024;

/// Default output buffer capacity (32KB) - DEPRECATED: guest now allocates
pub const OUTPUT_BUFFER_CAPACITY: usize = 32 * 1024;

// ============================================================================
// Error Handling Infrastructure
// ============================================================================

/// Error that occurred during host function execution.
///
/// This provides context about where and why an error occurred,
/// useful for debugging and logging.
#[derive(Debug, Clone)]
pub struct HostFunctionError {
    /// The interface/module name (e.g., "theater:simple/runtime")
    pub interface: String,
    /// The function name (e.g., "log")
    pub function: String,
    /// The kind of error that occurred
    pub kind: HostFunctionErrorKind,
}

impl std::fmt::Display for HostFunctionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "host function error in {}::{}: {}",
            self.interface, self.function, self.kind
        )
    }
}

impl std::error::Error for HostFunctionError {}

/// The specific kind of error that occurred in a host function.
#[derive(Debug, Clone)]
pub enum HostFunctionErrorKind {
    /// Failed to read from WASM memory
    MemoryRead(String),
    /// Failed to decode Graph ABI data
    Decode(String),
    /// Failed to convert Value to the expected type
    TypeConversion(String),
    /// Failed to write to WASM memory
    MemoryWrite(String),
    /// Failed to encode output as Graph ABI
    Encode(String),
}

impl std::fmt::Display for HostFunctionErrorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MemoryRead(e) => write!(f, "memory read failed: {}", e),
            Self::Decode(e) => write!(f, "decode failed: {}", e),
            Self::TypeConversion(e) => write!(f, "type conversion failed: {}", e),
            Self::MemoryWrite(e) => write!(f, "memory write failed: {}", e),
            Self::Encode(e) => write!(f, "encode failed: {}", e),
        }
    }
}

/// Handler function for host function errors.
///
/// This is called whenever an error occurs in a typed host function,
/// allowing for logging, metrics, or other error handling.
pub type ErrorHandler = Arc<dyn Fn(&HostFunctionError) + Send + Sync>;

/// Default error handler that logs to stderr.
fn default_error_handler(err: &HostFunctionError) {
    eprintln!("[composite] {}", err);
}

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

    /// Write a Value to WASM memory at the specified location.
    /// Returns the number of bytes written.
    ///
    /// The caller provides the output buffer location and capacity.
    /// Returns an error if the encoded data exceeds the capacity.
    pub fn write_value_at(
        &mut self,
        out_ptr: i32,
        out_cap: i32,
        value: &Value,
    ) -> Result<i32, LinkerError> {
        let bytes = encode(value).map_err(|e| LinkerError::EncodingError(e.to_string()))?;

        if bytes.len() > out_cap as usize {
            return Err(LinkerError::MemoryError(format!(
                "output buffer too small: need {} bytes, have {} capacity",
                bytes.len(),
                out_cap
            )));
        }

        let memory = self
            .caller
            .get_export("memory")
            .and_then(|e| e.into_memory())
            .ok_or_else(|| LinkerError::MemoryError("no memory export".into()))?;

        memory
            .write(&mut self.caller, out_ptr as usize, &bytes)
            .map_err(|e| LinkerError::MemoryError(e.to_string()))?;

        Ok(bytes.len() as i32)
    }

    /// Write a Value to WASM memory using the Graph ABI (legacy method).
    /// Returns (ptr, len) of the written data.
    ///
    /// **Deprecated**: Prefer `write_value_at()` with explicit buffer location.
    /// Uses a fixed offset (16KB) for backward compatibility.
    pub fn write_value(&mut self, value: &Value) -> Result<(i32, i32), LinkerError> {
        let out_ptr = OUTPUT_BUFFER_OFFSET as i32;
        let out_cap = OUTPUT_BUFFER_CAPACITY as i32;
        let len = self.write_value_at(out_ptr, out_cap, value)?;
        Ok((out_ptr, len))
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
    error_handler: Option<ErrorHandler>,
    _marker: PhantomData<T>,
}

impl<'a, T> HostLinkerBuilder<'a, T> {
    /// Create a new builder wrapping a wasmtime Linker
    pub fn new(engine: &'a Engine, linker: &'a mut Linker<T>) -> Self {
        Self {
            linker,
            engine,
            error_handler: None,
            _marker: PhantomData,
        }
    }

    /// Set a custom error handler for host function errors.
    ///
    /// The handler is called whenever an error occurs in a typed host function
    /// (e.g., decode failure, type conversion error, memory write failure).
    ///
    /// # Example
    ///
    /// ```ignore
    /// builder.on_error(|err| {
    ///     tracing::error!("Host function error: {}", err);
    ///     metrics::increment("host_function_errors");
    /// });
    /// ```
    pub fn on_error<F>(&mut self, handler: F) -> &mut Self
    where
        F: Fn(&HostFunctionError) + Send + Sync + 'static,
    {
        self.error_handler = Some(Arc::new(handler));
        self
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
        let error_handler = self.error_handler.clone();
        Ok(InterfaceBuilder {
            linker: self,
            module_name: name.to_string(),
            error_handler,
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

    /// Get the underlying wasmtime Linker for advanced operations
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
    error_handler: Option<ErrorHandler>,
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
    /// The WASM function signature is `(in_ptr, in_len, out_ptr_ptr, out_len_ptr) -> status`:
    /// - Input: in_ptr/in_len point to Graph ABI encoded input value
    /// - Output: host writes data to OUTPUT_BUFFER_OFFSET, then writes ptr/len to the provided slots
    /// - Returns 0 on success, -1 on error
    ///
    /// Errors during decode/encode are logged via the error handler (see
    /// `HostLinkerBuilder::on_error`). On error, returns -1.
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
        let func = Arc::new(func);
        let error_handler = self.error_handler.clone();
        let interface_name = self.module_name.clone();
        let func_name = name.to_string();

        self.linker
            .linker
            .func_wrap(
                &self.module_name,
                name,
                move |caller: Caller<'_, T>,
                      in_ptr: i32,
                      in_len: i32,
                      out_ptr_ptr: i32,
                      out_len_ptr: i32|
                      -> i32 {
                    let func = func.clone();
                    let error_handler = error_handler.clone();
                    let interface_name = interface_name.clone();
                    let func_name = func_name.clone();

                    // Helper to report errors
                    let report = |kind: HostFunctionErrorKind| {
                        let error = HostFunctionError {
                            interface: interface_name.clone(),
                            function: func_name.clone(),
                            kind,
                        };
                        if let Some(handler) = &error_handler {
                            handler(&error);
                        } else {
                            default_error_handler(&error);
                        }
                    };

                    // Create context - we keep ownership throughout
                    let mut ctx = Ctx::new(caller);

                    // Read and decode input
                    let input_value = match ctx.read_value(in_ptr, in_len) {
                        Ok(v) => v,
                        Err(e) => {
                            report(HostFunctionErrorKind::Decode(e.to_string()));
                            return -1;
                        }
                    };

                    // Convert to user type
                    let input: P = match P::try_from(input_value) {
                        Ok(p) => p,
                        Err(e) => {
                            report(HostFunctionErrorKind::TypeConversion(format!("{:?}", e)));
                            return -1;
                        }
                    };

                    // Call user function
                    let output: R = func(&mut ctx, input);

                    // Convert result to Value and encode
                    let output_value: Value = output.into();
                    let bytes = match encode(&output_value) {
                        Ok(b) => b,
                        Err(e) => {
                            report(HostFunctionErrorKind::Encode(e.to_string()));
                            return -1;
                        }
                    };

                    // Write output to a fixed buffer location
                    let memory = match ctx.caller.get_export("memory").and_then(|e| e.into_memory()) {
                        Some(m) => m,
                        None => {
                            report(HostFunctionErrorKind::MemoryWrite("no memory export".to_string()));
                            return -1;
                        }
                    };

                    // Write data to OUTPUT_BUFFER_OFFSET (after the slots)
                    let data_offset = OUTPUT_BUFFER_OFFSET + 8; // After the ptr/len slots
                    if let Err(e) = memory.write(&mut ctx.caller, data_offset, &bytes) {
                        report(HostFunctionErrorKind::MemoryWrite(e.to_string()));
                        return -1;
                    }

                    // Write ptr and len to the caller's slots
                    if let Err(e) = memory.write(&mut ctx.caller, out_ptr_ptr as usize, &(data_offset as i32).to_le_bytes()) {
                        report(HostFunctionErrorKind::MemoryWrite(e.to_string()));
                        return -1;
                    }
                    if let Err(e) = memory.write(&mut ctx.caller, out_len_ptr as usize, &(bytes.len() as i32).to_le_bytes()) {
                        report(HostFunctionErrorKind::MemoryWrite(e.to_string()));
                        return -1;
                    }

                    0 // Success
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
    /// The WASM function signature is `(in_ptr, in_len, out_ptr_ptr, out_len_ptr) -> status`.
    /// On decode/encode errors, returns -1.
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
        let func = Arc::new(func);
        let error_handler = self.error_handler.clone();
        let interface_name = self.module_name.clone();
        let func_name = name.to_string();

        self.linker
            .linker
            .func_wrap(
                &self.module_name,
                name,
                move |caller: Caller<'_, T>,
                      in_ptr: i32,
                      in_len: i32,
                      out_ptr_ptr: i32,
                      out_len_ptr: i32|
                      -> i32 {
                    let func = func.clone();
                    let error_handler = error_handler.clone();
                    let interface_name = interface_name.clone();
                    let func_name = func_name.clone();

                    // Helper to report errors
                    let report = |kind: HostFunctionErrorKind| {
                        let error = HostFunctionError {
                            interface: interface_name.clone(),
                            function: func_name.clone(),
                            kind,
                        };
                        if let Some(handler) = &error_handler {
                            handler(&error);
                        } else {
                            default_error_handler(&error);
                        }
                    };

                    let mut ctx = Ctx::new(caller);

                    // Read and decode input
                    let input_value = match ctx.read_value(in_ptr, in_len) {
                        Ok(v) => v,
                        Err(e) => {
                            report(HostFunctionErrorKind::Decode(e.to_string()));
                            return -1;
                        }
                    };

                    // Convert to user type
                    let input: P = match P::try_from(input_value) {
                        Ok(p) => p,
                        Err(e) => {
                            report(HostFunctionErrorKind::TypeConversion(format!("{:?}", e)));
                            return -1;
                        }
                    };

                    // Call user function
                    let result = func(&mut ctx, input);

                    // Encode result as WIT result variant
                    let output_value: Value = match result {
                        Ok(value) => Value::Variant {
                            type_name: "result".to_string(),
                            case_name: "ok".to_string(),
                            tag: 0,
                            payload: vec![value.into()],
                        },
                        Err(error) => Value::Variant {
                            type_name: "result".to_string(),
                            case_name: "error".to_string(),
                            tag: 1,
                            payload: vec![error.into()],
                        },
                    };

                    // Encode output
                    let bytes = match encode(&output_value) {
                        Ok(b) => b,
                        Err(e) => {
                            report(HostFunctionErrorKind::Encode(e.to_string()));
                            return -1;
                        }
                    };

                    // Write output to a fixed buffer location
                    let memory = match ctx.caller.get_export("memory").and_then(|e| e.into_memory()) {
                        Some(m) => m,
                        None => {
                            report(HostFunctionErrorKind::MemoryWrite("no memory export".to_string()));
                            return -1;
                        }
                    };

                    let data_offset = OUTPUT_BUFFER_OFFSET + 8;
                    if let Err(e) = memory.write(&mut ctx.caller, data_offset, &bytes) {
                        report(HostFunctionErrorKind::MemoryWrite(e.to_string()));
                        return -1;
                    }

                    // Write ptr and len to the caller's slots
                    if let Err(e) = memory.write(&mut ctx.caller, out_ptr_ptr as usize, &(data_offset as i32).to_le_bytes()) {
                        report(HostFunctionErrorKind::MemoryWrite(e.to_string()));
                        return -1;
                    }
                    if let Err(e) = memory.write(&mut ctx.caller, out_len_ptr as usize, &(bytes.len() as i32).to_le_bytes()) {
                        report(HostFunctionErrorKind::MemoryWrite(e.to_string()));
                        return -1;
                    }

                    0 // Success
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

impl<'a, 'b, T: Send + Clone + 'static> InterfaceBuilder<'a, 'b, T> {
    /// Register an async host function with automatic Graph ABI encode/decode.
    ///
    /// The closure receives an `AsyncCtx` containing a cloned copy of the store
    /// state, plus the decoded input parameter. The state is cloned before
    /// entering the async block to avoid lifetime issues.
    ///
    /// **Important**: This requires an async-enabled runtime (`AsyncRuntime`).
    ///
    /// The WASM function signature is `(in_ptr, in_len, out_ptr_ptr, out_len_ptr) -> status`.
    /// On decode/encode errors, returns -1.
    ///
    /// # Example
    ///
    /// ```ignore
    /// builder.interface("theater:runtime")?
    ///     .func_async("fetch", |ctx: AsyncCtx<MyState>, url: String| async move {
    ///         // Access state through ctx.data()
    ///         let base_url = ctx.data().base_url.clone();
    ///         let response = fetch_url(&format!("{}/{}", base_url, url)).await;
    ///         response.body
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
        let func = Arc::new(func);
        let error_handler = self.error_handler.clone();
        let interface_name = self.module_name.clone();
        let func_name = name.to_string();

        self.linker
            .linker
            .func_wrap_async(
                &self.module_name,
                name,
                move |mut caller: Caller<'_, T>,
                      (in_ptr, in_len, out_ptr_ptr, out_len_ptr): (i32, i32, i32, i32)| {
                    let func = func.clone();
                    let error_handler = error_handler.clone();
                    let interface_name = interface_name.clone();
                    let func_name = func_name.clone();

                    // Clone state before entering async block
                    let state = caller.data().clone();

                    Box::new(async move {
                        // Helper to report errors
                        let report = |kind: HostFunctionErrorKind| {
                            let error = HostFunctionError {
                                interface: interface_name.clone(),
                                function: func_name.clone(),
                                kind,
                            };
                            if let Some(handler) = &error_handler {
                                handler(&error);
                            } else {
                                default_error_handler(&error);
                            }
                        };

                        // Read memory for input
                        let memory = match caller
                            .get_export("memory")
                            .and_then(|e| e.into_memory())
                        {
                            Some(m) => m,
                            None => {
                                report(HostFunctionErrorKind::MemoryRead(
                                    "no memory export".to_string(),
                                ));
                                return -1;
                            }
                        };

                        let mut buffer = vec![0u8; in_len as usize];
                        if let Err(e) = memory.read(&caller, in_ptr as usize, &mut buffer) {
                            report(HostFunctionErrorKind::MemoryRead(e.to_string()));
                            return -1;
                        }

                        // Decode input
                        let input_value = match decode(&buffer) {
                            Ok(v) => v,
                            Err(e) => {
                                report(HostFunctionErrorKind::Decode(e.to_string()));
                                return -1;
                            }
                        };

                        let input: P = match P::try_from(input_value) {
                            Ok(p) => p,
                            Err(e) => {
                                report(HostFunctionErrorKind::TypeConversion(format!("{:?}", e)));
                                return -1;
                            }
                        };

                        // Create async context with cloned state
                        let ctx = AsyncCtx::new(state);

                        // Call async function
                        let output: R = func(ctx, input).await;

                        // Encode output
                        let output_value: Value = output.into();
                        let bytes = match encode(&output_value) {
                            Ok(b) => b,
                            Err(e) => {
                                report(HostFunctionErrorKind::Encode(e.to_string()));
                                return -1;
                            }
                        };

                        // Write data to OUTPUT_BUFFER_OFFSET (after the slots)
                        let data_offset = OUTPUT_BUFFER_OFFSET + 8;
                        if let Err(e) = memory.write(&mut caller, data_offset, &bytes) {
                            report(HostFunctionErrorKind::MemoryWrite(e.to_string()));
                            return -1;
                        }

                        // Write ptr and len to the caller's slots
                        if let Err(e) = memory.write(&mut caller, out_ptr_ptr as usize, &(data_offset as i32).to_le_bytes()) {
                            report(HostFunctionErrorKind::MemoryWrite(e.to_string()));
                            return -1;
                        }
                        if let Err(e) = memory.write(&mut caller, out_len_ptr as usize, &(bytes.len() as i32).to_le_bytes()) {
                            report(HostFunctionErrorKind::MemoryWrite(e.to_string()));
                            return -1;
                        }

                        0 // Success
                    })
                },
            )
            .map_err(|e| LinkerError::FunctionRegistration(e.to_string()))?;

        Ok(self)
    }

    /// Register an async host function that returns a Result.
    ///
    /// Both success and error types are encoded as WIT result variants.
    /// The `AsyncCtx` contains a cloned copy of the store state.
    ///
    /// The WASM function signature is `(in_ptr, in_len, out_ptr_ptr, out_len_ptr) -> status`.
    /// On decode/encode errors, returns -1.
    ///
    /// # Example
    ///
    /// ```ignore
    /// builder.interface("theater:runtime")?
    ///     .func_async_result("fetch", |ctx: AsyncCtx<MyState>, url: String| async move {
    ///         let base = ctx.data().base_url.clone();
    ///         fetch_url(&format!("{}/{}", base, url)).await.map_err(|e| e.to_string())
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
        let func = Arc::new(func);
        let error_handler = self.error_handler.clone();
        let interface_name = self.module_name.clone();
        let func_name = name.to_string();

        self.linker
            .linker
            .func_wrap_async(
                &self.module_name,
                name,
                move |mut caller: Caller<'_, T>,
                      (in_ptr, in_len, out_ptr_ptr, out_len_ptr): (i32, i32, i32, i32)| {
                    let func = func.clone();
                    let error_handler = error_handler.clone();
                    let interface_name = interface_name.clone();
                    let func_name = func_name.clone();

                    // Clone state before entering async block
                    let state = caller.data().clone();

                    Box::new(async move {
                        // Helper to report errors
                        let report = |kind: HostFunctionErrorKind| {
                            let error = HostFunctionError {
                                interface: interface_name.clone(),
                                function: func_name.clone(),
                                kind,
                            };
                            if let Some(handler) = &error_handler {
                                handler(&error);
                            } else {
                                default_error_handler(&error);
                            }
                        };

                        // Read memory for input
                        let memory = match caller
                            .get_export("memory")
                            .and_then(|e| e.into_memory())
                        {
                            Some(m) => m,
                            None => {
                                report(HostFunctionErrorKind::MemoryRead(
                                    "no memory export".to_string(),
                                ));
                                return -1;
                            }
                        };

                        let mut buffer = vec![0u8; in_len as usize];
                        if let Err(e) = memory.read(&caller, in_ptr as usize, &mut buffer) {
                            report(HostFunctionErrorKind::MemoryRead(e.to_string()));
                            return -1;
                        }

                        // Decode input
                        let input_value = match decode(&buffer) {
                            Ok(v) => v,
                            Err(e) => {
                                report(HostFunctionErrorKind::Decode(e.to_string()));
                                return -1;
                            }
                        };

                        let input: P = match P::try_from(input_value) {
                            Ok(p) => p,
                            Err(e) => {
                                report(HostFunctionErrorKind::TypeConversion(format!("{:?}", e)));
                                return -1;
                            }
                        };

                        // Create async context with cloned state
                        let ctx = AsyncCtx::new(state);

                        // Call async function
                        let result = func(ctx, input).await;

                        // Encode result as WIT result variant
                        let output_value: Value = match result {
                            Ok(value) => Value::Variant {
                                type_name: "result".to_string(),
                                case_name: "ok".to_string(),
                                tag: 0,
                                payload: vec![value.into()],
                            },
                            Err(error) => Value::Variant {
                                type_name: "result".to_string(),
                                case_name: "error".to_string(),
                                tag: 1,
                                payload: vec![error.into()],
                            },
                        };

                        let bytes = match encode(&output_value) {
                            Ok(b) => b,
                            Err(e) => {
                                report(HostFunctionErrorKind::Encode(e.to_string()));
                                return -1;
                            }
                        };

                        // Write data to OUTPUT_BUFFER_OFFSET (after the slots)
                        let data_offset = OUTPUT_BUFFER_OFFSET + 8;
                        if let Err(e) = memory.write(&mut caller, data_offset, &bytes) {
                            report(HostFunctionErrorKind::MemoryWrite(e.to_string()));
                            return -1;
                        }

                        // Write ptr and len to the caller's slots
                        if let Err(e) = memory.write(&mut caller, out_ptr_ptr as usize, &(data_offset as i32).to_le_bytes()) {
                            report(HostFunctionErrorKind::MemoryWrite(e.to_string()));
                            return -1;
                        }
                        if let Err(e) = memory.write(&mut caller, out_len_ptr as usize, &(bytes.len() as i32).to_le_bytes()) {
                            report(HostFunctionErrorKind::MemoryWrite(e.to_string()));
                            return -1;
                        }

                        0 // Success
                    })
                },
            )
            .map_err(|e| LinkerError::FunctionRegistration(e.to_string()))?;

        Ok(self)
    }
}

/// Async context for async host functions.
///
/// Provides access to a cloned copy of the store state for use in async
/// operations. Since async functions can't hold references across await
/// points, state is cloned before the async block.
///
/// # Example
///
/// ```ignore
/// builder.interface("theater:runtime")?
///     .func_async("process", |ctx: AsyncCtx<MyState>, input: Value| async move {
///         // Access cloned state
///         let config = ctx.data().config.clone();
///         // Async operations...
///         process_with_config(&config, input).await
///     })?;
/// ```
pub struct AsyncCtx<T> {
    state: T,
}

impl<T> AsyncCtx<T> {
    /// Create a new async context with the given state.
    pub fn new(state: T) -> Self {
        Self { state }
    }

    /// Get a reference to the store state.
    pub fn data(&self) -> &T {
        &self.state
    }

    /// Get a mutable reference to the store state.
    ///
    /// Note: Changes to state in async contexts are isolated to this
    /// cloned copy and won't affect the original store state.
    pub fn data_mut(&mut self) -> &mut T {
        &mut self.state
    }

    /// Consume the context and return the owned state.
    pub fn into_inner(self) -> T {
        self.state
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
