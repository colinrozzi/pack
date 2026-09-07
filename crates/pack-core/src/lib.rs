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
//! ## What's here now
//!
//! The **execution cluster** (host → guest): [`backend`] (the traits) and
//! [`abi_call`] (the export call path, generic over [`WasmInstance`]).
//!
//! ## What's next
//!
//! The **host-import cluster** (guest → host) — a backend-generic `Ctx`,
//! `HostImports`, guest-allocator re-entry, and `CallInterceptor` — is the
//! §4.2 "hard cluster" and gets its own pass. It carries the store-data
//! generics that are still being pinned down.

pub mod abi_call;
pub mod backend;

pub use abi_call::{call_with_value, CallError, INPUT_BUFFER_OFFSET, RESULT_LEN_OFFSET, RESULT_PTR_OFFSET};
pub use backend::{EngineError, Val, WasmEngine, WasmInstance};

/// The graph ABI, re-exported so downstream crates see exactly one `Value` type.
pub use packr_abi as abi;
pub use packr_abi::Value;
