# The pending/resume async-ABI — async host calls on any engine

**Status:** scope / design proposal · **Owner:** pack-dev · **Input:** theater-dev (id=293) + Colin
**Builds on:** `docs/engine-axis.md` (this is how the §4.2 host-import cluster works across *all* backends)

## 1. The problem

The host-import cluster we built uses wasmtime's `func_wrap_async`: when the guest calls a host import, **wasmtime suspends the wasm stack** while the host future runs, then resumes with the return value. The guest is written blocking-style and it works because *the engine can suspend the stack*.

Plain JS `WebAssembly` cannot do that — a wasm import is a synchronous call that must return now; you cannot park the wasm stack on a JS Promise. (That is exactly what **JSPI** adds, and JSPI is experimental/not universal.) So the async-host-fn abstraction has a hidden **native-only assumption: "the engine suspends my stack."** That assumption is the wall `packr-web` hits at the host-import cluster.

## 2. The reframe

**The guest is the whole wasm module** = actor code + the `packr-guest` runtime. So the guest can own a **single-threaded async executor and suspend *itself*** — the pause lives in the guest, not the engine. That makes async host calls possible on a synchronous engine, with no engine feature.

## 3. One host contract, three drivers

The host-fn registration we already built stays **uniform and unchanged**: a capture-based `Fn(Value) -> impl Future<Output = Result<Value>>` (see `packr_core::host`). What varies is *how the guest gets the result back across an await* — and that is a **driver** choice, not an ABI change:

| driver | engine | mechanism | guest build |
|---|---|---|---|
| **engine-suspend** | wasmtime (native) | trampoline awaits inline; engine suspends the guest stack | blocking-style, no executor |
| **JSPI** | browser (where available) | same, on a JS Promise | blocking-style |
| **resume/parking (async-ABI)** | **any** (browser without JSPI, and native) | guest parks the task, host re-enters with the result | executor-style |

Same actor **source** compiles to either guest build; it links a different `packr-guest` runtime variant and pairs with the matching trampoline. **Decision (Colin, 2026-09-07): the resume/parking async-ABI is the primary contract** — we skip investing further in engine-suspend as a destination and make pending/resume *the* mechanism, with engine-suspend/JSPI as optional perf drivers on top.

## 4. The wire ABI (concrete proposal)

Today's host-import ABI (from the built cluster): guest calls `import(in_ptr, in_len, out_ptr_slot, out_len_slot) -> status`, where `status` is `-1` error / `1` guest-owns-result. The async-ABI generalizes `status` into a **tag**:

### Guest → host (imports)
`import(in_ptr, in_len, out_ptr_slot, out_len_slot) -> tag`:
- **`READY`** — the host had the result synchronously. `(out_ptr_slot, out_len_slot)` hold the result buffer (guest-owned, as today). The guest shim resolves inline; no parking.
- **`PENDING`** — the host started async work. `out_ptr_slot` instead holds a **`completion_id: u32`**. The guest shim registers a waker keyed by `completion_id` and returns `Poll::Pending`.
- **`ERROR`** — as today.

Engine-suspend/JSPI drivers simply **never return `PENDING`** (they await inline, so the result is always ready) — which is why the same ABI degenerates cleanly to model A.

