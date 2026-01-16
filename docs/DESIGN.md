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

## Solution: WIT+ with Serialization ABI

Composite extends WIT with a `rec` keyword for recursive types. These types use a serialization-based ABI instead of fixed-layout:

```wit
rec variant sexpr {
    sym(string),
    num(s64),
    lst(list<sexpr>),
}
```

**Key insight**: The component boundary becomes a natural serialization point. Components internally can represent recursive data however they want; only the boundary encoding is specified.

## Design Principles

### 1. Superset Compatibility

Any valid WIT file is a valid WIT+ file. Standard types behave identically.

```
WIT ⊂ WIT+
```

This means:
- Existing tooling partially works (parsing standard portions)
- Migration path: start with standard WIT, add recursive types as needed
- Components not using recursive types work everywhere

### 2. Explicit Recursion

Recursive types must be explicitly marked with `rec`:

```wit
// This fails - recursion not allowed in standard variant
variant bad {
    node(list<bad>),  // ERROR
}

// This works - explicitly recursive
rec variant good {
    node(list<good>),  // OK
}
```

This makes the ABI implications clear: `rec` types use serialization, others use canonical ABI.

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

The serialization format is the same regardless of direction:
- Host → Component: serialize, write to memory, pass (ptr, len)
- Component → Host: component writes to memory, returns (ptr, len), host deserializes

This symmetry simplifies the mental model and implementation.

## Architecture

### Layer 1: WIT+ Parser

Extends WIT grammar with:

```
type-def ::= ... | rec-type-def

rec-type-def ::= 'rec' 'variant' id '{' variant-cases '}'
              | 'rec' 'group' '{' type-def* '}'
```

The `rec group` form handles mutually recursive types.

### Layer 2: Type System

```rust
enum TypeDef {
    // Standard WIT types
    Record(RecordDef),
    Variant(VariantDef),
    Enum(EnumDef),
    Flags(FlagsDef),

    // Extended
    Recursive(RecursiveDef),
}

enum Type {
    // Primitives: Bool, U8, ..., String
    // Compounds: List, Option, Result, Tuple
    // References
    Named(String),
    SelfRef,  // Reference to enclosing rec type
}
```

### Layer 3: ABI Encoding

**Standard types**: Use canonical ABI (or close approximation)
- Fixed-size types inline
- Strings/lists as (ptr, len)

**Recursive types**: Use tagged binary encoding
- Self-describing format
- Length-prefixed for nested structures
- Efficient for tree-structured data

### Layer 4: Component Linker

```rust
impl Runtime {
    fn instantiate(&mut self, component: &Component) -> Instance {
        // For each import:
        //   If standard type: bind with canonical ABI
        //   If recursive type: bind with serialization wrapper

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

## Serialization Format

Tagged, self-describing binary format:

```
tag     type        encoding
----    ----        --------
0x00    bool        u8 (0 or 1)
0x01    s32         i32 little-endian
0x02    s64         i64 little-endian
0x03    f32         f32 little-endian
0x04    f64         f64 little-endian
0x05    string      u32 length + utf8 bytes
0x06    list        u32 count + elements
0x07    variant     u32 tag + u8 has_payload + [payload]
0x08    option      u8 has_value + [value]
0x09    record      u32 field_count + (name + value)*
0x0A    u8          u8
0x0B    u16         u16 little-endian
0x0C    u32         u32 little-endian
0x0D    u64         u64 little-endian
```

This format is:
- Self-describing (no schema needed to parse)
- Reasonably compact
- Easy to implement in any language
- Supports arbitrary nesting

## Open Questions

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

### 3. Canonical ABI Interop

Can we compose with standard components that don't know about recursive types?
- Probably: just don't expose recursive types in the shared interface
- May need explicit "flatten" operations

### 4. Performance

Serialization overhead vs fixed-layout ABI:
- Measure actual overhead
- Consider binary formats (MessagePack, CBOR) if JSON-style is too slow
- JIT-generated serializers?

## Implementation Roadmap

### Phase 1: Foundation
- [ ] WIT+ parser (extend existing WIT parser or write new)
- [ ] Type system with recursive type support
- [ ] Basic serialization codec

### Phase 2: Runtime
- [ ] Wasmi integration
- [ ] Host function binding
- [ ] Component instantiation

### Phase 3: Polish
- [ ] Error messages and diagnostics
- [ ] Performance optimization
- [ ] Documentation and examples

### Phase 4: Ecosystem
- [ ] Bindgen for Rust
- [ ] Bindgen for other languages?
- [ ] Integration with Theater
- [ ] Integration with Wisp

## References

- [Component Model Explainer](https://github.com/WebAssembly/component-model/blob/main/design/mvp/Explainer.md)
- [Canonical ABI](https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md)
- [WIT Specification](https://github.com/WebAssembly/component-model/blob/main/design/mvp/WIT.md)
