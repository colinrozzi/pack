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

**Composite** extends WIT with recursive types, using a serialization-based ABI that naturally handles arbitrary-depth structures.

## Design Goals

1. **Superset of WIT** - Standard WIT files work unchanged
2. **Recursive types** - First-class support via `rec` keyword
3. **Compatible execution** - Uses standard WASM runtimes (wasmi, wasmtime)
4. **Clean ABI** - Recursive types serialize; standard types use canonical ABI

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

// NEW: Recursive types
rec variant sexpr {
    sym(string),
    num(s64),
    flt(f64),
    str(string),
    lst(list<sexpr>),  // Self-reference allowed
}

// NEW: Mutually recursive types
rec group {
    variant expr {
        literal(lit),
        binary(string, expr, expr),
    }

    variant lit {
        number(f64),
        quoted(expr),  // Cross-reference within group
    }
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
│  │   Standard types    │    Recursive types            │   │
│  │   → Canonical ABI   │    → Serialization ABI        │   │
│  │   (fixed layout)    │    (length-prefixed bytes)    │   │
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

## ABI for Recursive Types

Recursive types use a tagged, length-prefixed binary encoding:

```
value ::=
    | 0x00 <bool>                           -- bool
    | 0x01 <i32>                            -- s32
    | 0x02 <i64>                            -- s64
    | 0x03 <f32>                            -- f32
    | 0x04 <f64>                            -- f64
    | 0x05 <len:u32> <bytes:utf8[len]>      -- string
    | 0x06 <len:u32> <items:value[len]>     -- list
    | 0x07 <tag:u32> <has_payload:u8> [value]  -- variant
    ...
```

When a function parameter or return type involves a recursive type, the runtime:
1. Serializes the value to bytes
2. Writes bytes to linear memory
3. Passes (pointer, length) to the WASM function
4. Deserializes the result

## Compatibility

| Component Type | Standard Runtime | Composite |
|----------------|------------------|-----------|
| Standard WIT only | ✓ | ✓ |
| Uses recursive types | ✗ | ✓ |

Components that only use standard WIT types are fully compatible with other runtimes. Components using recursive types require Composite.

## Use Cases

### Wisp Compiler (S-expressions)

```wit
rec variant sexpr {
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
rec variant tree {
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
rec variant json {
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
