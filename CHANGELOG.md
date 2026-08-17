# Changelog

## v0.20.0 (2026-08-17)

### Added

- **Cross-file `use` imports.** A Pact file can now pull type definitions from
  another Pact file, so a type is single-sourced instead of hand-mirrored across
  crates:

  ```pact
  // consumer.pact
  use "../shared.pact".{msg, chat-state};
  record snapshot { latest: chat-state, last-msg: msg }
  world consumer { export snap: func(s: snapshot) -> snapshot }
  ```

  The path is **path-based**, resolved relative to the importing file's directory
  (relative to `CARGO_MANIFEST_DIR` for inline / `pact/`-directory input). The
  named types **plus their transitive same-file dependencies** are pulled in and
  generated locally, and every `use`d file is registered as a build dependency
  (so editing it triggers a rebuild). Because guest codegen is deterministic,
  types co-generated in different crates from the same source are wire-identical.
  - Guest lexer gained a string-literal token (for the quoted path).
  - Scope: the `pact!` macro (inline, `pact!(from …)`, and `pact/`-dir). Bare-name
    package imports (`use pkg.{…}`) are a possible later addition.

## v0.19.0 (2026-08-17)

### Added

- **`set<T>` type.** Front-end sugar for a set, the sibling of `map<K, V>`. It
  lowers to `BTreeSet<T>` in Rust and marshals as a key-sorted `list<T>` on the
  wire — **no new `Value` variant, no wire change**; a `set<T>` hashes/validates
  identically to that (sorted) list. Because a `BTreeSet` iterates in key order,
  the encoding is canonical (deterministic) for free — which is what makes a set
  suitable for replicated state-machine state (identical logical state encodes
  byte-identically across replicas).
  - Pact: `set<string>`, `set<tuple<list<u8>, list<u8>>>`, usable anywhere a type
    is — record field, variant payload, and **nested** (`map<K, set<V>>`,
    `list<set<T>>`).
  - Guest: a `set<...>` field/param generates a `BTreeSet<...>`.
- **`KnownValueType for BTreeSet<T>`** — this is what lets a `BTreeSet` nest
  inside another container (building the outer container's `elem_type` needs it).
  A bare top-level `BTreeSet` field already worked via `From`/`TryFrom`.

### Notes

- Wire-compatible: a set is *exactly* a `list<T>`, so existing packages and
  consumers are byte-for-byte unaffected. Ships as a minor.

## v0.18.1 (2026-08-17)

### Fixed

- **`#[derive(GraphValue)]` on a variant with an `error` case.** A type (or a
  `pact!`/`wit!` variant) with a case named `error` → `Error` failed to compile
  with `ambiguous associated item`: the generated `TryFrom<Value>` impl spelled
  its return type as `Self::Error`, which is ambiguous between the `Error`
  *variant* and the trait's `Error` *associated type* (a deny-by-default
  future-incompat lint). The return type is now spelled as the concrete
  `ConversionError`. No behavior change; any guest with an `error`/`result`-ish
  variant now compiles.

## v0.18.0 (2026-08-15) — BREAKING

Committed fully to the **Pact** name; **"wit" is gone** from the public surface.
This is a breaking release — guest actors on the old names must migrate.

### Breaking

- **`wit!` removed.** The deprecated `wit!` alias (renamed to `pact!` in 0.17) is
  deleted. Use `pact!`.
- **`wit/` directory → `pact/`.** `pact!()` / `world!()` with no argument now read
  a `pact/` directory (was `wit/`).
- **`.wit` / `.wit+` extensions → `.pact`.** Definition files must use `.pact`.
- **`#[export(wit = "…")]` / `#[import(wit = "…")]` → `pact = "…"`.** The
  signature-string attribute argument was renamed.

### Migration

- Replace every `wit!` with `pact!`.
- Rename each crate's `wit/` directory to `pact/` and its `*.wit`/`*.wit+` files
  to `*.pact`.
- Replace `wit =` with `pact =` in `#[export]` / `#[import]` attributes.
- No wire/ABI/metadata change — regenerated code is byte-identical. This is a
  source-level rename only.

### Changed (internal)

- Guest macro internals renamed (`wit_parser`→`pact_parser`, `parse_wit`→
  `parse_pact`, `WitRegistry`→`PactRegistry`, etc.); the host `parser::wit`
  module became `parser::world`. Docs debranded from "WIT+" to "Pact" (factual
  references to the external Component Model WIT standard are retained).

