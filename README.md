# Composite

A WebAssembly component runtime with extended WIT support for recursive data types.

## Motivation

The WebAssembly Component Model's WIT interface definition language doesn't support recursive types. This is a reasonable constraint for shared-memory scenarios where fixed-layout ABIs are desirable, but it's limiting for use cases involving tree-structured data:

- Abstract Syntax Trees (ASTs)
- S-expressions
- JSON/DOM-like structures
- File system trees
- Any recursive data structure

The standard workaround is to use **resources** (opaque handles) and manipulate trees through indirection. This works but is awkward for message-passing architectures where data is serialized anyway.

**Composite** defines a WIT+ dialect with recursion allowed by default and a
graph-encoded ABI that naturally handles arbitrary-depth structures.

## Design Goals

1. **WIT+ dialect** - Recursion is allowed by default
2. **Simple authoring** - No `rec` keywords or blocks
3. **Compatible execution** - Uses standard WASM runtimes (wasmi, wasmtime)
4. **Single ABI** - Graph-encoded schema-aware serialization for all values

## Extended WIT Syntax

```wit
// Standard WIT - unchanged
record point {
    x: s32,
    y: s32,
}

variant color {
    rgb(tuple<u8, u8, u8>),
    named(string),
}

// NEW: Recursive types (implicit)
variant sexpr {
    sym(string),
    num(s64),
    flt(f64),
    str(string),
    lst(list<sexpr>),  // Self-reference allowed
}

// NEW: Mutually recursive types
variant expr {
    literal(lit),
    binary(string, expr, expr),
}

variant lit {
    number(f64),
    quoted(expr),  // Cross-reference across types
}
```

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    Composite Runtime                         │
│                                                             │
│  ┌─────────────────────────────────────────────────────┐   │
│  │                  Component Layer                     │   │
│  │                                                      │   │
│  │   • WIT+ parsing (standard + recursive)             │   │
│  │   • Component instantiation and linking             │   │
│  │   • Host function binding                           │   │
│  └─────────────────────────────────────────────────────┘   │
│                           │                                 │
│                           │                                 │
│  ┌─────────────────────────────────────────────────────┐   │
│  │                    ABI Layer                         │   │
│  │                                                      │   │
│  │   Graph-encoded ABI for all values                  │   │
│  │   (schema-aware arena encoding)                     │   │
│  └─────────────────────────────────────────────────────┘   │
│                           │                                 │
│                           │                                 │
│  ┌─────────────────────────────────────────────────────┐   │
│  │              WASM Execution (pluggable)              │   │
│  │                                                      │   │
│  │   wasmi (interpreter) / wasmtime (JIT) / other      │   │
│  └─────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
```

## ABI for WIT+

All values use a schema-aware graph encoding. The runtime:
1. Encodes the value into a graph buffer
2. Writes bytes to linear memory
3. Passes (pointer, length) to the WASM function
4. Decodes the buffer using the expected schema

This format supports shared subtrees and cycles and enables future zero/low-copy
views over the arena.

## Compatibility

WIT+ is a new dialect and is not wire-compatible with canonical ABI components.
Interop requires explicit adapters at the boundary.

## Use Cases

### Wisp Compiler (S-expressions)

```wit
variant sexpr {
    sym(string),
    num(s64),
    lst(list<sexpr>),
}

interface macro {
    expand: func(input: sexpr) -> result<sexpr, string>;
}
```

### Tree Transformations

```wit
variant tree {
    leaf(string),
    node(list<tree>),
}

interface transform {
    map-leaves: func(t: tree, prefix: string) -> tree;
    flatten: func(t: tree) -> list<string>;
}
```

### Configuration/Data

```wit
variant json {
    null,
    bool(bool),
    number(f64),
    str(string),
    array(list<json>),
    object(list<tuple<string, json>>),
}

interface config {
    get: func(key: string) -> option<json>;
    set: func(key: string, value: json) -> result<_, string>;
}
```

## Status

**Early design phase.** This document describes the intended design, not current implementation.

## Related Projects

- [WebAssembly Component Model](https://github.com/WebAssembly/component-model)
- [WIT Specification](https://github.com/WebAssembly/component-model/blob/main/design/mvp/WIT.md)
- [wasmtime](https://github.com/bytecodealliance/wasmtime)
- [wasmi](https://github.com/wasmi-labs/wasmi)
