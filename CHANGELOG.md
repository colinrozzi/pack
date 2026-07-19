# Changelog

## v0.10.3 (2026-07-19)

### Fixed
- **Composite layout overrun corrupting the bundled allocator (prod hang).** A
  member whose `.rodata` exceeds the fixed `alloc_base` (default `0xE0000`)
  overwrote the bundled allocator's dlmalloc control structures, so the first
  allocation trapped or spun forever — the mail-spine 0.10.2 hang on big-surface /
  crypto actors (e.g. an actor with DKIM RSA). The Value decoder is O(n) and the
  allocator is clean; the root was purely the layout. `link()`/`compose` now
  **auto-raise** `alloc_base`/`heap_base`/`metadata_base` above every member
  (`fit_layout`), so a fixed default layout works for any actor with no per-actor
  change. Host-ABI unchanged (compose-side only). (#60)

### Added
- **`packr build` multi-member.** A build manifest (`[[member]]` crates +
  `[[link]]` edges) assigns each member a **disjoint memory region**
  automatically, builds it, and links — the fix for the multi-member same-base
  collision (two members at one base corrupt each other's static data and trap).
  (#60)
- **`packr::read_data_end` / `member_region`** — a member's `[base, __data_end)`
  static-data extent. `packr link` now **rejects pre-built members whose regions
  overlap** up front instead of emitting a silently-trapping composite. (#60)

## v0.10.2 (2026-07-15)

Follow-ups to the 0.10.0 self-contained cutover, tightening the loader's boot
contract and closing the allocator-provenance gap. (0.10.1 was folded in — its
boot check ships here.)

### Added
- **`packr::DEFAULT_ALLOCATOR_WASM`** — the `pack:alloc` allocator module, bundled
  into the crate and version-locked to it. Removing the PIC loader in 0.10.0 had
  dropped the runtime's embedded allocator, leaving `compose`/`link` consumers with
  no allocator to build a self-contained actor. A `link` manifest `[[binary]]` with
  `allocator = true` and **no `wasm` path** now uses the bundled default — a
  self-contained actor build needs no vendored allocator blob. (#55)

### Changed
- **Loader boot check now also requires `__pack_alloc`/`__pack_free`.**
  `assert_self_contained` validates packr's full marshalling ABI at load, not just
  memory-ownership. An actor missing the allocator exports would otherwise
  instantiate and silently limp on bounded fallback buffers; it now fails legibly
  at boot. Still host-agnostic (these are packr's own exports; lifecycle exports
  remain the host's contract, validated host-side). (#54)

## v0.10.0 (2026-07-15)

The **universal self-contained actor** cutover. An actor is now a single
self-contained `.wasm` that **owns its memory** (exports it), keeps data at
absolute addresses (no relocation), and imports only host functions.
**PIC side-module loading is removed** — this is a fleet-lockstep event: actors
must be built self-contained (via `pack compose`/`link`) and hosts (theater) must
bump to 0.10.0 together.

### Changed
- **BREAKING (loader): self-contained runtime loader replaces PIC.** The runtime
  no longer creates a shared memory/table, instantiates an allocator side module,
  or wires PIC linkage globals (`env.__memory_base`/`__table_base`/`__stack_pointer`/
  `GOT.mem.*`). `instantiate_with_host_and_interceptor_async` (signature unchanged)
  now wires only host functions, instantiates the actor, and grabs the actor's
  **exported** memory. The load-time guard inverts: `assert_self_contained` rejects
  a module that imports `env.memory`/`env.__memory_base` and requires an exported
  memory — a mis-flipped PIC/pre-0.10 actor fails legibly at boot. Host-agnostic
  (no host-interface allowlist); memory-ownership is the single axis it gates on.

### Added
- **Host-agnostic residual surface in `pack compose`.** `internalize` gates only on
  memory-ownership (a composite must own its memory); any import no link satisfied
  survives as legitimate *residual surface* for the eventual host to provide — no
  module-name allowlist. (#50)
- **`host-actor` fixture + `tests/link_actor.rs`** — the first composite with a
  non-empty residual surface, proving a host import survives while a helper import
  internalizes, end-to-end. (#51)

### Fixed
- **`.rodata` blanked in composites.** `embed_pack_types` deleted whole data
  segments beginning with the CGRF magic to strip stale `__pack_types` blobs, but
  that metadata is the prefix of a `.rodata` segment that also holds live string
  literals at fixed absolute addresses — blanking them. Now the magic is zeroed in
  place, preserving every string. Any composed actor reading static strings/tables
  was affected. (#51)

### Removed
- PIC runtime machinery: `PicComposition`/`PicCompositionBuilder`,
  `PicInstance`/`instantiate_pic`, and the internal `pic_link`/`resolve_got_data_end`/
  allocator-side-module path. Static composition (`pack compose`) and the
  self-contained loader supersede it. (#52)

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
