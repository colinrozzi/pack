# Composite Design Document

## Problem Statement

The WebAssembly Component Model provides a powerful foundation for building composable, sandboxed software components. However, its interface definition language (WIT) has a notable limitation: **no support for recursive types**.

This constraint stems from the Canonical ABI's design for shared-memory interop, where fixed-layout types enable efficient, zero-copy data sharing between components. For a recursive type like:

```wit
variant tree {
    leaf(string),
    node(list<tree>),  // ERROR: recursive reference
}
```

There's no fixed `sizeof(tree)` - the size depends on runtime data.

### Current Workarounds

1. **Resources (handles)**: Use opaque handles and accessor functions
   - Awkward API, lots of indirection
   - Works for shared-memory scenarios

2. **Serialization to bytes**: Manually serialize as `list<u8>`
   - Loses type safety
   - Each project reinvents encoding

3. **Flatten the structure**: Use indices into separate arrays
   - Awkward, error-prone
   - Loses the natural tree structure

### Observation

For message-passing architectures (like actor systems), data is serialized when crossing boundaries anyway. The fixed-layout constraint provides no benefit - we're already paying the serialization cost.

## Solution: WIT+ with Graph ABI

Composite defines a WIT+ dialect where recursion is allowed by default. All
values cross the boundary using a graph-encoded ABI:

```wit
variant sexpr {
    sym(string),
    num(s64),
    lst(list<sexpr>),
}
```

**Key insight**: The component boundary becomes a natural serialization point.
Components internally can represent recursive data however they want; only the
boundary encoding is specified.

## Design Principles

### 1. Unified ABI

WIT+ uses a single graph-encoded ABI for all values, regardless of whether types
are recursive. This removes any canonical ABI interop guarantees in exchange for
a consistent runtime model.

### 2. WIT+ Dialect

WIT+ is a new dialect with recursion allowed by default and a single ABI. It is
not wire-compatible with canonical ABI components, and interop requires
explicit adapters at the boundary.

### 3. Pluggable Execution

Composite doesn't reimplement WASM execution. It provides a component layer that works with existing WASM runtimes:

```rust
trait WasmExecutor {
    fn instantiate(&mut self, wasm: &[u8], imports: Imports) -> Instance;
    fn call(&mut self, instance: &Instance, func: &str, args: &[WasmValue]) -> Vec<WasmValue>;
    fn memory(&self, instance: &Instance) -> &[u8];
}
```

Initial implementation uses `wasmi` for simplicity; can swap to `wasmtime` for performance.

### 4. Symmetric ABI

All values use the same ABI regardless of direction:
- Host → Component: encode to graph buffer, write to memory, pass (ptr, len)
- Component → Host: component writes buffer, returns (ptr, len), host decodes

This symmetry simplifies the mental model and implementation.

## Architecture

### Layer 1: WIT+ Parser

Defines a WIT+ grammar with recursion allowed by default:

```
type-def ::= variant-def | record-def | enum-def | flags-def | alias-def
```

Mutual recursion is allowed by named references among type definitions.

**Name resolution and recursion rules**

- All type definitions in a file share a single namespace.
- Named references are resolved by that namespace, regardless of order.
- Cycles are permitted; the type graph may be cyclic.
- Undefined names are a hard error.

### Layer 2: Type System

```rust
enum TypeDef {
    // Standard WIT types
    Record(RecordDef),
    Variant(VariantDef),
    Enum(EnumDef),
    Flags(FlagsDef),
}

enum Type {
    // Primitives: Bool, U8, ..., String
    // Compounds: List, Option, Result, Tuple
    // References
    Named(String),
    SelfRef,  // Reference to enclosing type definition
}
```

Mutual recursion is represented by `Named` references that resolve to other
type definitions in the same namespace.

### Layer 3: ABI Encoding

**All types**: Use a schema-aware graph encoding
- Arena layout with node indices
- Validated against the WIT+ schema at the boundary
- Supports shared subtrees and cycles

### Layer 4: Component Linker

```rust
impl Runtime {
    fn instantiate(&mut self, component: &Component) -> Instance {
        // For each import:
        //   Bind with graph-encoded ABI wrappers

        // For each export:
        //   Register with appropriate ABI handling
    }
}
```

### Layer 5: Host Binding

```rust
// Binding a host function that takes recursive types
runtime.bind_import("my-interface", "process", |sexpr: SExpr| {
    // Composite handles serialization/deserialization
    transform(sexpr)
});
```

