//! The wasm-backend traits — the "engine axis" seam.
//!
//! This is the **execution cluster** (host → guest) from `docs/engine-axis.md`
//! §4.1: compile a module, instantiate it, call its exports, and touch its
//! linear memory. Everything here is backend-agnostic; `packr-wasmtime` and
//! (later) `packr-web` implement it. The graph-ABI orchestration in
//! [`crate`] runs *on top* of these primitives and never names a concrete
//! engine.
//!
//! Async-first (locked, 2026-09-06): every backend-crossing call is `async`
//! because JS `WebAssembly` is inherently Promise-based. There is no sync path.

use async_trait::async_trait;

use crate::host::HostImports;

/// A numeric wasm value at the call boundary.
///
/// packr's guest ABI is entirely `i32`-typed today (the export convention is
/// `(i32,i32,i32,i32) -> i32`, the allocator is `i32 -> i32`, etc.); `I64` is
/// carried for headroom. Floats are intentionally absent — nothing at the pack
/// ABI boundary uses them. Widen only if an export boundary ever demands it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Val {
    I32(i32),
    I64(i64),
}

impl Val {
    /// The `i32` payload, or `None` if this is some other width.
    pub fn as_i32(self) -> Option<i32> {
        match self {
            Val::I32(v) => Some(v),
            _ => None,
        }
    }

    /// The `i32` payload, panicking on a width mismatch. For call sites that
    /// know the ABI signature (which is all of packr's orchestration).
    pub fn unwrap_i32(self) -> i32 {
        self.as_i32().expect("expected an i32 wasm value at the ABI boundary")
    }
}

impl From<i32> for Val {
    fn from(v: i32) -> Self {
        Val::I32(v)
    }
}

impl From<i64> for Val {
    fn from(v: i64) -> Self {
        Val::I64(v)
    }
}

/// Errors from the backend layer. Deliberately string-carrying at the seam:
/// each backend's native error (wasmtime `Trap`, a JS exception) is stringified
/// here so `packr-core` never depends on a backend's error type.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("failed to compile module: {0}")]
    Compile(String),

    #[error("failed to instantiate module: {0}")]
    Instantiate(String),

    #[error("no export named '{0}'")]
    ExportNotFound(String),

    #[error("call to '{name}' failed: {reason}")]
    Call { name: String, reason: String },

    #[error("guest memory access out of bounds: offset {offset}, len {len}")]
    MemoryOutOfBounds { offset: usize, len: usize },

    #[error("no guest memory available")]
    NoMemory,

    #[error("guest allocation failed for {0} bytes")]
    AllocFailed(usize),

    #[error("{0}")]
    Other(String),
}

/// A live wasm instance: call its exports and read/write its linear memory.
///
/// The orchestration layer drives this to run the pack ABI: allocate an input
/// buffer via a guest export, write encoded bytes, call the export, read the
/// result ptr/len slots, decode. See `docs/engine-axis.md` §3.
#[async_trait]
pub trait WasmInstance: Send {
    /// Call an exported function with numeric args, returning its numeric
    /// results. Async so both wasmtime (`call_async`) and JS fit, and so an
    /// export can transitively re-enter the guest allocator.
    async fn call(&mut self, name: &str, args: &[Val]) -> Result<Vec<Val>, EngineError>;

    /// Copy `buf.len()` bytes of guest linear memory starting at `offset`.
    fn read_memory(&self, offset: usize, buf: &mut [u8]) -> Result<(), EngineError>;

    /// Write `data` into guest linear memory starting at `offset`.
    fn write_memory(&mut self, offset: usize, data: &[u8]) -> Result<(), EngineError>;

    /// Current size of guest linear memory, in bytes.
    fn memory_size(&self) -> usize;

    /// Grow guest linear memory by `delta_pages` 64 KiB pages.
    fn grow_memory(&mut self, delta_pages: u64) -> Result<(), EngineError>;

    /// Whether the instance exports `name`.
    fn has_export(&self, name: &str) -> bool;

    /// Names of all exported functions.
    fn export_names(&self) -> Vec<String>;

    /// Arm the runaway-guest kill switch: the next call traps after
    /// `ticks_until_trap` of the backend's preemption unit.
    ///
    /// Preemption is a capability, not an assumption (§4.4): wasmtime maps this
    /// to an epoch deadline; a browser driver maps it to `Worker.terminate`;
    /// a backend with no native mechanism makes this a documented no-op.
    fn set_deadline(&mut self, ticks_until_trap: u64);
}

/// A wasm engine: compiles module bytes and instantiates them.
///
/// v1 has no compile cache (dropped per the 2026-09-06 decisions):
/// [`WasmEngine::compile`] recompiles each time. When the cache returns it will
/// be backend-owned state behind this same method — the trait shape is
/// cache-ready (`Module` is a cheap-clone handle).
#[async_trait]
pub trait WasmEngine: Send + Sync {
    /// A compiled-module handle. Cheap to clone (wasmtime's `Module` is an
    /// `Arc` inside; a JS `WebAssembly.Module` is a handle) so a future cache
    /// can hand out clones.
    type Module: Clone + Send + Sync;

    /// A live instance produced by [`WasmEngine::instantiate`].
    type Instance: WasmInstance;

    /// Compile wasm bytes into a module.
    async fn compile(&self, bytes: &[u8]) -> Result<Self::Module, EngineError>;

    /// Instantiate a compiled module, satisfying its host imports from
    /// `imports` (plus the built-in default `pack:alloc`). Pass
    /// [`HostImports::new`] for a self-contained actor that imports only
    /// `pack:alloc`.
    async fn instantiate(
        &self,
        module: &Self::Module,
        imports: HostImports,
    ) -> Result<Self::Instance, EngineError>;
}
