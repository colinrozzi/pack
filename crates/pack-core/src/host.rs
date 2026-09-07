//! The host-import cluster (guest → host) — backend-agnostic.
//!
//! When a guest calls an imported function, the pack ABI mirrors the export
//! path: the guest calls `(in_ptr, in_len, out_ptr_slot, out_len_slot) -> status`,
//! the host reads + decodes the input from guest memory, runs the host function,
//! then writes the encoded output back (with `(ptr, len)` into the guest's
//! result slots). Status: `-1` host-side failure, `1` success (guest owns the
//! output buffer and frees it after decoding).
//!
//! **Store-data model: capture-based** (`docs/engine-axis.md` §4.2). A host
//! function is `Fn(Value) -> async Result<Value>` that *captures* whatever host
//! state it needs — there is no typed store threaded through the engine. This
//! matches what the old runtime already did (its async path cloned the store
//! data into the call context; host fns never got `&mut store`), keeps the
//! backend traits non-generic, and is browser-neutral (JS has no typed store).
//!
//! **async-first:** the return is ALWAYS guest-allocated (status `1`); there is
//! no fixed 32 KB scratch buffer, so host returns are unbounded.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use packr_abi::{decode, encode, Value};

use crate::backend::EngineError;
use crate::interceptor::CallInterceptor;

/// A boxed, `Send` future — the return of an async host function.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// A host-side failure of a host function itself (not an application-level
/// error — those are encoded as ordinary pact `result` Values and returned via
/// `Ok`). Surfaces to the guest as status `-1`.
#[derive(Debug, thiserror::Error)]
#[error("host function error: {0}")]
pub struct HostError(pub String);

impl From<String> for HostError {
    fn from(s: String) -> Self {
        HostError(s)
    }
}

impl From<&str> for HostError {
    fn from(s: &str) -> Self {
        HostError(s.to_string())
    }
}

/// A host function: given the decoded input [`Value`], produce an output
/// [`Value`]. async-first; captures its own state (capture-based model).
pub type HostFn = Arc<dyn Fn(Value) -> BoxFuture<'static, Result<Value, HostError>> + Send + Sync>;

/// Wrap an `async fn(Value) -> Result<Value, HostError>` (capturing whatever
/// state it needs) as a [`HostFn`]. Ergonomic sugar over the boxing.
pub fn host_fn<F, Fut>(f: F) -> HostFn
where
    F: Fn(Value) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<Value, HostError>> + Send + 'static,
{
    Arc::new(move |v| Box::pin(f(v)))
}

/// Wrap a *typed* host function as a [`HostFn`], recovering typed I/O ergonomics
/// over the `Value → Value` core without a typed store. The input is converted
/// from the decoded [`Value`] via `TryFrom` (a mismatch surfaces as
/// [`HostError`]) and the output via `Into<Value>`. State is still captured by
/// the closure (capture-based model, `docs/engine-axis.md` §4.2).
pub fn typed_host_fn<P, R, F, Fut>(f: F) -> HostFn
where
    P: TryFrom<Value> + Send + 'static,
    <P as TryFrom<Value>>::Error: core::fmt::Debug,
    R: Into<Value> + Send + 'static,
    F: Fn(P) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<R, HostError>> + Send + 'static,
{
    let f = Arc::new(f);
    Arc::new(move |v| {
        let f = f.clone();
        Box::pin(async move {
            let arg = P::try_from(v)
                .map_err(|e| HostError(format!("host-fn argument type mismatch: {e:?}")))?;
            f(arg).await.map(Into::into)
        })
    })
}

/// Per-call access to the guest, provided by the backend while a host function
/// runs: read/write guest linear memory and re-enter the guest allocator. This
/// is the single backend-specific seam in the guest → host path.
#[async_trait::async_trait]
pub trait HostCallCtx: Send {
    /// Read `buf.len()` bytes of guest memory starting at `offset`.
    fn read(&mut self, offset: usize, buf: &mut [u8]) -> Result<(), EngineError>;

    /// Write `data` into guest memory starting at `offset`.
    fn write(&mut self, offset: usize, data: &[u8]) -> Result<(), EngineError>;

    /// Guest-allocate `size` bytes (calls the guest's `__pack_alloc`). Async so
    /// wasmtime re-enters via `call_async` and JS via a Promise.
    async fn alloc(&mut self, size: usize) -> Result<usize, EngineError>;