## Recursive ABI: Graph-Encoded Arena

Recursive values are encoded into a self-contained arena that supports shared
subtrees and cycles. The ABI payload is a single contiguous byte buffer passed
as (ptr, len). This keeps v1 copy-friendly while enabling future zero/low-copy
"view" decoding.

### Serialization Format vs Tagged Encoding

WIT+ uses a schema-aware graph encoding. The type schema is known at the
boundary, so values do not carry per-value type tags or field names. This keeps
the format compact and makes validation a schema-driven process.

In contrast, a tagged/self-describing format would embed type tags with every
value. That is not the chosen design for WIT+.

### Buffer Layout (Little Endian)

```
u32 magic = 'CGRF'
u16 version = 1
u16 flags

u32 node_count
u32 root_index

Node[node_count]
```

Each node has a fixed header followed by a variable payload:

```
u8  kind
u8  flags
u16 reserved
u32 payload_len
<payload bytes>
```

### Node Kinds (v1)

```
tag     type        encoding
----    ----        --------
0x01    bool        u8 (0 or 1)
0x02    s32         i32 little-endian
0x03    s64         i64 little-endian
0x04    f32         f32 little-endian
0x05    f64         f64 little-endian
0x06    string      u32 length + utf8 bytes
0x07    list        u32 count + u32[count] child_indices
0x08    variant     u32 case_tag + u8 has_payload + [u32 child_index]
0x09    record      u32 field_count + u32[field_count] child_indices
0x0A    option      u8 has_value + [u32 child_index]
0x0B    tuple       u32 arity + u32[arity] child_indices
0x0C    u8          u8
0x0D    u16         u16 little-endian
0x0E    u32         u32 little-endian
0x0F    u64         u64 little-endian
0x10    s8          i8
0x11    s16         i16 little-endian
0x12    char        u32 Unicode scalar
0x13    flags       u64 bitmask (0..63)
```

### Validation Rules

- All child indices must be < node_count.
- payload_len must match the actual payload size.
- Values are validated against the WIT+ schema at the boundary:
  - If a field type is `list<sexpr>`, the node must be `list` and each child
    must validate as `sexpr`.
  - Variant case tags must be in range and payload presence must match the case.

### Mapping Recursive Types

Example:

```wit
variant sexpr {
    sym(string),
    num(s64),
    lst(list<sexpr>),
}
```

- `sexpr` values are encoded as `variant` nodes.
- `sym` references a `string` node.
- `num` references an `s64` node.
- `lst` references a `list` node whose children reference `sexpr` nodes.

This encoding permits shared subtrees and cycles by referencing existing node
indices.

## Open Questions

### ABI Considerations

These should be specified alongside the graph encoding:

- Type-checking algorithm: validation rules for nodes vs WIT+ schema.
- Error model: how decode/validation errors are surfaced to host/component.
- Limits: maximum node count, max string/list sizes, recursion depth, total buffer size.
- Determinism: canonicalization rules (e.g., record field order).
- Memory ownership: who allocates/frees recursive buffers.
- Versioning: magic/version/flags evolution strategy.
- Security: DoS protections for large or cyclic graphs.

### Type-Checking Algorithm (Sketch)

At the recursive ABI boundary, validate the graph buffer against the expected
WIT+ type.

1. Parse header, bounds-check counts, and build a table of node headers and
   payload slices (without interpreting payloads yet).
2. Validate the root node against the expected type using a DFS with memoization
   on (node_index, expected_type).
3. For each node:
   - Ensure the node kind matches the expected type.
   - For list/tuple/record/variant/option, validate child indices are in range.
   - Recursively validate each child against its expected type.
4. If a node is reached again with a different expected type, fail with a type
   mismatch error.
5. Enforce payload_len and primitive constraints (e.g., UTF-8 strings).

This algorithm allows cycles by tracking visited (node_index, expected_type)
pairs and short-circuiting repeats.

### Error Model (Sketch)

Errors are structural (malformed buffer) or semantic (type mismatch):

- MalformedBuffer: invalid magic/version, out-of-bounds indices, payload_len
  mismatch, invalid UTF-8, truncated payload.
- TypeMismatch: node kind does not match expected WIT+ type, variant tag out of
  range, option/variant payload presence mismatch.
- LimitExceeded: buffer size, node count, recursion depth, or string/list size
  limits exceeded.

Errors should include a stable code and optional context (node index, expected
type, actual kind). The ABI should not expose internal pointers or host-specific
details.

### Limits (Sketch)

