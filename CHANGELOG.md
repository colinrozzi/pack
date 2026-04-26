# Changelog

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
