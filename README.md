# Composite

A WebAssembly package runtime with extended WIT support for recursive data types.

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
│  │                  Package Layer                       │   │
│  │                                                      │   │
│  │   • WIT+ parsing (standard + recursive)             │   │
│  │   • Package instantiation and linking               │   │
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

**Working prototype.** Core functionality is implemented and tested:

- [x] **WIT+ Parser** - Parses recursive and mutually recursive type definitions
- [x] **Graph ABI** - CGRF format encoding/decoding with schema validation
- [x] **WASM Execution** - Load and run modules via wasmi
- [x] **Memory Access** - Read/write linear memory, pass data to WASM
- [x] **Graph ABI Integration** - `write_value`, `read_value`, `call_with_value` for passing recursive types
- [x] **Rust Packages** - no_std packages using shared `composite-abi` crate
- [x] **Host Imports** - Packages can call back to host (`host.log`, `host.alloc`)
- [x] **Derive Macros** - `#[derive(GraphValue)]` for automatic Value conversion
- [x] **S-expression Evaluator** - Full Lisp-like evaluator as demo package
- [x] **Interface Enforcement** - Validate WASM modules implement WIT interfaces
- [x] **Flexible Host Functions** - Namespaced interfaces, typed functions, provider pattern
- [x] **Interface Hashing** - Merkle-tree hashes for O(1) compatibility checking

### Project Structure

```
composite/
├── src/
│   ├── lib.rs              # Main library exports
│   ├── abi/                # Graph-encoded ABI (CGRF format)
│   ├── wit_plus/           # WIT+ parser and type system
│   └── runtime/            # WASM execution and host binding
├── crates/
│   ├── composite-abi/      # Shared ABI crate (no_std compatible)
│   └── composite-derive/   # Derive macros for Value conversion
├── packages/
│   ├── echo/               # Example: echo/transform values
│   ├── logger/             # Example: uses host imports
│   └── sexpr/              # Example: S-expression evaluator
└── tests/
    ├── wasm_execution.rs      # WASM runtime integration tests
    ├── interface_enforcement.rs # Interface validation tests
    ├── host_functions.rs      # Host function API tests
    ├── abi_roundtrip.rs       # ABI encoding tests
    └── schema_validation.rs   # Type validation tests
```

### Quick Start (Host)

```rust
use composite::{Runtime, abi::Value, runtime::HostImports};

// Load a WASM package
let runtime = Runtime::new();
let module = runtime.load_module(&wasm_bytes)?;

// Instantiate with host imports
let imports = HostImports::new();
let mut instance = module.instantiate_with_imports(imports)?;

// Call with recursive values
let input = Value::List(vec![
    Value::S64(1),
    Value::S64(2),
    Value::Variant { tag: 0, payload: Some(Box::new(Value::String("hello".into()))) },
]);
let output = instance.call_with_value("process", &input, 0)?;

// Check logs from package
for msg in instance.get_logs() {
    println!("Package logged: {}", msg);
}
```

### Custom Host Functions

For advanced use cases, register custom host functions with namespaced interfaces:

```rust
use composite::{Runtime, abi::Value};
use wasmi::Caller;

struct MyState {
    counter: i32,
}

let module = runtime.load_module(&wasm_bytes)?;

let mut instance = module.instantiate_with_host(MyState { counter: 0 }, |builder| {
    // Register functions under namespaced interfaces
    builder.interface("myapp:api/v1")?
        // Raw functions for direct WASM-level access
        .func_raw("increment", |caller: Caller<'_, MyState>, amount: i32| -> i32 {
            let state = caller.data();
            state.counter += amount;
            state.counter
        })?
        // Typed functions with automatic Graph ABI encode/decode
        .func_typed("transform", |ctx, input: Value| -> Value {
            match input {
                Value::S64(n) => Value::S64(n * 2),
                other => other,
            }
        })?;
    Ok(())
})?;
```

#### Typed Functions with Custom Types

Use `#[derive(GraphValue)]` types with `func_typed`:

```rust
#[derive(GraphValue)]
struct Point { x: i64, y: i64 }

builder.interface("geometry")?
    .func_typed("translate", |ctx, point: Point| -> Point {
        Point { x: point.x + 10, y: point.y + 10 }
    })?;
```

#### Reusable Function Providers

Create reusable sets of host functions:

```rust
use composite::runtime::{HostFunctionProvider, HostLinkerBuilder, LinkerError};

struct LoggingProvider;

impl<T> HostFunctionProvider<T> for LoggingProvider {
    fn register(&self, builder: &mut HostLinkerBuilder<'_, T>) -> Result<(), LinkerError> {
        builder.interface("logging")?
            .func_raw("debug", |caller, ptr, len| { /* ... */ })?
            .func_raw("info", |caller, ptr, len| { /* ... */ })?;
        Ok(())
    }
}

// Use it
builder.register_provider(&LoggingProvider)?;
```

## Writing Packages

Packages are written in Rust with `no_std` and compile to WASM.

### Simple Types with Derive

For non-recursive types, use the derive macro:

```rust
use composite_abi::{GraphValue, Value};

#[derive(GraphValue)]
struct Point {
    x: i64,
    y: i64,
}

#[derive(GraphValue)]
enum Shape {
    Circle(f64),
    Rectangle(f64, f64),
    Point,
}

// Automatic conversion
let point = Point { x: 10, y: 20 };
let value: Value = point.into();
let back: Point = value.try_into().unwrap();
```

### Recursive Types (Manual)

Recursive types use `Box<T>` which requires manual `From`/`TryFrom` implementations:

```rust
use composite_abi::{Value, ConversionError};

enum SExpr {
    Num(i64),
    Cons(Box<SExpr>, Box<SExpr>),
    Nil,
}

impl From<SExpr> for Value {
    fn from(expr: SExpr) -> Value {
        match expr {
            SExpr::Num(n) => Value::Variant {
                tag: 0,
                payload: Some(Box::new(Value::S64(n)))
            },
            SExpr::Cons(head, tail) => Value::Variant {
                tag: 1,
                payload: Some(Box::new(Value::Tuple(vec![
                    (*head).into(),
                    (*tail).into(),
                ]))),
            },
            SExpr::Nil => Value::Variant { tag: 2, payload: None },
        }
    }
}

impl TryFrom<Value> for SExpr {
    type Error = ConversionError;
    // ... symmetric implementation
}
```

See `packages/sexpr/` for a complete example with 25+ built-in functions.

### Package Cargo.toml

```toml
[package]
name = "my-package"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
composite-abi = { path = "../../crates/composite-abi", default-features = false, features = ["derive"] }

[profile.release]
opt-level = "s"
lto = true
```

### Host Imports

Packages can call host functions:

```rust
#[link(wasm_import_module = "host")]
extern "C" {
    fn log(ptr: i32, len: i32);
    fn alloc(size: i32) -> i32;
}
```

## Related Projects

- [WebAssembly Component Model](https://github.com/WebAssembly/component-model)
- [WIT Specification](https://github.com/WebAssembly/component-model/blob/main/design/mvp/WIT.md)
- [wasmtime](https://github.com/bytecodealliance/wasmtime)
- [wasmi](https://github.com/wasmi-labs/wasmi)
