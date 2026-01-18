# RFC: Memory Allocation Strategy for Host Function Returns

## Context

**Composite** is a WebAssembly runtime designed to replace wasmtime's Component Model inside **Theater**. The key motivations are:

1. **Recursive data types**: Theater's message-passing system uses recursive types (S-expressions, ASTs, message trees). The Component Model can't represent these natively, forcing serialization workarounds.

2. **Barrier control**: Theater needs to capture and audit everything that crosses the WASM boundary. The Component Model abstracts this away, making enforcement difficult.

3. **Owned tooling**: Full control over the stack enables better tooling, integration, and developer experience.

Composite uses **Graph ABI** for encoding values (including recursive structures) and **WIT+** for type definitions that allow recursion.

## The Problem

When a host function needs to return data to a guest, it must write that data somewhere in the guest's linear memory. Currently, Composite uses a **fixed offset** (16KB):

```rust
// Current implementation in func_typed, func_async, etc.
let bytes = encode(&output_value);
let out_ptr = 16 * 1024;  // Always writes to 16KB!
memory.write(out_ptr, &bytes);
return (out_ptr, bytes.len());
```

### Why This Is Problematic

1. **Nested calls**: If host function A triggers a call back into WASM, which calls host function B, both write to 16KB - B's output overwrites A's.

2. **Size collisions**: Output buffer (16KB-48KB) could collide with guest heap if outputs are large.

3. **Concurrent async**: Multiple async host functions could race to write to the same location.

4. **No coordination**: Guest has no control over where data lands in its memory.

## Theater's Usage Pattern

Theater actors follow a message-handling pattern:

```
┌─────────────┐                      ┌─────────────┐
│   Runtime   │                      │    Actor    │
└──────┬──────┘                      └──────┬──────┘
       │                                    │
       │  "here's a message" (input)        │
       │───────────────────────────────────>│
       │                                    │
       │           actor processes...       │
       │                                    │
       │  "here's my response" (output)     │
       │<───────────────────────────────────│
       │                                    │
       │  runtime captures response,        │
       │  moves to next message             │
       │                                    │
```

Key characteristics:
- Request/response pattern
- Responses are immediately consumed by the runtime
- Actors don't need to retain response data after returning
- Runtime controls both sides (can define conventions)

## Options

### Option A: Guest Exports Realloc (Component Model Style)

The guest exports a `cabi_realloc` function. When the host needs to return data, it calls back into the guest to allocate memory.

```
┌──────────┐                           ┌──────────┐
│   Host   │                           │  Guest   │
└────┬─────┘                           └────┬─────┘
     │                                      │
     │  (computing result...)               │
     │                                      │
     │  cabi_realloc(0, 0, 8, 1024)        │
     │─────────────────────────────────────>│
     │                                      │
     │  returns ptr=65536                   │
     │<─────────────────────────────────────│
     │                                      │
     │  writes 1024 bytes @ 65536           │
     │                                      │
     │  returns (ptr=65536, len=1024)       │
     │─────────────────────────────────────>│
```

**Implementation:**
```rust
impl Ctx<'_, T> {
    fn allocate_in_guest(&mut self, size: usize) -> Result<i32, Error> {
        let realloc = self.get_export("cabi_realloc")?;
        realloc.call((0, 0, 8, size as i32))
    }

    fn write_value(&mut self, value: &Value) -> Result<(i32, i32), Error> {
        let bytes = encode(value)?;
        let ptr = self.allocate_in_guest(bytes.len())?;
        self.memory().write(ptr, &bytes)?;
        Ok((ptr, bytes.len() as i32))
    }
}
```

**Guest must export:**
```rust
#[no_mangle]
pub extern "C" fn cabi_realloc(
    old_ptr: i32, old_size: i32, align: i32, new_size: i32
) -> i32 {
    // Standard allocator implementation
}
```

**Pros:**
- Proven pattern (used by Component Model)
- Guest controls its own memory
- Handles arbitrary return sizes
- Clean ownership semantics
- Could leverage existing `wit-bindgen` patterns

**Cons:**
- Every actor needs allocator boilerplate (until tooling automates it)
- Function call overhead for every return
- What if guest allocator fails mid-call?
- More complex than necessary for Theater's use case?

---

### Option B: Caller Provides Output Buffer

The runtime provides both input AND output buffers. The guest writes its response to the provided location.

```
┌──────────┐                           ┌──────────┐
│ Runtime  │                           │  Actor   │
└────┬─────┘                           └────┬─────┘
     │                                      │
     │  call(in_ptr, in_len,                │
     │       out_ptr, out_cap)              │
     │─────────────────────────────────────>│
     │                                      │
     │  actor writes response to out_ptr    │
     │                                      │
     │  returns out_len                     │
     │<─────────────────────────────────────│
     │                                      │
     │  runtime reads [out_ptr..out_len]    │
```

**Calling convention:**
```rust
// Current: (in_ptr, in_len) -> (out_ptr, out_len) packed as i64
// New:     (in_ptr, in_len, out_ptr, out_cap) -> out_len
```

