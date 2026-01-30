# Embedded Type Metadata

## Status: Proposed

## Summary

Pack packages should embed full type signature metadata for all their imports and exports. A standard export function `__pack_types` provides access to this metadata, which is stored as a static CGRF-encoded blob in a data segment.

## Motivation

Currently, loading a Pack package gives you a set of exported functions with the flat signature `(i32, i32, i32, i32) -> i32`. There is no way to discover what logical types those functions expect or return without external knowledge.

This means consumers (REPLs, linkers, dev tools) must either:
- Hardcode or guess function signatures
- Require out-of-band type information (separate files, documentation)

A self-describing package solves this. Tools can inspect any package to discover its full interface - what it provides, what it requires, and the types involved.

## Design

### Embedded Data

At compile time, the compiler serializes a CGRF blob describing all import and export signatures into a WASM data segment. This is static, read-only metadata baked into the binary.

### Access Function

```
(export "__pack_types")
fn __pack_types(out_ptr_ptr: i32, out_len_ptr: i32) -> i32
```

Returns 0 on success. Writes pointer and length of the metadata blob to the provided slots (following the standard guest-allocates ABI convention). Since the data is in a static segment, no allocation is needed - it just returns a pointer into the data segment.

### Metadata Format

The metadata is a CGRF-encoded value describing all imports and exports with full type signatures. Proposed structure:

```
record package-metadata {
  imports: list<function-sig>,
  exports: list<function-sig>,
}

record function-sig {
  interface: string,       // e.g., "example:math/ops"
  name: string,            // e.g., "add"
  params: list<param-sig>,
  results: list<type-desc>,
}

record param-sig {
  name: string,
  type: type-desc,
}

variant type-desc {
  s32,
  s64,
  f32,
  f64,
  string,
  bool,
  list(type-desc),
  option(type-desc),
  result { ok: type-desc, err: type-desc },
  record { name: string, fields: list<field-desc> },
  variant { name: string, cases: list<case-desc> },
  tuple(list<type-desc>),
}

record field-desc {
  name: string,
  type: type-desc,
}

record case-desc {
  name: string,
  payload: option<type-desc>,
}
```

### Example

A package exporting `add(a: s32, b: s32) -> s32` and importing `log(msg: string)` would have metadata like:

```
{
  imports: [
    { interface: "runtime", name: "log", params: [{ name: "msg", type: string }], results: [] }
  ],
  exports: [
    { interface: "math", name: "add", params: [{ name: "a", type: s32 }, { name: "b", type: s32 }], results: [s32] }
  ]
}
```

## Use Cases

- **Wisp REPL**: Generate correct CGRF wrappers for imported Pack functions based on discovered types
- **Pack linker**: Validate that a package's imports are satisfied by another package's exports
- **Dev tools**: Inspect packages, generate documentation, IDE support
- **Runtime validation**: Verify arguments match expected types before calling

## Implementation Plan

- [ ] Define the metadata CGRF schema
- [ ] Add metadata generation to pack-guest macros (derive from `#[export]` and `#[import]` attributes)
- [ ] Generate `__pack_types` export in pack-guest
- [ ] Add `Pack::Instance::types()` method that calls `__pack_types` and returns parsed metadata
- [ ] Add metadata generation to Wisp compiler's Pack codegen
- [ ] Update Wisp REPL to use discovered types for wrapper generation