### Host → guest (exports the host drives)
- **the operation entry** (today's `__export_impl` path): runs the guest executor until the top-level task is either done or all in-flight tasks are `Pending`, then **returns, fully unwinding the stack** — task state lives in guest linear memory, held by the executor. Return tag: `DONE(result)` or `PENDING(top_level_completion_id)`.
- **`__pack_resume(completion_id, result_ptr, result_len) -> tag`** (new export): the host re-enters here when a host future resolves. The guest finds the parked task's waker by `completion_id`, stores the result, wakes it, and re-polls the executor to the next `Pending` or to completion. Returns `DONE(result)` / `PENDING` again.

So in model B **both directions become non-blocking at the wasm boundary**: an import poll-returns a tag, and an export runs-and-unwinds. All actual awaiting happens in the **host-side pump** *between* guest re-entries.

## 5. Component split & ownership

Grounded in the current code:

- **`packr-guest`** (`crates/pack-guest/src/lib.rs`) — *the heart of model B, pack-dev's:*
  - a single-threaded **executor** + **waker registry keyed by `completion_id`**.
  - reshape **`__import_impl`** (today blocking: call import, read status, decode) into an **`async` shim**: call the import; on `READY` decode + return; on `PENDING` register the waker and yield `Poll::Pending`.
  - reshape **`__export_impl`** (today sync) to spawn `f` as a task and drive the executor, returning `DONE`/`PENDING`.
  - add the **`__pack_resume`** export.
  - This is a new `setup_guest!` runtime variant (executor-style) alongside today's blocking one.

- **`packr-core`** (`crates/pack-core/src/host.rs`) — backend-free, shared:
  - a **completion registry**: `completion_id -> in-flight host future`.
  - **`dispatch_host_import`** (already exists) changes from *await-inline* to **poll-once**: invoke the registered `HostFn`; if `Ready` → write result, return `READY`; else stash under a fresh `completion_id` → return `PENDING(id)`.
  - the **resume-driver pump**: the export call path (`call_with_value`) becomes a loop — call the entry; while `PENDING`, await an in-flight host future to completion, write its result via `HostCallCtx::alloc`, call `__pack_resume`, repeat until `DONE`.

- **backend** (`packr-wasmtime` / `packr-web`) — minimal:
  - `HostCallCtx` (read/write guest memory + async alloc) — **already built**.
  - "invoke a guest export" (`call` / `__pack_resume`) — `call` **already built**.
  - be driven by the host async runtime for the pump. **wasmtime** drives `__pack_resume` via a normal `Func::call_async`; **packr-web** drives it from the JS event loop / microtask queue — **no JSPI**.
  - NOTE: with poll-once dispatch, wasmtime host imports no longer need `func_wrap_async` — a plain sync `func_wrap` suffices (the future is stashed, not awaited inline). The awaiting moves entirely to the pump.

- **theater / `pack_bridge`** — **nothing changes.** It registers capture-based host fns returning futures; whether they are awaited-inline (model A) or pumped-via-resume (model B) is entirely below its line. Zero-diff consumed surface across all three drivers — the property that makes this worth doing.

## 6. The Send question, localized

The deferred host-path Send question (`CallInterceptor` / `HostCallCtx` / `HostFn`) rides here: in model B the **completion registry holds in-flight host futures on the host side**, and the pump awaits them. Their `Send`-ness is `HostFn`'s `Send`-ness — a **per-driver** property (native pump: `Send`; web pump: `!Send`). So this is where theater-dev's "Send pushed to the per-environment driver" pattern lands concretely, rather than a hard `Send` in the shared core. Resolve it as part of building the pump (still theater-dev's pattern; the async-ABI doesn't worsen it and gives it a natural home).

## 7. Build order (theater-dev's rec, adopted)

1. **Design packr-core's host-import ABI around pending/resume now** — so engine-suspend, JSPI, and async-ABI are all just *drivers* of one contract (rather than the ABI being locked to "engine suspends my stack").
2. **Build the resume/async-ABI path first, and validate it on `packr-wasmtime`** — wasmtime can drive `__pack_resume` via a sync `call_async`, so the *entire* protocol (poll-once dispatch → PENDING → pump → `__pack_resume` → DONE) is testable **natively, in-container, before `packr-web` needs a browser.** This is the single most valuable property of the plan: the hard new protocol gets proven where we have full tooling.
3. Keep wasmtime on engine-suspend afterward if we want the per-call perf (both satisfy the same host-fn contract), or run it on resume too.
4. **`packr-web` async-ABI driver** — the universal browser answer, no engine feature. MUST-HAVE.
5. **JSPI driver** — optional later perf/density driver where JSPI exists. Same source, different guest build.

The old "sync-only limited browser" fallback is obsolete — model B gives full `.await` everywhere.

## 8. What this reshapes

- The built host-import cluster (model A, `func_wrap_async` engine-suspend) becomes **one driver**, not the destination. Its wire `status` widens to the `READY`/`PENDING` tag; engine-suspend simply never emits `PENDING`.
- `packr-guest` gains a second, executor-style runtime variant. Actor source is unchanged; it's a build-target choice.

## 9. Open questions

1. **Tag encoding** — reuse the `i32` return (`-1`/`READY`/`PENDING` as distinct values, with `completion_id` in `out_ptr_slot` on `PENDING`) vs. a small out-param struct. Lean: widen the existing `i32` status — minimal churn, keeps the slot machinery.
2. **`completion_id` allocation & lifetime** — host-side monotonic `u32`; freed when the task resumes. Guest keeps a `completion_id -> waker` map; host keeps `completion_id -> future`. Two halves of the same id space.
3. **Nested/concurrent host calls** — an actor task may await several host calls; multiple `completion_id`s outstanding for one top-level export call. The pump must drive *all* of them (a `select`/join over the registry), not just one. This is the main correctness subtlety.
4. **Guest executor choice** — a minimal hand-rolled single-threaded executor (no deps) vs. a `futures`-based `LocalPool`. Lean: minimal hand-rolled, to keep the guest small and dependency-free.
5. **Cancellation / actor teardown** mid-flight — what happens to parked tasks + in-flight host futures if the actor is stopped between resumes. Needs a defined drop path on both sides.
6. **Interaction with the interceptor** — record/replay must capture the *logical* host call (input → eventual output), not the pending/ready mechanics. `before_import`/`after_import` fire at dispatch and at completion respectively; replay short-circuits to `READY`.

## 10. TL;DR

Make **pending/resume the host-import contract**: the guest owns an executor and parks itself; the host polls host-fns once and pumps their completions back in via `__pack_resume`. Async host calls then work on **any** engine with **no** engine feature. `packr-guest` owns the executor + resume; `packr-core` owns the registry + pump; backends just invoke exports + provide memory/alloc (already built); **theater is untouched.** Build the resume path first and prove it on wasmtime before the browser — the whole protocol is validatable natively.