    /// Free a guest buffer (calls `__pack_free` if the guest exports it).
    async fn free(&mut self, ptr: usize, size: usize) -> Result<(), EngineError>;
}

/// One registered host import.
#[derive(Clone)]
pub struct HostImport {
    pub interface: String,
    pub function: String,
    pub func: HostFn,
}

/// The host imports (guest → host functions) to satisfy at instantiate time,
/// plus an optional [`CallInterceptor`]. Passed to `WasmEngine::instantiate`.
#[derive(Clone, Default)]
pub struct HostImports {
    imports: Vec<HostImport>,
    interceptor: Option<Arc<dyn CallInterceptor>>,
}

impl HostImports {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a host function under `interface.function`.
    pub fn define(&mut self, interface: &str, function: &str, func: HostFn) -> &mut Self {
        self.imports.push(HostImport {
            interface: interface.to_string(),
            function: function.to_string(),
            func,
        });
        self
    }

    /// Install a record/replay interceptor for this instance.
    pub fn with_interceptor(&mut self, interceptor: Arc<dyn CallInterceptor>) -> &mut Self {
        self.interceptor = Some(interceptor);
        self
    }

    pub fn imports(&self) -> &[HostImport] {
        &self.imports
    }

    pub fn interceptor(&self) -> Option<&Arc<dyn CallInterceptor>> {
        self.interceptor.as_ref()
    }
}

/// Run one guest → host call. The backend builds a [`HostCallCtx`] over its live
/// instance and calls this with the raw ABI arguments; all decode / interceptor
/// / encode / guest-allocate logic lives here, once, backend-free.
///
/// Returns the pack host-import status: `-1` on any host-side failure, `1` on
/// success (the output buffer is guest-allocated and the guest frees it).
#[allow(clippy::too_many_arguments)]
pub async fn dispatch_host_import(
    ctx: &mut dyn HostCallCtx,
    func: &HostFn,
    interceptor: Option<&Arc<dyn CallInterceptor>>,
    interface: &str,
    function: &str,
    in_ptr: u32,
    in_len: u32,
    out_ptr_slot: u32,
    out_len_slot: u32,
) -> i32 {
    // Read + decode the input from guest memory.
    let mut buf = vec![0u8; in_len as usize];
    if ctx.read(in_ptr as usize, &mut buf).is_err() {
        return -1;
    }
    let input = match decode(&buf) {
        Ok(v) => v,
        Err(_) => return -1,
    };

    // Interceptor replay: short-circuit with the recorded output.
    if let Some(ic) = interceptor {
        if let Some(recorded) = ic.before_import(interface, function, &input).await {
            ic.after_import(interface, function, &input, &recorded)
                .await;
            return write_output(ctx, out_ptr_slot, out_len_slot, &recorded).await;
        }
    }

    // Keep a copy for the after_import notification only when recording.
    let input_for_after = interceptor.map(|_| input.clone());

    // Invoke the host function.
    let output = match func(input).await {
        Ok(v) => v,
        Err(_) => return -1,
    };

    if let (Some(ic), Some(iv)) = (interceptor, input_for_after.as_ref()) {
        ic.after_import(interface, function, iv, &output).await;
    }

    write_output(ctx, out_ptr_slot, out_len_slot, &output).await
}

/// Encode `value`, guest-allocate a buffer, write it, and store `(ptr, len)`
/// into the guest's result slots. Returns status `1` (guest owns the buffer) or
/// `-1` on failure.
async fn write_output(
    ctx: &mut dyn HostCallCtx,
    out_ptr_slot: u32,
    out_len_slot: u32,
    value: &Value,
) -> i32 {
    let bytes = match encode(value) {
        Ok(b) => b,
        Err(_) => return -1,
    };
    let ptr = match ctx.alloc(bytes.len()).await {
        Ok(p) => p,
        Err(_) => return -1,
    };
    if ctx.write(ptr, &bytes).is_err() {
        return -1;
    }
    if ctx
        .write(out_ptr_slot as usize, &(ptr as u32).to_le_bytes())
        .is_err()
    {
        return -1;
    }
    if ctx
        .write(out_len_slot as usize, &(bytes.len() as u32).to_le_bytes())
        .is_err()
    {
        return -1;
    }
    1
}