## v0.17.0 (unreleased — folded into 0.18.0)

> This rename+deprecation step was never published on its own; 0.16.0 → 0.18.0
> jumps straight to the hard removal. Kept here as a record of the progression.


### Changed

- **`wit!` is now `pact!`.** The guest type-generation macro was renamed to match
  the Pact interface format the runtime uses everywhere else (`parse_pact`,
  `.pact`, `PactInterface`). `pact!` behaves identically — inline, `pact!(from
  "path")`, and the `wit/`-directory forms all work the same.

  ```rust
  packr_guest::pact! { /* … */ }
  packr_guest::pact!(from "../shared/api.wit+");
  ```

### Deprecated

- **`wit!`** — kept as a working alias that forwards to `pact!`, now marked
  `#[deprecated]`. Migrate by replacing `wit!` with `pact!`; the alias will be
  removed in a future release.

## v0.16.0 (2026-08-13)

### Added

- **`wit!(from "path")` — source a definition from a file.** The `wit!` macro now
  accepts a file path in addition to inline WIT+ and the `wit/` directory:

  ```rust
  packr_guest::wit!(from "../shared/api.wit+");   // or: wit!("../shared/api.wit+")
  ```

  This lets several crates share **one** WIT+ definition file instead of copying
  (or symlinking) a duplicate into each repo. A relative path resolves against
  `CARGO_MANIFEST_DIR` (the crate root); an absolute path is used as-is. The file
  is registered as a build dependency (via `include_bytes!`), so editing the
  shared definition triggers a rebuild of every crate that reads it — something a
  symlinked copy does not reliably do.

## v0.15.0 (2026-08-12)

### Added

- **`map<K, V>` type.** Front-end sugar for an associative map. It lowers to
  `BTreeMap<K, V>` in Rust and marshals as a key-sorted `list<tuple<K, V>>` on
  the wire — so it introduces **no new `Value` variant and no wire change**, and
  a `map<K, V>` hashes/validates identically to the equivalent list of pairs
  (type-parameter erasure, the same principle as generics). Because a
  `BTreeMap` iterates in key order, the encoding is canonical (deterministic
  key ordering) for free.
  - Pact/WIT+: `map<string, u32>`, usable anywhere a type is (fields, params,
    results, nested in `list`/`option`/generics).
  - Guest: a `map<...>` field/param generates a `BTreeMap<...>`; encode/decode
    come from `packr_abi`'s `From<BTreeMap>` / `TryFrom<Value>` impls (and a
    matching `KnownValueType`), routed through the tested `GraphValue` derive.

### Notes

- Wire-compatible: a map is *exactly* a `list<tuple<K, V>>`, so existing
  packages and consumers are byte-for-byte unaffected. Ships as a minor.

## v0.14.0 (2026-08-09)

### Added

- **Recursive types.** A type can refer to itself with `self`, and it now
  round-trips. Recursion through a list needs no wrapper (`list<self>` →
  `Vec<Self>`); a direct self-reference uses the new `Rec<T>` indirection
  (`neg(self)` → `Rec<Sexpr>`, `option<self>` → `Option<Rec<Cons>>`).
- **`Rec<T>`** (`packr_abi::Rec`, re-exported from `packr_guest`) — a heap
  indirection that round-trips through `Value`. `std::boxed::Box<T>` is
  `#[fundamental]` and so can never carry the ABI codec traits; `Rec<T>` is a
  packr-owned transparent wrapper over `Box<T>` that can, and unlike a bare
  `Box` it works in every position, including nested inside a container — which
  is what a **recursive struct** needs (`Option<Rec<Self>>`). It derefs to `T`,
  constructs with `Rec::new`/`From<T>`, and encodes byte-identically to the inner
  value (no wire cost). The `wit!` codegen emits it automatically.
- The `GraphValue` derive also handles a direct `Box<Self>` field (a boxed
  self-reference) by decoding the inner value and re-boxing.

### Notes

- Wire-compatible: `Rec<T>` is transparent, so recursive values encode exactly as
  their structure. No format change.
- A `Box` nested inside a container (`Option<Box<Self>>`) is still not supported —
  `Box` is `#[fundamental]`. Use `Rec<T>` there (the codegen does this for you).

## v0.13.1 (2026-08-05)

### Fixed

