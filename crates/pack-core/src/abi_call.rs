//! The pack-ABI export call path — backend-agnostic.
//!
//! This is `AsyncInstance::call_with_value_async` (`src/runtime/mod.rs:641`)
//! re-expressed over the [`WasmInstance`] trait instead of a concrete
//! `wasmtime` instance. It is the proof of `docs/engine-axis.md` §3: the call
//! path is pure ABI orchestration over a handful of backend primitives
//! (allocate, write memory, call export, read memory). Nothing here names an
//! engine.
//!
//! Interceptor hooks (record/replay) are intentionally omitted for now — they
//! belong with the host-import cluster pass (§4.2), since `CallInterceptor`
//! also mediates the guest → host direction.

use crate::backend::{EngineError, Val, WasmInstance};
use packr_abi::{decode, encode, Value};

/// Default input buffer offset, used only when the guest exports no allocator.
pub const INPUT_BUFFER_OFFSET: usize = 0;

/// Slot where the guest writes the output pointer (guest-allocates ABI).
pub const RESULT_PTR_OFFSET: usize = 16 * 1024;

/// Slot where the guest writes the output length (guest-allocates ABI).
pub const RESULT_LEN_OFFSET: usize = 16 * 1024 + 4;

/// An error from an ABI-level export call.
#[derive(Debug, thiserror::Error)]
pub enum CallError {
    /// The backend failed (compile/instantiate/call/memory).
    #[error(transparent)]
    Engine(#[from] EngineError),

    /// Graph encode/decode failed.
    #[error("ABI error: {0}")]
    Abi(String),

    /// The guest function ran but returned a non-zero status; the message is
    /// the guest-supplied error string.
    #[error("guest function '{name}' returned an error: {message}")]
    Guest { name: String, message: String },
}

/// Call an exported pack function with a [`Value`] argument, returning its
/// [`Value`] result. Drives the guest ABI end to end:
///
/// 1. encode the input,
/// 2. allocate an input buffer via the guest allocator (fixed-offset fallback),
/// 3. call `name(in_ptr, in_len, result_ptr_slot, result_len_slot) -> status`,
/// 4. read the output ptr/len slots, then the output (or error) bytes,
/// 5. decode, freeing guest buffers along the way.
pub async fn call_with_value<I>(inst: &mut I, name: &str, input: &Value) -> Result<Value, CallError>
where
    I: WasmInstance + ?Sized,
{
    let input_bytes = encode(input).map_err(|e| CallError::Abi(format!("{e:?}")))?;

    // Allocate the input buffer in guest memory; fall back to the fixed buffer
    // if the guest exports no allocator.
    let (in_ptr, dynamic_input) = match pack_alloc(inst, input_bytes.len()).await {
        Ok(ptr) => (ptr, true),
        Err(_) => (INPUT_BUFFER_OFFSET, false),
    };
    inst.write_memory(in_ptr, &input_bytes)?;

    let results = inst
        .call(
            name,
            &[
                Val::I32(in_ptr as i32),
                Val::I32(input_bytes.len() as i32),
                Val::I32(RESULT_PTR_OFFSET as i32),
                Val::I32(RESULT_LEN_OFFSET as i32),
            ],
        )
        .await?;
    let status = results.first().copied().and_then(Val::as_i32).unwrap_or(-1);

    if dynamic_input {
        let _ = pack_free(inst, in_ptr, input_bytes.len()).await;
    }

    let out_ptr = read_u32(inst, RESULT_PTR_OFFSET)? as usize;
    let out_len = read_u32(inst, RESULT_LEN_OFFSET)? as usize;

    if status != 0 {
        let mut err_bytes = vec![0u8; out_len];
        inst.read_memory(out_ptr, &mut err_bytes)?;
        let _ = pack_free(inst, out_ptr, out_len).await;
        return Err(CallError::Guest {
            name: name.to_string(),
            message: String::from_utf8_lossy(&err_bytes).into_owned(),
        });
    }

    let mut out_bytes = vec![0u8; out_len];
    inst.read_memory(out_ptr, &mut out_bytes)?;
    let value = decode(&out_bytes).map_err(|e| CallError::Abi(format!("{e:?}")))?;
    let _ = pack_free(inst, out_ptr, out_len).await;

    Ok(value)
}

/// Call the guest `__pack_alloc(size) -> ptr`. Errors (including a missing
/// export) let the caller fall back to the fixed input buffer.
async fn pack_alloc<I>(inst: &mut I, size: usize) -> Result<usize, EngineError>
where
    I: WasmInstance + ?Sized,
{
    let results = inst.call("__pack_alloc", &[Val::I32(size as i32)]).await?;
    let ptr = results.first().copied().and_then(Val::as_i32).unwrap_or(0);
    if ptr == 0 {
        return Err(EngineError::AllocFailed(size));
    }
    Ok(ptr as usize)
}

/// Call the guest `__pack_free(ptr, len)` if it is exported; a no-op otherwise.
async fn pack_free<I>(inst: &mut I, ptr: usize, len: usize) -> Result<(), EngineError>
where
    I: WasmInstance + ?Sized,
{
    if inst.has_export("__pack_free") {
        inst.call("__pack_free", &[Val::I32(ptr as i32), Val::I32(len as i32)])
            .await?;
    }
    Ok(())
}

/// Read a little-endian `u32` from guest memory at `offset`.
fn read_u32<I>(inst: &I, offset: usize) -> Result<u32, EngineError>
where
    I: WasmInstance + ?Sized,
{
    let mut bytes = [0u8; 4];
    inst.read_memory(offset, &mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}
