# WIT+ Dialect and ABI Updates

## Summary

We pivoted to a WIT+ dialect that allows recursion by default and uses a single
schema-aware graph ABI for all values. This removed the canonical ABI split and
simplified the type system and parsing rules. We also added a minimal parser
scaffold, graph buffer serialization, and a first Value codec implementation.

## Highlights

- WIT+ is now a distinct dialect; recursion is implicit and mutual recursion is
  allowed via named references in a single namespace.
- The ABI is a graph-encoded arena for all values; no tagged self-describing
  encoding or canonical ABI interop.
- Type system and docs were updated to remove `rec`/`rec group` and the old
  `RecursiveDef`.
- A parser scaffold now supports interfaces, type definitions, and basic
  function signatures with validation.
- Graph buffer serialization and validation exist alongside a Value graph codec.

## Key Files Updated

- docs/DESIGN.md
- README.md
- src/wit_plus/types.rs
- src/wit_plus/mod.rs
- src/wit_plus/parser.rs
- src/abi/mod.rs
- src/lib.rs

## Notable API/Behavior Changes

- `wit_plus` validation now resolves named types across the whole file and allows
  mutual recursion; undefined names and misuse of `self` are errors.
- `abi::encode` now returns `Result<Vec<u8>, AbiError>`; `abi::decode` validates
  the graph buffer before decoding.
- Graph buffer serialization uses a fixed header (magic/version) and per-node
  payload encoding; decoding performs structural checks and detects cycles.

## Open Follow-ups

- Extend codec support for all primitive types (u8/u16/u32/u64/s8/s16/char).
- Add schema-aware validation against WIT+ type definitions.
- Add round-trip and edge-case tests for ABI encoding/decoding.
- Expand parser coverage for imports/exports and interface naming conventions.
