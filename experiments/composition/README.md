# Package Composition Experiment

This experiment demonstrates **shared-nothing composition** of WebAssembly components
using `wasm-tools compose`.

## What This Demonstrates

Two separate WASM components, each with their own linear memory, composed into a single
`.wasm` file where one component's imports are satisfied by the other's exports.

```
┌─────────────────────────────────────────────────────┐
│                  composed.wasm                       │
│  ┌─────────────┐         ┌─────────────────────┐   │
│  │   doubler   │         │       adder         │   │
│  │             │         │                     │   │
│  │ export:     │────────▶│ import: math.double │   │
│  │ math.double │         │ export: compute     │──▶│ final export
│  │             │         │                     │   │
│  │ [memory 1]  │         │ [memory 2]          │   │
│  └─────────────┘         └─────────────────────┘   │
└─────────────────────────────────────────────────────┘
```

## Prerequisites

Install the required tools:

```bash
# Install cargo-component (for building WASM components)
cargo install cargo-component

# Install wasm-tools (for composition)
cargo install wasm-tools

# Ensure you have the wasm32-wasip1 target
rustup target add wasm32-wasip1
```

## Structure

```
composition/
├── wit/
│   ├── doubler.wit    # Defines the math interface (double function)
│   └── adder.wit      # Imports math, exports compute interface
├── doubler/           # Component that doubles numbers
│   ├── Cargo.toml
│   └── src/lib.rs
├── adder/             # Component that imports doubler, adds 1
│   ├── Cargo.toml
│   └── src/lib.rs
├── build-and-compose.sh
└── README.md
```

## Building & Composing

```bash
# Run the build script
./build-and-compose.sh

# Or manually:
cd doubler && cargo component build --release && cd ..
cd adder && cargo component build --release && cd ..

wasm-tools compose \
  adder/target/wasm32-wasip1/release/adder.wasm \
  -d doubler/target/wasm32-wasip1/release/doubler.wasm \
  -o composed.wasm
```

## How It Works

### 1. Doubler Component (`doubler/`)

Exports `process:doubler/math.double(n: s64) -> s64`:

```rust
impl Guest for Doubler {
    fn double(n: i64) -> i64 {
        n * 2
    }
}
```

### 2. Adder Component (`adder/`)

Imports `process:doubler/math` and exports `process:adder/compute.process(n: s64) -> s64`:

```rust
impl Guest for Adder {
    fn process(n: i64) -> i64 {
        let doubled = math::double(n);  // calls doubler
        doubled + 1
    }
}
```

### 3. Composition

`wasm-tools compose` wires adder's import to doubler's export, creating a single
component that:
- Contains both components internally (with separate memories)
- Exports only `process:adder/compute.process`
- Handles cross-component calls via the canonical ABI

## Testing

```bash
# Inspect the composed component's WIT
wasm-tools component wit composed.wasm

# Run it with wasmtime (if it supports the interface)
wasmtime run composed.wasm --invoke process 5
# Expected: 11 (5 * 2 + 1)
```

## Key Concepts

- **Shared-Nothing**: Each component keeps its own linear memory. Data crossing
  component boundaries is copied/serialized via the canonical ABI.

- **Interface-Based**: Components are composed at the interface level. The socket
  (adder) declares what it needs; the plug (doubler) provides it.

- **Single Binary**: The result is one `.wasm` file that can be deployed anywhere
  a WASM runtime supports the Component Model.

## Next Steps for Composite

To integrate this with Composite's Graph ABI:

1. Define WIT interfaces that use the Graph ABI encoding
2. Create a `composite:abi/value` interface type
3. Build components that import/export using this type
4. Compose components that transform values through a pipeline
