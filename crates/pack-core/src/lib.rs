//! # packr-core
//!
//! The **environment-agnostic** packr runtime. It defines the wasm-backend
//! traits ([`WasmEngine`] / [`WasmInstance`]) and the graph-ABI orchestration
//! that runs on top of them — with **no dependency on any concrete wasm engine**.
//! `packr-wasmtime` implements the traits for native execution; `packr-web`
//! (later) will implement them over the browser's JS `WebAssembly`.
//!
//! This is the "engine axis" of theater portability. See `docs/engine-axis.md`
//! for the full design; the short version:
//!
//! - **Boundary (A):** abstract only the raw wasm primitives; keep the graph
//!   ABI ([`packr_abi`]) single-sourced *above* the backend line.
//! - **async-first:** every backend-crossing call is `async` (JS forces it).
//!   There is no sync path.
//! - **crate-per-backend:** one impl crate per environment.
//!
//! ## What's here
//!
//! - The **execution cluster** (host → guest): [`backend`] (the traits) and
//!   [`abi_call`] (the export call path, generic over [`WasmInstance`]).
//! - The **host-import cluster** (guest → host): [`host`] (the [`HostImports`]
//!   registry, the [`HostCallCtx`] backend seam, and the `dispatch_host_import`
//!   trampoline) and [`interceptor`] (record/replay). Store-data model is
//!   **capture-based** — host fns capture their state, no typed store is
//!   threaded through the engine (see `docs/engine-axis.md` §4.2).

pub mod abi_call;
pub mod backend;
pub mod host;
pub mod interceptor;
pub mod metadata;
pub mod resume;

pub use abi_call::{
    call_with_value, CallError, INPUT_BUFFER_OFFSET, RESULT_LEN_OFFSET, RESULT_PTR_OFFSET,
};
pub use backend::{EngineError, Val, WasmEngine, WasmInstance};
pub use host::{
    dispatch_host_import, host_call_future, host_fn, typed_host_fn, HostCallCtx, HostError, HostFn,
    HostImport, HostImports,
};
pub use interceptor::CallInterceptor;
pub use resume::{drive, CompletionRegistry, ResumeTarget, TAG_ERROR, TAG_PENDING, TAG_READY};

/// The graph ABI, re-exported so downstream crates see exactly one `Value` type.
pub use packr_abi as abi;
pub use packr_abi::Value;
