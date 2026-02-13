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
//! │  types     - Unified type system        │
//! │  parser    - Extended WIT parsing       │
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
pub mod compose;
pub mod interface_impl;
pub mod metadata;
pub mod parser;
pub mod runtime;
pub mod types;

pub use abi::{decode, encode};
pub use interface_impl::{
    FuncSignature, HostFunc, InterfaceImpl, PackParams, PackType,
};
pub use metadata::{
    decode_metadata, decode_metadata_with_hashes, encode_metadata, encode_metadata_with_hashes,
    compute_interface_hash, compute_interface_hashes, hash_type, hash_function_from_sig,
    hash_function, hash_interface, hash_list, hash_option, hash_record,
    hash_result, hash_tuple, hash_variant, Binding,
    CaseDesc, FieldDesc, FunctionSignature, InterfaceHash, MetadataError,
    MetadataWithHashes, PackageMetadata, ParamSignature, TypeDesc, TypeHash,
    HASH_BOOL, HASH_CHAR, HASH_F32, HASH_F64, HASH_FLAGS, HASH_S16, HASH_S32,
    HASH_S64, HASH_S8, HASH_STRING, HASH_U16, HASH_U32, HASH_U64, HASH_U8,
};
pub use runtime::{
    validate_instance_implements_interface, AsyncCompiledModule, AsyncCtx, AsyncInstance,
    AsyncRuntime, CallInterceptor, CompiledModule, Ctx, DefaultHostProvider, ErrorHandler,
    HostFunctionError, HostFunctionErrorKind, HostFunctionProvider, HostLinkerBuilder, Instance,
    InterfaceBuilder, InterfaceError, LinkerError, Runtime,
};
pub use parser::{Interface, InterfacePath, TypeDef, World, WorldItem};
pub use types::{Arena, Case, Field, Function, Param, Type, TypePath};

pub use compose::{ComposeError, ParsedModule, StaticComposer};