**Implementation:**
```rust
impl Instance<T> {
    fn call_with_value(&mut self, name: &str, input: &Value) -> Result<Value, Error> {
        // Write input
        let in_len = self.write_value(INPUT_OFFSET, input)?;

        // Provide output buffer (runtime-managed region)
        let out_ptr = OUTPUT_BUFFER_OFFSET;
        let out_cap = OUTPUT_BUFFER_SIZE;  // e.g., 1MB

        let out_len = self.call_func(name, (
            INPUT_OFFSET, in_len,
            out_ptr, out_cap
        ))?;

        self.read_value(out_ptr, out_len)
    }
}
```

**Pros:**
- No guest allocator needed at all
- Zero callback overhead
- Runtime has full control over memory
- Simpler actors (no boilerplate)
- Perfect fit for request/response pattern
- Easy to capture/audit all data crossing boundary

**Cons:**
- What if response exceeds buffer capacity?
  - Option: Return error, runtime retries with larger buffer
  - Option: Two-phase (first call returns size, second call writes)
- Changes calling convention (breaking change)
- Less flexible than guest-controlled allocation

---

### Option C: Runtime-Managed Buffer Pool

The runtime maintains a pool of buffers, assigning one per call context.

```rust
struct CallContext {
    id: u64,
    output_buffer: Vec<u8>,
    output_offset: usize,  // Location in linear memory
}

impl Runtime {
    fn begin_call(&mut self) -> CallContext {
        // Allocate or reuse a buffer
        // Map it into guest memory at a unique offset
    }

    fn end_call(&mut self, ctx: CallContext) {
        // Return buffer to pool
    }
}
```

**Pros:**
- Handles nested calls (each gets own buffer)
- No guest boilerplate
- Efficient buffer reuse

**Cons:**
- Complex runtime implementation
- Memory mapping complexity
- Buffer lifecycle management

---

### Option D: Hybrid Approach

Use caller-provides-buffer as default, with fallback to guest realloc for oversized responses.

```rust
fn write_value(&mut self, value: &Value) -> Result<(i32, i32), Error> {
    let bytes = encode(value)?;

    if bytes.len() <= INLINE_BUFFER_SIZE {
        // Fast path: use pre-allocated buffer
        self.memory().write(INLINE_BUFFER_OFFSET, &bytes)?;
        Ok((INLINE_BUFFER_OFFSET, bytes.len()))
    } else {
        // Slow path: ask guest to allocate
        let ptr = self.allocate_in_guest(bytes.len())?;
        self.memory().write(ptr, &bytes)?;
        Ok((ptr, bytes.len()))
    }
}
```

**Pros:**
- Fast path for common case (small responses)
- Handles large responses gracefully
- Flexible

**Cons:**
- Two code paths to maintain
- Guest still needs realloc export (for large responses)

---

## Questions for Discussion

1. **How large are typical Theater messages?** If 99% fit in 1MB, Option B might be sufficient with a simple error for oversized responses.

2. **Are nested host calls common?** If actor A calls host, which calls back to actor A, which calls host again - how deep can this go?

3. **Is the calling convention change acceptable?** Option B changes from `(in_ptr, in_len) -> i64` to `(in_ptr, in_len, out_ptr, out_cap) -> i32`.

4. **How important is zero guest boilerplate?** Option A requires every actor to export `cabi_realloc`. Is that acceptable, or is simplicity paramount?

5. **Should Composite be Theater-specific or general?** A general-purpose runtime might favor Option A (Component Model style). A Theater-specific runtime might favor Option B (simpler, fits the use case).

## Decision: Option B Implemented

After gathering feedback, **Option B (caller provides output buffer)** was chosen and implemented. The new calling convention is:

```
Old: (in_ptr, in_len) -> i64  // packed (out_ptr, out_len)
New: (in_ptr, in_len, out_ptr, out_cap) -> i32  // returns out_len, or -1 on error
```

Key benefits:
- Zero boilerplate for actors
- Runtime controls the barrier completely
- Fits the consumption pattern (responses are immediately read by runtime)
- No guest allocator needed
- Clean error handling (-1 indicates failure)

---

## Appendix: Memory Layout (Implemented)

```
WASM Linear Memory:
┌─────────────────────────────────────────────┐
│ 0-16KB      Default input buffer            │
├─────────────────────────────────────────────┤
│ 16-48KB     Default output buffer (32KB)    │ ← Now caller-provided
├─────────────────────────────────────────────┤
│ 48KB+       Bump allocator (host.alloc)     │
└─────────────────────────────────────────────┘

Constants defined in src/runtime/host.rs:
- INPUT_BUFFER_OFFSET = 0
- OUTPUT_BUFFER_OFFSET = 16 * 1024
- OUTPUT_BUFFER_CAPACITY = 32 * 1024
```

## Appendix: Relevant Code

- `src/runtime/host.rs` - `Ctx::write_value()`, `func_typed()`, `func_async()`
- `src/runtime/mod.rs` - `Instance::call_with_value()`
- Fixed offset: `let out_ptr = 16 * 1024;`