- **`wit!` (and host) codegen for user-defined records/variants/enums.** The
  generated code built tuple-form values (`Value::Record(vec)`,
  `Value::Variant { tag, payload }`) against the struct-variant `Value` enum and
  failed to compile once the types were used. Records/variants/enums are now
  generated with `#[derive(GraphValue)]` (the same path the host codegen uses),
  so their marshalling comes from the tested derive.
- **`option<...>` fields.** The derive now decodes fields via `FromValue` instead
  of `TryFrom`, so `Option<T>` fields work — including a generic `Option<T>`
  field (the gap left in v0.13.0). Behaviour is unchanged for every other type.
- **`packr-guest`'s `derive` feature is now on by default**, since generated code
  emits `#[derive(GraphValue)]` and so needs the derive to always be available.

## v0.13.0 (2026-08-05)

### Added

- **User-defined generics.** Pact now supports first-order, fully-applied generic
  types — parameterized `record`/`variant`/`type` (`record pair<a, b>`, recursive
  `variant tree<t>`, `type boxed<t> = list<t>`) — with arity checking and argument
  substitution in the resolver, on both the host pact parser and the guest WIT+
  code generator. See [`docs/generics.md`](docs/generics.md).
- **`#[derive(GraphValue)]` on generic types.** The derive now injects the bounds
  each type parameter needs to round-trip through the ABI, so deriving on a generic
  `struct Pair<A, B>` (or enum) works directly. packr ships the machinery, not a
  CRDT/data-structure library — define your own `OrSet<T>` etc. and derive.
- **Generic interfaces + compose-time unification.** An interface can be
  parameterized (`type s: constraint`), and composition can wire a generic side to
  a concrete one — e.g. an RSM node generic over its SM `state<s>` composed with a
  concrete SM that pins `s`. On an interface-hash mismatch, compose structurally
  unifies the generic signatures against the concrete side, binds every parameter
  consistently, and reconciles the link. Only generic↔concrete is supported.
- Interface-level `type_params` are embedded into `__pack_types` metadata (host and
  guest), so composition can identify which signature type-references are generic
  parameters. Emitted only for generic interfaces, so non-generic packages are
  byte-identical.
- End-to-end test (`tests/compose_generic.rs`) composing a real generic node with a
  concrete state machine on real wasm and asserting the value crosses the reconciled
  generic boundary.

### Notes

- **Type-parameter erasure keeps this wire-compatible.** Because the ABI is
  structural, a generic instantiation encodes byte-identically to the equivalent
  monomorphic type — there is no wire-format change and no monomorphization, so
  this ships as a backward-compatible minor and existing packages are unaffected.
- Not yet implemented: higher-kinded/const generics, constraint *enforcement*
  (constraints are carried but not checked), `wit!`-macro codegen of a generic
  component trait (hand-written components work today), and a generic `Option<T>`
  field in the derive.

## v0.12.7 (2026-08-02)

### Fixed
- **`packr compose` now strips internalized imports when `__pack_types` metadata is
  interior to a merged `.rodata` segment (LTO).** When the entry component is built
  with `lto = true`, the CGRF metadata blob is merged into the module's single
  `.rodata` data segment, so `__pack_types`'s baked-in DATA_ADDR is an *interior*
  offset (segment base + rel), not the start of a dedicated segment.
  `strip_internalized_imports_from_metadata` only matched a segment whose base
  EXACTLY equalled DATA_ADDR, so it bailed with *"no active data segment found at
  metadata address ..."* whenever the entry's sole import was fully internalized. It
  now finds the segment whose address range CONTAINS DATA_ADDR and slices/overwrites
  the metadata at `rel = DATA_ADDR - base` (the old exact match is just the
  `rel == 0` case) — fully backward-compatible for dedicated segments. Root-caused
  and patch-authored by mesh-dev against the mesh RSM SM-boundary (sm-smoke /
  sm-trivial). Regression test `tests/compose_lto_interior.rs` forces the interior
  layout deterministically and fails pre-fix with the exact error.

## v0.12.6 (2026-07-26)

### Fixed
- **`#[graph(crate = "...", forward_compatible)]` combined form now parses the crate
  correctly.** `get_crate_path` assumed `crate = "..."` was the entire `graph(...)`
  list, so a trailing arg (like `forward_compatible`) left the string not ending in a
  quote and it silently fell back to the default `packr_abi` crate — wrong for a guest
  using `packr_guest::composite_abi`. It now scans the comma-separated args, so the
  combined form works alongside any other `graph` arg. (Surfaced pairing on the
  0.12.5 forward_compatible adoption; the separate-attr form always worked.) Unit
  tests added in `pack-derive`.

