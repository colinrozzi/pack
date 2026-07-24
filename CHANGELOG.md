# Changelog

## v0.12.0 (2026-07-24)

**Component composition — packr's Component-Model equivalent.** Compose N isolated
packages into ONE multi-memory wasm binary via `packr compose`. Each component keeps
its own memory (so the fusion reconciliation bug class is structurally impossible), a
statically-generated bridging shim marshals every cross-component call over the Graph
ABI, and the composite loads as a normal theater actor — it exports the entry
component's `memory` + `__pack_alloc`/`__pack_free` + pact functions, and its only
residual imports are host functions. Proven end-to-end under real theater (a composite
runs through theater's own loader and is driven through the actor lifecycle) and against
an async service component (a provider that suspends on an async host call resumes
correctly through the synchronous shim).

### Added
- **`packr compose <manifest> -o <out>`** + `packr::compose(components, links)` — compose
  N components across a link graph into one multi-memory composite. Manifest is TOML:
  `[[component]]` (name/wasm/entry) + `[[link]]` (consumer/import/provider/export).
- **Hash-checked links.** Before wiring, `compose` statically reads each component's
  per-interface Merkle hashes from its `__pack_types` segment and **rejects a link whose
  consumer-import and provider-export interface hashes disagree — at compose time**, with
  an error naming the interface and both hashes, instead of a runtime "failed to convert
  parameter". This catches signature drift between independently-versioned packages
  automatically (both sides embed hashes via the guest macro). A component with no
  embedded hashes, or a name-remapped link, is left name-wired.
- Async-transparent composition: a composed component that suspends on an async host
  import resumes correctly through the **unchanged** synchronous bridging shim (wasmtime
  suspends the whole fiber at the host boundary).
- `metadata::find_cgrf_metadata` is now public — statically extract a module's CGRF
  `__pack_types` bytes from its data segments, no instantiation required.

### Fixed
- **Shim result-buffer leak.** Both generated shims (the link shim and the host-bridge
  shim) copy the callee's result into the caller's memory but returned the callee's raw
  status, so the caller's `__import_impl` never freed the buffer — one dlmalloc chunk
  leaked per cross-component (or residual host) call, growing unboundedly for a host-heavy
  composed actor. Both shims now return the guest-owned status so the caller frees the
  buffer; the host-bridge additionally frees the host's result buffer when the host
  guest-allocated it. Regression test asserts the entry heap plateaus over 20k calls.
- A temp-file race in the wasm-merge step (two concurrent `compose` calls in one process
  shared PID-named temp files); added a per-call nonce.

## v0.11.1 (2026-07-23)

### Added
- **`pact codegen` now emits the `GraphValue` codec on generated types.** Records,
  variants, and enums get `#[derive(..., packr_guest::GraphValue)]` +
  `#[graph(crate = "packr_guest::composite_abi")]`, so a pact-generated Rust module
  actually **serializes** (encode/decode via the Graph ABI) instead of only declaring
  types. This closes the last gap for **importable app-to-app pact packages**: a
  `.pact` now codegens directly into a working, importable codec crate with zero
  hand-editing. Verified end-to-end against `mesh-api/src/control.rs`'s oracle vectors
  (the `mesh:control` envelope) — codegen'd types compile and round-trip losslessly
  (`T → Value → encode → bytes → decode → Value → T`). Regression guard:
  `codegen::tests::codegen_emits_graphvalue_codec_on_records_and_variants`.

## v0.11.0 (2026-07-21)

**An actor is now a plain `cargo build`.** This retires packr's composition/fuse
machinery entirely — a deliberate hard break (we control every actor, so the break
forces the clean rebuild).

### Changed
- **BREAKING: `setup_guest!()` installs a LINKED-IN allocator** (`DlmallocAllocator`)
  instead of the old `ImportedAllocator` that imported `pack:alloc` to be satisfied
  by a fused-in allocator module. So a plain wasm cdylib exports its own memory +
  `__pack_alloc`/`__pack_free` + lifecycle and imports **no** `pack:alloc` — nothing
  to compose. Build an actor with a normal `cargo build --target wasm32-unknown-unknown`
  plus `--export-memory --no-entry` (no fixed-base recipe, no `packr build`/`link`).
  The actor's memory is **growable** (no `internalize` cap), which also removes the
  capped-heap failure mode.

### Removed
- **BREAKING: all composition/fuse machinery.** `pack compose` / `packr link` /
  `packr build` CLI commands; the `compose`/`link` library APIs (`compose`,
  `ComposeSpec`, `PackageSpec`, `Layout`, `link`, `resolve_links`, `read_data_end`,
  `member_region`, …); `internalize` and the multi-member fuse (the source of the
  shadow-stack / resource-reconciliation bug class); the bundled
  `DEFAULT_ALLOCATOR_WASM` allocator blob; and the now-dead `ImportedAllocator`.
  `packr`'s only remaining subcommand is `inspect`.

### Migration
Composition model going forward: **source-deps** for zero-cost sharing (import a
package as a crate and compile it in — "as other libraries do it"), **isolated
actors** (theater message boundary) for runtime composition. A package that was a
fused *helper* becomes a crate dependency; host interfaces (`theater:simple/*`) stay
residual imports the runtime provides. Every actor must be rebuilt as a plain cdylib
on packr-guest 0.11.0 — no compat path, by design.

## v0.10.6 (2026-07-21)

### Fixed
- **Epoch deadline overflow panicked/mis-fired on every actor spawn (0.10.5
  regression).** The self-contained instantiate paths armed a "no deadline"
  default of `store.set_epoch_deadline(u64::MAX)`. But `set_epoch_deadline(delta)`
  computes `current_epoch() + delta`, so once the host advances the engine epoch
  (a 1/sec ticker driving `increment_epoch()`), `current + u64::MAX` **overflows**
  — a panic in debug, a wrap to a garbage near-immediate deadline in release —
  on *every* instantiate. The 0.10.5 kill-switch test missed it because the epoch
  was still 0 at instantiation. The default is now `u64::MAX / 2` (`NO_EPOCH_DEADLINE`)
  — still ~4.6e18 ticks (never trips), and `current + it` cannot overflow for any
  realistic epoch count. The store genuinely has `epoch_interruption` enabled
  (confirmed: `current_epoch()` returns a valid small count); only the default
  delta was wrong. Regression test `epoch_deadline_survives_advanced_epoch`
  reproduces the panic (advance epoch → instantiate → arm → call) and passes on
  the fix. Host wiring (`set_epoch_deadline(N)` + `increment_epoch()` ticker) is
  unchanged and correct.

## v0.10.5 (2026-07-21)

### Added
- **Runaway-guest kill switch: epoch interruption on `AsyncRuntime`.** A guest
  stuck in an infinite loop (e.g. a pathological decode) was previously
  UNINTERRUPTIBLE — it pegged a core forever and could wedge the host (the
  mail-spine failure class: one bad mailbox `init` hung the whole spine, and the
  init-watchdog could name the spinner but not kill it). The async engine now
  enables `epoch_interruption`; `AsyncInstance::set_epoch_deadline(ticks)` arms a
  per-call deadline and the host advances epochs via
  `AsyncRuntime::engine().increment_epoch()` on a ticker — when the deadline
  passes the guest **traps** and the call returns `Err`, so a runaway fails
  cleanly instead of burning a core. **Non-breaking**: stores default to no
  deadline (`u64::MAX`), so behaviour is unchanged until a caller opts in. Test:
  `runaway_guest_traps_on_epoch_deadline`.

## v0.10.4 (2026-07-19)

### Fixed
- **Mailbox `load_state` decode spun the mail spine (prod hang, root cause).**
  Decoding a graph value deep-cloned *every* node's subtree into the DAG-dedup
  cache (`cache.insert(index, value.clone())`) — even though the encoder only
  ever emits trees (no shared nodes), so nothing was ever read back. For a
  restored mailbox (`MailboxState { messages: Vec<Message> }` — hundreds of
  records) that is a **~3x peak-memory blowup and ~3x the allocations**. Decode
  stays *linear in time* either way (a flat `Vec` is shallow — it was never
  quadratic), but against a self-contained actor's **capped** WASM heap
  (`internalize` fixes `memory.max`, so `memory.grow` cannot extend it) the
  transient blowup pushes a big-enough mailbox past the ceiling: dlmalloc can't
  grow, the allocation fails, and the guest spins (~42% CPU) before its first log
  line. The decoder now runs a refcount pre-pass (`shared_nodes`) and caches
  **only** nodes referenced by more than one parent, so a tree clones nothing —
  decode is O(n) with ~1x peak. Genuine DAGs still decode correctly and never
  re-traverse (new `dag_shared_child_decodes_correctly` guard). Size-correlated,
  before-first-log, and CI-missed (fake sub-manifests → no real store load) — all
  consistent. **Wire format unchanged**: the same bytes decode to the same value,
  no data migration — the existing accumulated mailboxes decode fast on the fixed
  decoder. New regression bench: `tests/mailbox_decode_bench.rs` (synthetic
  N-sweep with a counting allocator, proving linear time + ~1x peak). Fleet
  event: theater bumps its `packr-abi` pin and re-cuts; `packr-guest` consumers
  rebuild in lockstep.

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
