# Pack

A WebAssembly package runtime with extended WIT support for recursive data types.

## Motivation

The WebAssembly Component Model's WIT interface definition language doesn't support recursive types. This is a reasonable constraint for shared-memory scenarios where fixed-layout ABIs are desirable, but it's limiting for use cases involving tree-structured data:

- Abstract Syntax Trees (ASTs)
- S-expressions
- JSON/DOM-like structures
- File system trees
- Any recursive data structure

The standard workaround is to use **resources** (opaque handles) and manipulate trees through indirection. This works but is awkward for message-passing architectures where data is serialized anyway.

**Pack** defines a WIT+ dialect with recursion allowed by default and a
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

// Recursive types (implicit)
variant sexpr {
    sym(string),
    num(s64),
    flt(f64),
    str(string),
    lst(list<sexpr>),  // Self-reference allowed
}

// Mutually recursive types
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
│                       Pack Runtime                           │
│                                                             │
│  ┌─────────────────────────────────────────────────────┐   │
│  │                  Package Layer                       │   │
│  │                                                      │   │
│  │   • WIT+ parsing (standard + recursive)             │   │
│  │   • Package instantiation and linking               │   │
│  │   • Host function binding                           │   │
│  └─────────────────────────────────────────────────────┘   │
│                           │                                 │
│  ┌─────────────────────────────────────────────────────┐   │
│  │                    ABI Layer                         │   │
│  │                                                      │   │
│  │   Graph-encoded ABI for all values                  │   │
│  │   (schema-aware arena encoding)                     │   │
│  └─────────────────────────────────────────────────────┘   │
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

## Interface Hashing

Pack uses **Merkle-tree hashing** for O(1) interface compatibility checking. Every type, function, and interface has a content-addressed hash:

```
Actor A                              Actor B
   │                                    │
   │ import-hashes:                     │ export-hashes:
   │ "math/ops" → a1b2c3d4...           │ "math/ops" → a1b2c3d4...
   │                                    │
   └──────────── hashes match ──────────┘
                    ✓ compatible!
```

Key design decisions:
- **Type names excluded**: `Point` and `Vec2` with same structure have same hash (structural typing)
- **Field names included**: `{x: s32}` ≠ `{y: s32}` (access patterns matter)
- **Interface bindings included**: Interface hash includes name→type mappings

See [docs/INTERFACE-HASHING.md](docs/INTERFACE-HASHING.md) for details.

## Crates

| Crate | Description |
|---|---|
| `packr` | Host-side runtime (CLI + library) |
| `packr-abi` | Shared ABI types (`Value`, `GraphValue` derive) — `no_std` compatible |
| `packr-derive` | Derive macros for automatic Value conversion |
| `packr-guest` | Guest-side helpers for writing WASM packages |
| `packr-guest-macros` | Proc macros (`#[export]`, `#[import]`, `pack_types!`) |

## Writing Packages

Packages are written in Rust with `no_std` and compile to WASM.

```rust
#![no_std]
extern crate alloc;
use alloc::string::String;
use packr_guest::{export, Value};

packr_guest::setup_guest!();

packr_guest::pack_types! {
    exports {
        echo: func(input: value) -> value,
    }
}

#[export]
fn echo(input: Value) -> Value {
    input
}
```

### Typed Actors with Derive

```rust
#![no_std]
extern crate alloc;
use alloc::string::String;
use packr_guest::{export, import, pack_types, GraphValue};

packr_guest::setup_guest!();

#[derive(Clone, GraphValue)]
#[graph(crate = "packr_guest::composite_abi")]
pub struct MyState {
    pub name: String,
    pub count: u32,
}

pack_types! {
    exports {
        init: func(state: value) -> result<my-state, string>,
        greet: func(state: my-state, name: string) -> result<tuple<my-state, string>, string>,
    }
}

#[export]
fn init(_state: MyState) -> Result<(MyState, ()), String> {
    Ok((MyState { name: String::from("world"), count: 0 }, ()))
}

#[export]
fn greet(state: MyState, name: String) -> Result<(MyState, String), String> {
    let msg = alloc::format!("Hello, {}!", name);
    Ok((MyState { count: state.count + 1, ..state }, msg))
}
```

### Package Cargo.toml

```toml
[package]
name = "my-package"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
packr-guest = { git = "https://github.com/colinrozzi/pack.git", tag = "v0.3.0" }

[profile.release]
opt-level = "s"
lto = true
```

## Project Structure

```
pack/
├── src/                    # Host-side runtime
│   ├── main.rs             # packr CLI
│   ├── bin/pact.rs         # pact CLI (type checker)
│   ├── abi/                # Graph-encoded ABI (CGRF format)
│   └── runtime/            # WASM execution and host binding
├── crates/
│   ├── pack-abi/           # packr-abi: shared ABI types (no_std)
│   ├── pack-derive/        # packr-derive: GraphValue derive macro
│   ├── pack-guest/         # packr-guest: guest helpers
│   └── pack-guest-macros/  # packr-guest-macros: proc macros
├── packages/               # Example WASM packages
│   ├── echo/               # Echo/transform values
│   ├── logger/             # Uses host imports
│   ├── sexpr/              # S-expression evaluator
│   ├── typed-actor/        # Typed state example
│   └── ...
└── tests/                  # Integration tests
```

## Status

**Working prototype.** Core functionality is implemented and tested:

- [x] WIT+ Parser — recursive and mutually recursive type definitions
- [x] Graph ABI — CGRF format encoding/decoding with schema validation
- [x] WASM Execution — load and run modules via wasmtime
- [x] Guest Macros — `#[export]`, `#[import]`, `pack_types!`, `#[derive(GraphValue)]`
- [x] Host Imports — packages can call back to host
- [x] Interface Enforcement — validate WASM modules implement WIT interfaces
- [x] Interface Hashing — Merkle-tree hashes for O(1) compatibility checking
- [x] Static Composition — compose multiple packages into one module

## Related Projects

- [Theater](https://github.com/colinrozzi/theater) — Actor runtime built on Pack
- [Wisp](https://github.com/colinrozzi/wisp) — Lisp that compiles to Pack WASM modules
- [WebAssembly Component Model](https://github.com/WebAssembly/component-model)
