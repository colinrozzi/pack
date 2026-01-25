//! Composite: A package runtime with extended WIT support
//!
//! This runtime extends the WebAssembly Component Model with support for
//! recursive data types, enabling natural representation of tree structures
//! like ASTs, S-expressions, and other recursive data.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────┐
//! │           Composite Runtime             │
//! │                                         │
//! │  wit_plus  - Extended WIT parsing       │
//! │  abi       - Type encoding/decoding     │
//! │  runtime   - Package instantiation      │
//! │                                         │
//! ├─────────────────────────────────────────┤
//! │       WASM Execution (wasmtime)         │
//! └─────────────────────────────────────────┘
//! ```
//!
//! ## Extended WIT Types
//!
//! WIT+ allows recursive types by default:
//!
//! ```wit
//! variant sexpr {
//!     sym(string),
//!     num(s64),
//!     lst(list<sexpr>),
//! }
//! ```
//!
//! ## Async Support
//!
//! For async host functions, use `AsyncRuntime`:
//!
//! ```ignore
//! let runtime = AsyncRuntime::new();
//! let module = runtime.load_module(&wasm_bytes)?;
//!
//! let instance = module.instantiate_with_host_async(MyState::new(), |builder| {
//!     builder.interface("theater:runtime")?
//!         .func_async("fetch", |ctx, url: String| {
//!             Box::pin(async move { fetch(&url).await })
//!         })?;
//!     Ok(())
//! }).await?;
//! ```

pub mod abi;
pub mod runtime;
pub mod wit_plus;

pub use abi::{decode, encode};
pub use runtime::{
    validate_instance_implements_interface, AsyncCompiledModule, AsyncCtx, AsyncInstance,
    AsyncRuntime, CompiledModule, Ctx, DefaultHostProvider, ErrorHandler, HostFunctionError,
    HostFunctionErrorKind, HostFunctionProvider, HostLinkerBuilder, Instance, InterfaceBuilder,
    InterfaceError, LinkerError, Runtime,
};
pub use wit_plus::{Interface, InterfacePath, TypeDef, World, WorldItem};
