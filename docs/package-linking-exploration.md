# Package Linking Design Exploration

This document captures a design discussion about implementing package-to-package linking in Composite.

## Current State

Packages can only import from the host - there's no way for Package A to call Package B directly. Package linking is listed as a Phase 4 TODO in the design roadmap.

## Design Decisions (Tentative)

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Instantiation | Eager (all at once) | Simpler when bundling packages together |
| Dependencies | DAG only, no cycles | Avoids chicken-and-egg instantiation problems |
| Versioning | Semver | Standard approach for interface compatibility |
| Performance | Defer optimization | Get it working first |

## Core Implementation Requirements

### 1. Package Registry
Track loaded packages and their exports:
```rust
struct PackageRegistry {
    packages: HashMap<PackageId, RegisteredPackage>,
}

struct RegisteredPackage {
    instance: Instance<T>,
    exports: Vec<Export>,
    imports: Vec<Import>,
}
```

### 2. Dependency Resolver
- Topological sort for instantiation order
- Error on cycles: "Circular dependency detected: A -> B -> A"

### 3. Cross-Memory Bridge
Each WASM package has isolated linear memory. Passing data between packages requires:
- Reading bytes from source package's memory
- Writing bytes to target package's memory

### 4. Linker Integration
Extend `HostLinkerBuilder` to wire package exports as other packages' imports.

---

## Buffer Architecture Discussion

### Current: Buffers in WASM Linear Memory

```
Each package's memory layout:
+-------------------------------------+
| 0KB      Input Buffer (16KB)        |  <- host writes input here
+------------------------------------|
| 16KB     Output Buffer (32KB)       |  <- component writes output here
+-------------------------------------+
| 48KB     Heap (bump allocator)      |  <- package's working memory
+-------------------------------------+
```

Flow:
1. Host encodes Value to bytes
2. Host writes bytes into package's memory at offset 0
3. Package reads, processes, writes result at offset 16KB
4. Host reads bytes from package's memory

### Alternative: Host-Managed Buffers

Instead of each package owning I/O buffers, the host could manage a buffer pool:

```
Host Process Memory:
+------------------------------+
|  Buffer Pool                 |
|  +--------+ +--------+       |
|  | Buf 0  | | Buf 1  |  ...  |
|  +--------+ +--------+       |
+------------------------------+
```

Packages interact via host calls:
```
host::read_input(buf_handle, offset, len) -> bytes
host::write_output(buf_handle, bytes)
```

**Advantage**: For package linking, pass the same buffer handle from A's output to B's input - no copy.

**Tradeoff**: Packages can't do direct memory access; must call host for every buffer read/write.

### Alternative: Callee-Returns-Pointer

Current signature:
```
(in_ptr, in_len, out_ptr, out_cap) -> out_len
// Caller provides output buffer
```

Alternative:
```
(in_ptr, in_len) -> (out_ptr, out_len)
// Callee allocates output wherever, returns pointer
```

**Advantages**:
- No fixed buffer size limit
- Package controls its own memory layout
- Could return pointer to existing data (no copy)

**Questions**:
- Memory ownership: who frees the output?
- Options: bump allocator + reset, explicit free call, arena per call

---

## Zero-Copy: Wire Format as In-Memory Format

### The Idea

If the Graph ABI wire format was also the in-memory representation, there's no serialization step. Package just returns a pointer to where the data already lives.

### Current: Two Representations

**In-memory (Rust's Value enum):**
```rust
Value::List(Vec<Value>)  // Vec uses pointers
Value::Record(Vec<(String, Value)>)  // scattered heap allocations
```

**Wire format (Graph ABI / CGRF):**
```
[MAGIC][VERSION][node_count][root_index]
[Node0: kind + payload_len + payload]
[Node1: kind + payload_len + payload]
...
```
Flat, contiguous bytes. Children referenced by index, not pointers.

### Trade-offs

| Aspect | Native Rust | Wire Format In-Memory |
|--------|-------------|----------------------|
| Tree traversal | Follow pointer | Base + (index * stride) |
| Add a child | `vec.push()` | Rebuild buffer or use arena |
| Mutation | Direct write | Complex (may shift offsets) |
| Memory layout | Rust decides | You control every byte |
| Derive macros | `#[derive(GraphValue)]` | Need custom builders |

### When Wire-Format-In-Memory Works Well

- Build data once, return it (functional style)
- Light transformations
- Pass-through packages

### When It's Constraining

- Mutate trees in place
- Heavy recursive algorithms with intermediate structures
- Need Rust's pattern matching

---

## Open Questions

1. **Buffer ownership**: Host-managed vs WASM-owned vs hybrid?
2. **Output convention**: Caller-provides-buffer vs callee-returns-pointer?
3. **Zero-copy potential**: Worth constraining in-memory format for transfer efficiency?
4. **Call patterns**: Occasional orchestration (copy overhead OK) vs tight loops (need zero-copy)?

## Next Steps

1. Start with simplest approach: current buffer model, accept copies
2. Implement package registry and dependency resolution
3. Build cross-memory bridge
4. Measure performance, optimize if needed

---

*This document is exploratory. Decisions are tentative and subject to change based on implementation experience.*
