# RESOLVED: StaticComposer produces invalid WASM for complex modules

**Status: FIXED** - See fixes below

## Summary

`StaticComposer` produces invalid WASM when composing larger/complex modules. The composed output has many function call type mismatches, suggesting the function index remapping is incorrect.

## Reproduction

### Input modules

1. **spawn-repl-actor.wasm** (18,445 bytes)
   - 4 function imports
   - 13 defined functions
   - 1 memory
   - Valid per `wasm-validate`

2. **wisp-compiler.wasm** (149,766 bytes)
   - 0 function imports
   - 133 defined functions
   - 1 memory
   - Valid per wasmtime (wabt reports opcode 0x12 issue but wasmtime loads it fine)

### Composition code

```rust
let composed_wasm = StaticComposer::new()
    .add_module("compiler", compiler_wasm)?
    .add_module("repl", actor_wasm)?
    .wire(
        "repl",
        "wisp:compiler/compiler",
        "compile-source",
        "compiler",
        "compile-source",
    )
    .export("theater:simple/actor.init", "repl", "theater:simple/actor.init")
    .export("theater:simple/message-server-client.handle-send", "repl", "theater:simple/message-server-client.handle-send")
    .export("theater:simple/message-server-client.handle-request", "repl", "theater:simple/message-server-client.handle-request")
    .export("memory", "repl", "memory")
    .compose()?;
```

### Result

Composed WASM is 156,883 bytes but **invalid**:

```
$ wasm-validate --enable-multi-memory composed-repl.wasm
0000272: error: type mismatch in call, expected [i32, i32, i32, i32] but got [i32]
0000374: error: type mismatch in `if true` branch, expected [i32] but got [... i64]
...
(hundreds of similar errors)
```

## Analysis

The errors are all function call type mismatches - call instructions are targeting wrong functions after index remapping.

### Suspected issue location

In `src/compose/merger.rs`, the function index remapping at line 822:

```rust
StoredOperator::Call(idx) => {
    let new_idx = remap.functions.get(idx).copied().unwrap_or(*idx);
    Instruction::Call(new_idx)
}
```

The `.unwrap_or(*idx)` fallback uses the original index if not found in remap, which would be wrong in a merged module context.

### Possible causes

1. **Incomplete remap population** - Not all function indices are being added to the remap HashMap

2. **Ordering issue** - When processing the second module, function indices might not account for the first module's functions correctly

3. **Import counting** - The `num_imported_functions` tracking might be off when one import is wired internally (resolved to another module's export)

### Expected behavior for this case

- Compiler (added first, 0 imports, 133 functions):
  - Functions get merged indices 0-132 (assuming no external imports from compiler)

- Repl-actor (added second, 4 imports where 1 is wired, 13 functions):
  - 3 external imports get merged import indices 0-2
  - 1 wired import (compile-source) maps to compiler's export function
  - 13 defined functions get indices after all imports and compiler functions

When repl-actor code has `call 5` (to its function index 5), it should be remapped to the correct merged index, not left as 5.

## Test modules vs real modules

The 75 existing tests pass. The test modules are small and simple:
- Few functions
- Simple wirings
- Single-digit indices

The real modules are much larger, which likely exposes edge cases in the remapping logic.

## Files

- Composed output saved at: `examples/actors/composed-repl.wasm` (in wisp repo)
- Source modules: `examples/actors/spawn-repl-actor.wasm`, `examples/wisp-compiler.wasm`

## How to reproduce

```bash
# From wisp repo root
cd /home/colin/work/wisp

# Compile the actor (if not already compiled)
cargo run -- compile examples/actors/spawn-repl-actor.lisp examples/actors/spawn-repl-actor

# The compiler WASM should already exist at examples/wisp-compiler.wasm

# Run theater-repl with --static flag to trigger composition
cargo run -p theater-repl -- --static

# This will fail with "WASM execution error: WebAssembly translation error"
# The composed WASM is saved to examples/actors/composed-repl.wasm for inspection
```

## Debugging suggestions

1. Add debug logging to `merge_module()` showing:
   - Each module's import/function counts
   - Each remap entry as it's created
   - Final remap state before processing function bodies

2. Create a minimal failing test with two modules that have:
   - Multiple imports (some wired, some external)
   - Many functions
   - Cross-module calls via wiring

3. Verify the remap contains entries for ALL function indices before `remap_function_body()` is called

## Environment

- Pack commit: (current main)
- wasmparser: 0.219.2
- wasm-encoder: (matching version)
- Rust: 1.85+
- OS: NixOS

## Resolution

Two issues were identified and fixed:

### Issue 1: Import/function processing order

**Problem**: Imports and defined functions were processed per-module sequentially, causing function index collisions. When module A (no imports, 133 functions) was processed first, its functions got indices 0-132. When module B (4 imports) was processed second, its imports got indices 0-2, overlapping with module A's functions.

**Fix**: Split processing into two passes:
1. First pass: Process ALL imports from ALL modules
2. Second pass: Process ALL defined functions (which now start after all imports)

Location: `src/compose/merger.rs` - Step 5 split into Step 5 (imports) and Step 5b (defined functions)

### Issue 2: Unsupported WASM instructions silently dropped

**Problem**: The `convert_operator()` function returns `None` for unsupported instructions, and the calling code silently skips them. This corrupts function bodies when instructions like `return_call` (tail-call proposal) are used.

**Fix**: Added support for `return_call` and `return_call_indirect` instructions in both parser and merger.

Location:
- `src/compose/parser.rs` - Added `ReturnCall` and `ReturnCallIndirect` variants
- `src/compose/merger.rs` - Added conversion for tail-call instructions

### Issue 3: Host functions not wired to packages without cross-package imports

**Problem**: `CompositionBuilder` treated packages without `wire()` calls as "providers" and instantiated them without host functions, even if host functions were registered.

**Fix**: Modified the provider/consumer classification to treat packages as consumers when host functions are defined.

Location: `src/runtime/composition.rs` - Added `has_host_functions` check to consumer filter
