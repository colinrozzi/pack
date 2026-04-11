# Pack Bug: "Buffer too large" when decoding supervisor child events

## Problem

When a supervisor actor spawns a child that shuts down, theater calls
`handle-child-event` on the parent. The event data (a serialized chain event)
gets encoded as a `Value::List { elem_type: U8, items: [...] }` — one
`Value::U8` node per byte. For even moderately-sized event payloads, the
graph-encoded representation of this list explodes well beyond the 16MB
`max_buffer_size` default in pack's `Limits`.

The decode fails at:

```
pack/crates/pack-abi/src/lib.rs:308-309
    if bytes.len() > limits.max_buffer_size {
        return Err(AbiError::InvalidEncoding(String::from("Buffer too large")));
    }
```

This causes the actor to crash even though the comment in theater's supervisor
handler says this callback is optional and failures should only be logged at
debug level.

## Reproduction

1. Build and run the `subway` actor (`theater start subway/acceptor/manifest.toml`)
2. Make an HTTP request (`curl http://localhost:8080/`)
3. The handler serves the page, then calls `shutdown(None)`
4. The supervisor tries to deliver the child event to the acceptor
5. Pack fails to decode the params → `SerializationError` → actor crashes

## Root Cause

The encoding blowup: a `List<U8>` of N bytes becomes N individual graph nodes,
each with overhead. The chain event data for even a simple handler lifecycle can
be large enough that the graph-encoded representation exceeds 16MB.

**Two issues here:**

### 1. Pack: `List<U8>` encoding is pathologically inefficient

Each byte in a `List<U8>` becomes a separate graph node. A 100KB byte payload
produces a graph buffer that's orders of magnitude larger. Byte lists should
have a compact encoding (e.g., a single blob node) rather than one node per
element.

Relevant code:
- `pack/crates/pack-abi/src/lib.rs` — `Limits::default()` sets `max_buffer_size: 16MB`
- `pack/src/abi/mod.rs` — same limits definition (duplicated)

### 2. Theater: child-event error is not gracefully handled

The supervisor handler (`theater-handler-supervisor/src/lib.rs:249-265`) says
errors should be logged at debug level, but the actual error propagation crashes
the parent actor. The `call_function` path goes through `runtime.rs:1009-1012`
where the decode failure becomes `ActorError::SerializationError`, which is
fatal.

## Suggested Fix

**In pack:** Add a compact encoding for `List<U8>` (and potentially other
primitive lists). Instead of N graph nodes, encode as a single payload blob with
a type tag. This is the real fix — byte lists are extremely common in theater
(state, messages, event data).

**In theater (workaround):** Make the `handle-child-event` call truly optional —
catch the serialization error before it becomes fatal, matching the intent of
the existing comment.
