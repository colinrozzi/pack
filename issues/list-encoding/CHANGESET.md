# Changeset: Array node kind for compact primitive list encoding

Resolves the "Buffer too large" crash described in `PACK_BUG.md`.

## What changed

Pack's CGRF graph format now has a new node kind, **`Array`** (`0x15`), for
encoding lists of fixed-size primitive types. Instead of allocating one graph
node per element (the old `List` behavior), `Array` stores all element data
contiguously in a single node's payload.

### Wire format

```
Array node payload: [elem_type:u8, count:u32, data:u8*]
```

- `elem_type` — a single type tag byte (same tags used elsewhere in CGRF v2)
- `count` — number of elements, little-endian u32
- `data` — `count * width` bytes of element data, all little-endian

### Supported element types

| Type   | Tag    | Width |
|--------|--------|-------|
| Bool   | `0x01` | 1     |
| U8     | `0x0C` | 1     |
| S8     | `0x10` | 1     |
| U16    | `0x0D` | 2     |
| S16    | `0x11` | 2     |
| U32    | `0x0E` | 4     |
| S32    | `0x02` | 4     |
| F32    | `0x04` | 4     |
| Char   | `0x12` | 4     |
| U64    | `0x0F` | 8     |
| S64    | `0x03` | 8     |
| F64    | `0x05` | 8     |
| Flags  | `0x13` | 8     |

### Encoding rules

Primitive element types **must** use `Array`. Compound element types **must**
use `List`. The encoder enforces this:

- `elem_type` is fixed-width primitive AND all items match → `Array` node
- `elem_type` is compound (String, Record, Variant, etc.) → `List` node
- `elem_type` is primitive but items don't match → **encode error**

That last case (e.g. `elem_type: S64` with a Variant item in the list) was
silently accepted before. It's now rejected — the `elem_type` must be honest.

### Decoding rules

The decoder enforces the same split:

- `Array` node → reads contiguous primitive data
- `List` node with primitive `elem_type` → **decode error**
- `List` node with compound `elem_type` → reads child node indices as before

## Impact on the original bug

A `List<U8>` of 100,000 bytes now encodes as a single `Array` node with
~100KB of payload, instead of 100,001 nodes that blew past the 16MB
`max_buffer_size`. The supervisor child-event payloads in theater should
encode and decode without hitting size limits.

## What theater still needs to do

This fix resolves the encoding blowup, but the bug report identified a second
issue: `handle-child-event` errors are fatal when they should be non-fatal.

Specifically, in the supervisor handler (`theater-handler-supervisor/src/lib.rs`,
around lines 249-265), the comment says errors should be logged at debug level,
but the actual error propagation path through `runtime.rs` (around lines
1009-1012) turns decode failures into `ActorError::SerializationError`, which
crashes the parent actor.

Recommended theater-side change: catch serialization errors from the
`handle-child-event` call path before they reach the fatal error handler, and
log them at debug level instead. This provides defense-in-depth — even if a
future encoding issue arises, the parent actor won't crash on an optional
callback.

## Files changed in pack

- `crates/pack-abi/src/lib.rs` — `Array` variant on `NodeKind`, `fixed_width()`,
  `encode_array_element()`, `decode_array_element()`, encode/decode/validation
- `src/abi/mod.rs` — same changes (duplicate ABI implementation)
- `src/parser/validation.rs` — accept `Array` nodes when validating primitive
  list types
- `tests/abi_roundtrip.rs` — new tests for Array encoding across types
- All Wasm guest packages rebuilt against updated `pack-abi`

## Updating theater

Theater needs to bump its `pack-abi` dependency to pick up the new `Array` node
kind. Any Wasm guest packages should be rebuilt against the updated `pack-abi`.
