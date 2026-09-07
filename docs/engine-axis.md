# The Engine Axis — making packr environment-agnostic

**Status:** design ratified, pre-code · **Owner:** pack-dev · **Reviewers:** Colin (direction), theater-dev (host consumer)
**Companion:** theater's `docs/portability-runtime.md` (the *concurrency* axis; this doc is its *engine* counterpart)

> **Decisions locked (2026-09-06, Colin):**
> - **async-first** — the sync `Runtime` / `func_typed` / fixed-32 KB-return path is *removed*, not ported. No compatibility audit: we design for the target, not for today's consumers.
> - **crate-per-backend** — a backend-free core crate + one crate per backend.
> - **no compile cache in v1** — dropped for now; reintroduced once the stack runs end to end.
> - **naming (pack-dev's call):** `packr-core` (agnostic runtime + traits), `packr-wasmtime` (native backend), `packr-web` (browser backend, later); the top `packr` crate becomes a default-wasmtime facade so existing `packr::…` consumers don't move. See §4.6.

## 1. Goal

Make the whole stack — packr and the theater core built on it — **environment-agnostic**, so an actor can run anywhere the substrate allows: a native OS process, a browser tab, an embedded target. The way we get there is to **separate the core (the actor model, supervision, chain/event log, the graph ABI) from the actual execution of actors** (how wasm is compiled, instantiated, and run, and how work is scheduled).

theater-dev framed this as two orthogonal axes that meet at exactly one seam, `pack_bridge`:

| Axis | Owner | What it abstracts | Status |
|---|---|---|---|
| **Concurrency** | theater-dev | how actor loops are *scheduled* (`TheaterRuntime<E: Spawn>` + per-env driver crate) | **done** (#190 spawn seam, #192 crate split, #191 clock/metrics rip-out) |
| **Engine** | **pack-dev** | how wasm is *executed* (compile / instantiate / call / memory / preemption) | **this doc** |

theater-core cannot be built or validated as wasm until the engine axis exists: the only authoritative proof that the core is portable is an actual wasm cross-compile, and that needs an engine to compile against. So this axis is currently the critical path for theater portability.

## 2. The boundary decision (confirmed)

Two candidate boundaries for the backend abstraction:

- **(A) Low boundary — abstract only the raw wasm primitives.** compile / instantiate / call-export / linear-memory read+write+grow / host-import registration. packr's *ABI marshalling* — graph encode/decode, the `pack:alloc` protocol, the output-buffer / guest-allocate return path, the interceptor hooks — stays **above** the line, written once, shared by every backend. wasmtime is one impl; the browser's JS `WebAssembly` is the next.
- **(B) High boundary — abstract at the Value / `AsyncInstance` level.** Each backend re-implements the ABI marshalling underneath.

**Decision: (A).** The graph ABI *is* the crown jewel; it must never fork per environment or we reintroduce the exact "two copies drift" failure this whole system fights. (A) keeps a single ABI and swaps only the execution substrate. It is more upfront abstraction work, but it is the only version where "run anywhere" does not multiply the correctness surface.

The rest of this doc is the concrete shape of (A).

## 3. Why (A) is tractable: the call path is already pure ABI orchestration

The load-bearing observation. `AsyncInstance::call_with_value_async` (`src/runtime/mod.rs:641`) — the function theater actually calls for every actor invocation — is already almost entirely backend-free ABI logic:

```
encode(input)                          ← ABI            (stays above the line)
call_pack_alloc_async → in_ptr         ← call a guest export   ┐
memory.write(in_ptr, bytes)            ← write memory          │  backend
get_typed_func(name).call_async(...)   ← call a guest export   │  primitives
memory.read(RESULT_PTR/LEN slots)      ← read memory           │  (the only
call_pack_free_async                   ← call a guest export   ┘  wasmtime bits)
decode(out bytes)                      ← ABI            (stays)
interceptor.before/after_export        ← ABI-level      (stays; already generic)
```

The same is true of `types()` / `types_with_hashes()` (`mod.rs:798`, `:847`), the `pack:alloc` protocol (`call_pack_alloc_async` `:744`, `call_pack_free_async` `:763`), and the fixed-buffer/`__pack_alloc` fallback dance. Every one of them decomposes into **~5 backend primitives** with ABI logic on top. The extraction moves very little crown-jewel code; it re-expresses the existing orchestration in terms of a trait.

## 4. The trait design

Two clusters fall out of the two directions data crosses the boundary.

### 4.1 Execution (host → guest) — the mechanical cluster

```rust
/// A wasm engine: compiles bytes into modules and owns any compile cache.
#[async_trait]
trait WasmEngine: Send + Sync {
    type Module: Send + Sync + Clone;   // cheap-clone handle (wasmtime::Module is; JS WebAssembly.Module is)
    type Instance: WasmInstance;

    async fn compile(&self, bytes: &[u8]) -> Result<Self::Module, EngineError>;

    /// Instantiate with host imports + optional interceptor. `imports` is the
    /// backend-neutral registrar (§4.2). Returns a live instance.
    async fn instantiate(
        &self,
        module: &Self::Module,
        imports: HostImports,               // backend-neutral (see §4.2)
        interceptor: Option<Arc<dyn CallInterceptor>>,
    ) -> Result<Self::Instance, EngineError>;
}

/// A live instance: call exports, touch linear memory, arm preemption.
#[async_trait]
trait WasmInstance: Send {
    /// Call an exported function. All packr guest calls are i32/i64-typed
    /// (the ABI convention is (i32,i32,i32,i32)->i32 etc.), so `Val` is a
    /// small numeric enum, not a general value type.
    async fn call(&mut self, name: &str, args: &[Val]) -> Result<Vec<Val>, EngineError>;

    fn mem_read(&self, off: usize, buf: &mut [u8]) -> Result<(), EngineError>;
    fn mem_write(&mut self, off: usize, data: &[u8]) -> Result<(), EngineError>;
    fn mem_size(&self) -> usize;
    fn mem_grow(&mut self, pages: u64) -> Result<(), EngineError>;

    fn has_export(&self, name: &str) -> bool;
    fn export_names(&self) -> Vec<String>;

    /// Preemption — §4.4. No-op on backends without a native mechanism.
    fn set_deadline(&mut self, ticks_until_trap: u64);
}
```

Once these exist, `call_with_value_async`, `types()`, `call_pack_alloc_async`, `write_memory`/`read_memory`/`read_value`/`write_value`, and `set_epoch_deadline` all rewrite in terms of them, **once**, in a backend-free module.

### 4.2 Host imports (guest → host) — the hard cluster

This is where `src/runtime/host.rs` (~57 KB) lives, and it's the real work. It's the reverse direction: the guest calls a host function, which must (a) read args out of guest memory, (b) possibly re-enter the guest to `__pack_alloc` its return (the guest-allocate path — theater's large returns: `get-chain`, `store.get`, `wat-to-wasm`), and (c) write the return back.

The good news: the memory-access seam is **already abstracted** into `Ctx` (`host.rs:169`). Everything funnels through `Ctx::resolve_memory()` and the three methods `read_value` / `write_value_at` / `read_string`. The only genuinely wasmtime-typed pieces of `Ctx` are the `caller: Caller` + `memory: Option<Memory>` fields and the `data()/data_mut()` store accessors.

**Plan: make `Ctx` backend-generic.**

```rust
/// Backend-neutral host-call context. Replaces the wasmtime Caller+Memory pair.
struct Ctx<'a, T> {
    store: &'a mut dyn StoreAccess<T>,   // data() / data_mut()
    mem:   &'a mut dyn GuestMemory,      // read()/write()/grow()/size()
    // + re-entry handle so an async host fn can call __pack_alloc (see below)
    alloc: &'a mut dyn GuestReentry,
}
```

`read_value` / `write_value_at` / `read_string` are unchanged except they call `self.mem` instead of `self.caller` + `resolve_memory()`. `HostLinkerBuilder` / `InterfaceBuilder` / `func_typed` / `func_async` / `func_async_result` register their closures through a backend `HostImports` registrar:
- **wasmtime:** `HostImports` wraps a `Linker<T>`; closures get a `Ctx` built from `Caller`.
- **browser:** `HostImports` builds an `importObject` of JS functions closing over the `WebAssembly.Memory`; closures get a `Ctx` built from that memory + a JS re-entry shim.

The `pack:alloc` default provider (`register_default_alloc`, `mod.rs:366`) is registered the same neutral way — and critically stays a **raw, non-intercepted link** (theater's constraint: keep alloc off the chain log), which the `HostImports` registrar preserves as a first-class distinction from the typed/intercepted interface functions.

### 4.3 What stays above the line, untouched

- The graph ABI (`crates/pack-abi`): `encode` / `decode`, `Value`, `Pattern`, the pact parser, value-literal syntax. Zero change.
- `CallInterceptor` (`src/runtime/interceptor.rs`): already 100% backend-agnostic — operates on `Value` / `&str`. **Caveat that pushes §4.5:** its *sync* bridge drives interceptor futures via `tokio::task::block_in_place` + the current `Handle`. That's native-only. The async bridges `.await` directly and are fine everywhere.
- The `pack:alloc` protocol, output-buffer offsets, guest-allocate return semantics — all ABI-level, all above the line.
- Metadata (`__pack_types` decode), interface-hash checking, `assert_self_contained`.

### 4.4 Preemption is a capability, not an assumption

Today preemption is wasmtime epoch interruption: `AsyncRuntime::new` builds the engine with `config.epoch_interruption(true)` (`mod.rs:290`); `AsyncInstance::set_epoch_deadline` (`mod.rs:573`) arms it; a host ticker calls `engine.increment_epoch()`.

Under (A) this becomes `WasmInstance::set_deadline`, whose meaning is backend-defined:
- **wasmtime:** epoch deadline (today's behavior, unchanged).
- **browser:** actors run in Workers; preemption is `Worker.terminate()`. `set_deadline` maps to the driver's watchdog-terminate, or is a no-op if the driver owns the timeout entirely.
- **bare/embedded:** documented no-op (accept un-interruptible guests, or rely on a fuel counter if the backend has one).

Corollary (theater-dev's note): theater's **init-watchdog exists only** to make an un-interruptible wasmtime hang *legible*. A backend with native preemption (`Worker.terminate`) doesn't need it — so it's a native-driver diagnostic, not a core feature.

### 4.5 async-first, single call path (locked)

**The trait is async-first; the sync execution/host-fn path is removed, not ported.** Rationale:
1. JS `WebAssembly.instantiate` and any await-into-guest is inherently Promise-based; a sync path can't wrap it.
2. Async subsumes the guest-allocate return path (the id=9 fix): an async host fn can `.await` back into `__pack_alloc` for unbounded returns. A sync host fn structurally cannot (it's why the 32 KB fixed buffer exists) — so async-first also *deletes the 32 KB host-return cap* as a side effect.
3. It deletes the `block_in_place` interceptor-driving hack (§4.3) and the tokio dependency it implies — which the browser backend can't satisfy anyway.
4. theater's entire host surface is already async (`func_async_result`), and `call_with_value_async` is the only export path it uses — so the removal is a no-op for theater.

The sync `Runtime` / `func_typed` / `func_typed_result` / fixed-32 KB-return path is deleted. Colin's call (2026-09-06): **no compatibility audit** — "let's not let where we are right now determine the future." Any non-theater sync consumer migrates to the async path or is dropped; we design for the target shape, not the current one.

### 4.6 Crate layout & naming

Backend-free core, one crate per backend, thin facade for continuity:

```
pack-abi        (exists)   values + graph wire + pact parser + value-literal syntax. Unchanged.
packr-core      (new)      the environment-agnostic runtime:
                             - traits: WasmEngine, WasmInstance, GuestMemory, HostImports, GuestReentry, Val, EngineError
                             - Ctx (backend-generic), HostLinkerBuilder / InterfaceBuilder
                             - ABI orchestration: the call path, pack:alloc protocol, __pack_types metadata
                             - CallInterceptor (moved here, unchanged)
                           depends only on pack-abi. No wasmtime, no JS.
packr-wasmtime  (new)      implements the traits over wasmtime. Native.
packr-web       (new,later) implements the traits over JS WebAssembly (js-sys/web-sys). Browser.
packr           (exists)   CLI + convenience facade: re-exports packr-core and defaults the backend to
                           packr-wasmtime, so existing `packr::…` consumers (theater) don't move.
```

Dependency DAG (no cycles): `packr-{wasmtime,web}` → `packr-core` → `pack-abi`; `packr` → `packr-core` + `packr-wasmtime`. Mirrors theater's `theater` (core) / `theater-native` (driver) split.

Note: for v1 the `WasmEngine`/`WasmInstance` traits live *in* `packr-core`, so a backend crate depends on `packr-core` and pulls the (dead-code-eliminable) orchestration into its tree. If browser bundle size later demands it, extract a thin `packr-backend` trait-only crate that both `packr-core` and the backends depend on. Not worth it in v1.

**Compile cache:** dropped for v1. `WasmEngine::compile` just compiles every time. Reintroduced later as backend-owned state behind the same trait method (theater's higher-level module cache is unaffected either way).

## 5. Migration order

Staged so the async wasmtime path is provably equivalent to today before any new backend exists. Steps 1–4 keep wasmtime as the only backend; the guest ABI never changes.

1. **Define the traits in `packr-core`** (§4.1) + carve the backend-free ABI orchestration + `CallInterceptor` into it. `packr-wasmtime` implements the traits; the top `packr` crate re-exports core defaulted to wasmtime. Async-only from the start — the sync path is dropped here, not ported (§4.5). Existing *async* tests (`tests/host_functions.rs`, `tests/compose_async.rs`, round-trip + pact-parse suites) stay green; sync-only tests are ported to async or removed. This is the big, careful refactor — the bulk of the work.
2. **Genericize `Ctx` + `HostImports`** (§4.2) with wasmtime still the only backend. Green tests throughout.
3. **Drop the compile cache** (§4.6) — `compile` recompiles each call; simplifies the `WasmEngine` surface for v1.
4. **Prove the async wasmtime path end to end** against the real fleet consumers (theater's `pack_bridge`) — no behavior change from today.
5. **Stand up `packr-web`** — the JS `WebAssembly` backend over `js-sys`/`web-sys`. This is where the genuinely new work is; everything above is refactor.
6. **theater cross-compiles to wasm against `packr-web`** — the authoritative portability proof theater-dev is blocked on.

## 6. Blast radius / consumer impact

- **theater (host):** Steps 1–2 are internal to packr; theater's consumed surface (`instantiate_with_host_and_interceptor_async`, `HostLinkerBuilder`, `CallInterceptor`, `call_function_with_value`, `get_exports`/`get_export_hashes`/`has_export`, `decode_function_result`) is preserved. Step 3 (async-first) is a no-op for theater (already all-async). Step 5 is theater's own work on its side of `pack_bridge`. Net: **no theater code change for 1–3**; the change theater *feels* is that its two `pack_bridge.rs` instantiate sites get a backend type parameter (or a defaulted-to-wasmtime alias so even that is zero-diff).
- **Guests (`packr-guest`):** **zero.** The guest ABI (memory layout, `__pack_alloc`/`__pack_free`, `__pack_types`, export signatures) is unchanged — a guest `.wasm` runs identically on any backend. This is the whole point of keeping the ABI above the line.
- **crates.io release:** steps 1–2 are additive-ish but touch the public runtime API (new trait params); likely a minor-with-care or major bump. **No wire/ABI break**, so no fleet actor rebuild is forced — but per the standing gate, any release + theater pin-bump waits on Colin's explicit go, and I'll flag the exact semver + surface delta in the release note.

## 7. Decisions (resolved) & remaining unknowns

Resolved with Colin (2026-09-06):
1. **Async-first** — yes, sync path removed, no audit (§4.5).
2. **Backend selection** — crate-per-backend (§4.6); a `WasmEngine` type param in core, cargo features only to gate which impl compiles (embedded/browser won't want wasmtime in the tree).
3. **Compile cache** — dropped for v1 (§4.6).
4. **Naming** — `packr-core` / `packr-wasmtime` / `packr-web`; `packr` as default-wasmtime facade (§4.6).

Remaining, to settle during implementation (not blocking the start):
- **`Val` breadth:** packr guest calls are all i32 today (+ alloc i32→i32). Keep `Val` minimal (i32/i64) and widen only if an export boundary demands it.
- **Async trait mechanism:** `async-trait` (allocates per call) vs. hand-written `Pin<Box<dyn Future>>` vs. RPITIT once MSRV allows. Start with `async-trait` for readability; optimize if it shows up in a profile.
- **`packr-web` re-entry shim:** the exact mechanism for an async host fn to `.await` back into guest `__pack_alloc` under JS (call_async re-entrancy analog). Deferred to step 5.

## 8. TL;DR

The ABI is already cleanly separable from execution — the call path is pure ABI orchestration over ~5 primitives, and the host-call memory seam is already isolated in `Ctx`. The engine axis is therefore mostly a careful refactor (wasmtime behind a trait, `Ctx` genericized), plus one genuinely new artifact (the JS `WebAssembly` backend). One ABI, many substrates. The single decision that needs ratifying up front is **async-first** (delete the sync path); everything else is staged so wasmtime behavior is provably unchanged at every step.