## v0.12.5 (2026-07-26)

### Added
- **`#[graph(forward_compatible)]` on the `GraphValue` derive** — an opt-in,
  schema-evolution-tolerant record decode. On an opted-in struct a MISSING field
  defaults (instead of erroring) and an EXTRA field is ignored, so **appending a
  field is decode-safe in both directions**: an old build reads new data (the
  rollback case — no more orphaned store history) and a new build reads old data
  (retiring hand-written pad-missing migrations). Default (attr absent) keeps the
  strict field-count decode, so genuine field-count bugs still fail loud. Named
  structs match fields by name (add/remove/reorder tolerated); tuple structs decode
  positionally, so only appending a trailing field is safe. Encode is unchanged —
  no wire-format change. It ships schema-neutral first, then a field-add lands
  rollback-safe. Motivated by a persisted-store field-add that was a one-way door
  under the strict decode (an old, rolled-back build rejected the extra field and
  re-init'd empty). Tests: `crates/pack-abi/tests/derive_tests.rs`.

## v0.12.4 (2026-07-25)

### Fixed
- **`packr compose` now strips internalized interfaces from the composite's
  `__pack_types` metadata.** Compose correctly deleted an internalized interface's
  wasm imports (wiring them to the bridging shim) but left the interface declared as
  a REQUIRED import in the composite's `__pack_types` metadata. A theater loader that
  resolves handlers from that metadata then demanded a host handler for an interface
  that is satisfied internally, and failed actor setup: "No handler provides interface
  `mesh` required by actor". Now the composite's declared import surface matches its
  real residual (host-only) wasm imports — every interface internalized by a link
  whose consumer is the entry component is removed from the entry's `__pack_types`
  (the whole interface, by name, so it works even when the consumer declares more
  functions than it links, as a hash-checked link requires). Surfaced by loading the
  composed sentinel under a real theater handler set. Regression guard in
  `tests/compose_actor.rs` asserts the composite metadata carries only the residual
  host imports.

## v0.12.3 (2026-07-24)

### Fixed
- **The `packr` flake package (`packages.packr`/`.default`) now builds in a sealed
  nix sandbox and carries `binaryen` at runtime.** Two papercuts every nix consumer
  of `packr compose` hit (found by the sentinel compose-in-nix integration):
  - `doCheck = false` on the CLI package — the integration tests shell out to
    `wasm-merge` and build wasm fixtures, neither available in the buildRustPackage
    sandbox, so they failed the package build. The full suite still runs in CI
    (under `nix develop`, with binaryen present); building the CLI doesn't need it.
  - `packr` is wrapped so `binaryen` is on its PATH — `packr compose` shells out to
    `wasm-merge` at runtime, so consumers no longer need to add binaryen to their own
    build environment.

## v0.12.2 (2026-07-24)

### Added
- **`packr verify <wasm> --host-only`** — assert every import of a composite (or a
  plain self-contained actor) is a host function under `theater:simple/`, exiting
  non-zero and listing the offenders otherwise. A one-shot, first-class check for a
  build pipeline to gate that a composed actor is deployable — nothing cross-component
  left unsatisfied — instead of grep-parsing `wasm-tools print`. Requested by the
  sentinel compose-in-nix integration. Library entry point: `packr::verify::non_host_imports`.

## v0.12.1 (2026-07-24)

### Fixed
- **`#[import_from]` decodes a package import's return value via `FromValue`** (like
  `#[import]` and `#[export]` already do), instead of `TryFrom<Value>`. The composite
  ABI implements `FromValue` for nested `Option`/`Result` but not `TryFrom<Value>`, so
  every consumer of a `Result`- or `Option`-returning package function previously
  needed a hand-written `TryFrom<Value>` newtype shim just to call it. Now it decodes
  directly, no shim. Surfaced by the mesh-client pilot (its `mesh` functions return
  `result<...>`). Regression guard: `tests/compose_import_result.rs`.

### Changed
- `packr compose` now documents (in `--help`) and, on a missing component wasm,
  reports that each component's `wasm` path is resolved relative to the **manifest
  file's directory**, not the cwd — a papercut the pilot hit.

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