Defaults should be conservative and configurable by the runtime:

- Max buffer size: 16 MiB
- Max node count: 1,000,000
- Max string size: 8 MiB
- Max list/tuple/record arity: 1,000,000
- Max recursion depth during validation: 10,000

Any limit violation yields LimitExceeded.

### Determinism (Sketch)

- Record fields are serialized in WIT declaration order.
- Variant case tags use WIT declaration order (0-based).
- Tuple ordering matches the WIT tuple order.

This keeps encoding deterministic and stable across toolchains.

### Memory Ownership (Sketch)

Values cross the boundary as (ptr, len) buffers:

- Host -> Component: host allocates buffer in component memory (via allocator
  import), passes (ptr, len), then frees after call returns.
- Component -> Host: component allocates buffer in its memory, returns (ptr,
  len), and exposes a `free` export for host to release.

This mirrors the established component string/list ownership pattern.

### Versioning (Sketch)

- The header includes magic + version + flags.
- Minor, backward-compatible extensions set feature flags.
- Major changes bump the version and require explicit opt-in.

### Security (Sketch)

- Strict bounds checking on all indices and payload lengths.
- Enforce limits to avoid memory blowups or pathological cycles.
- Treat invalid UTF-8 or malformed payloads as MalformedBuffer.

### 1. Schema Evolution

How do we handle changes to recursive types over time?
- Add new variant cases?
- Deprecate old ones?
- Versioning?

### 2. Streaming

For very large trees, full serialization may be expensive. Could support:
- Lazy serialization
- Chunked encoding
- Reference to already-serialized subtrees

### 3. Performance

Serialization overhead vs fixed-layout ABI:
- Measure actual overhead
- Consider binary formats (MessagePack, CBOR) if JSON-style is too slow
- JIT-generated serializers?

## Implementation Roadmap

### Phase 1: Foundation ✓
- [x] WIT+ parser (recursive and mutually recursive types)
- [x] Type system with recursive type support
- [x] Graph-encoded ABI (CGRF format)
- [x] Schema-aware encoding/decoding with validation

### Phase 2: Runtime ✓
- [x] Wasmi integration (load, instantiate, call)
- [x] Memory read/write for data passing
- [x] Graph ABI integration (`write_value`, `read_value`, `call_with_value`)
- [x] Host function binding (`host.log`, `host.alloc`)
- [x] Component instantiation with imports

### Phase 3: Components ✓
- [x] Shared `composite-abi` crate (no_std compatible)
- [x] Rust component examples (echo, logger)
- [x] Components calling host imports

### Phase 4: In Progress
- [ ] Component-to-component linking
- [ ] More host functions (file I/O, networking)
- [ ] Resource types (handles for host objects)
- [ ] Async/streaming for large values
- [ ] Performance optimization

### Phase 5: Ecosystem
- [ ] Bindgen for Rust (derive macros)
- [ ] Bindgen for other languages
- [ ] Integration with Theater
- [ ] Integration with Wisp

## MVP Definition (Draft)

### Target Use Case

Round-trip a minimal recursive `node` value across the component boundary using
the graph-encoded ABI.

Example WIT+ type:

```wit
variant node {
    leaf(s64),
    list(list<node>),
}
```

Mutual recursion example:

```wit
variant expr {
    literal(lit),
    add(expr, expr),
}

variant lit {
    number(f64),
    quoted(expr),
}
```

### Required Capabilities

- Parse WIT+ with recursive and mutually recursive type definitions.
- Encode/decode recursive values to/from the graph buffer.
- Instantiate a component in `wasmi`.
- Host calls an exported function taking a recursive type and receives a
  recursive return.
- Component calls a host import taking a recursive type and receives a
  recursive return.

### Acceptance Tests

1. **Parse**: A WIT+ file with recursive and mutually recursive types parses
   without error.
2. **Round-trip encode/decode**: Encoding then decoding a deeply nested `node`
   yields structural equality.
3. **Component -> Host**: A component returns a transformed `node` (e.g.,
   wraps a leaf in a list) and the host decodes it correctly.
4. **Host -> Component**: Host passes a recursive `node` to a component
   function and gets the expected response.
5. **Validation**: Malformed buffers and type mismatches are rejected with
   stable error codes.

## References

- [Component Model Explainer](https://github.com/WebAssembly/component-model/blob/main/design/mvp/Explainer.md)
- [Canonical ABI](https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md)
- [WIT Specification](https://github.com/WebAssembly/component-model/blob/main/design/mvp/WIT.md)
