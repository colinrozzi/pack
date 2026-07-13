# Changelog

## v0.9.0 (2026-07-13)

### Added
- **`pack compose`**: static composition of compiled packages into a single self-contained `.wasm` with **zero imports**, runnable on any stock runtime. `packr compose <manifest.toml>` merges packages (binaryen `wasm-merge`) and internalizes cross-package imports into direct calls; a `walrus` pass unifies the memory imports into one internal memory and bakes the allocator's base/heap globals into constants. Requires `wasm-merge` (binaryen) at compose time. Public API: `packr::{compose, ComposeSpec, PackageSpec, Layout}`.

### Removed
- **`compose::StaticComposer`** and **`runtime::CompositionBuilder`** (with `BuiltComposition`, `HostFn`) — superseded composers. Runtime composition is now `runtime::PicCompositionBuilder` (shared-memory PIC, v0.8.x); static composition is `pack compose`. `ParsedModule` is retained.

## v0.2.0 (2026-04-26)

### Added
- **Type space validation**: `validate_value_in_type_space` checks runtime Values against declared type spaces. Supports records, variants, enums, flags, nested types, and `Type::Value` escape hatch.
- **`TypeValidationError`**: nested error type with context paths for clear diagnostics.
- **`pack_types!` type definitions**: the macro accepts `record`, `variant`, `enum`, `flags`, and `type` alias definitions alongside `imports`/`exports`.
- **`pack_types!(file = "path")`**: load type definitions from external `.pact` files.
- **Array ABI encoding**: compact primitive list encoding (`0x15` node kind).
- **Interface transforms**: `InterfaceTransform` trait and `RpcTransform` for composable interface modification.
- **Interface hashing**: Merkle-tree structural hashing for O(1) compatibility checking.

### Fixed
- Host-side metadata decoder now extracts full TypeDefs (record fields, variant cases) from encoded metadata. Previously discarded structural info and only kept names.

### Changed
- `pack-guest` derive and macro improvements.
- State passed as `Value` directly (not `Option<Value>`).

## v0.1.0

Initial release. Graph ABI encoding, WIT+ parser with recursive types, pack runtime.
