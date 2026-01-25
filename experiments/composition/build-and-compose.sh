#!/usr/bin/env bash
set -e

cd "$(dirname "$0")"

echo "=== Building doubler component ==="
cd doubler
cargo component build --release
cd ..

echo "=== Building adder component ==="
cd adder
cargo component build --release
cd ..

echo "=== Composing components ==="
wasm-tools compose \
  adder/target/wasm32-wasip1/release/adder.wasm \
  -d doubler/target/wasm32-wasip1/release/doubler.wasm \
  -o composed.wasm

echo "=== Inspecting composed component ==="
wasm-tools component wit composed.wasm

echo ""
echo "Success! Created composed.wasm"
echo "The composed component exports: process:adder/compute.process"
echo ""
echo "To test it, you can use wasmtime:"
echo "  wasmtime run --invoke 'process' composed.wasm 5"
echo "  # Should output 11 (5 * 2 + 1)"
