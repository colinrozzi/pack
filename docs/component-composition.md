# Component composition (packr's Component-Model equivalent)

Status: **design / M1 in progress.** Rust-first.

## Goal

packr's answer to the WebAssembly Component Model: a package is a standalone
wasm entity, and packages can be **composed** into a new package — a single
distributable binary — with all wiring done at the wasm level, over a
language-neutral value ABI (the Graph ABI). The differentiator vs. the standard
Component Model is packr's reason to exist: **recursive value types** (ASTs,
S-expressions, JSON/DOM trees), which standard WIT forbids.

We build our own rather than adopt the Component Model precisely because that
incompatibility is structural, not incidental — the canonical ABI is built on
non-recursive types by construction. We already own the hard, differentiated
core (the Graph ABI: graph-encoded values with DAG/cycle handling), so this is
building a composition layer on a value ABI we have, not a component model from
scratch.

Scope discipline: **build the composition MODEL + the wiring, Rust-first.**
Per-language bindgen is a separate, demand-driven axis — add a language only when
there's an actor in it. One language (Rust) proves the whole model.

## What we are NOT doing

This is **not** the fusion/compose machinery retired in 0.11.0. That merged two
finished binaries into one **shared-memory** module and forced stack-pointer /
data / table reconciliation — the source of the resource-collision bug class.
Composition here keeps every component's memory **separate** (see below), so
there is nothing to reconcile.

A single actor remains a plain `cargo build` (the 0.11.0 model is untouched).
Composition is an **opt-in** build step, only when combining packages.

## Concepts

- **Component** — a plain wasm module (a 0.11.0 actor: exports `memory` +
  `__pack_alloc`/`__pack_free` + its pact functions; imports what it needs) plus
  its **pact** (imports/exports contract). Atomic (one module) or composite
  (several wired modules), but it presents **one** interface to whoever
  instantiates it; internal wiring is hidden.
- **Link** — `A.import(iface) ← B.export(iface)`, **hash-checked** via packr's
  Merkle interface hashing (a link is valid iff the interface hashes match). This
  is `packr link`'s good idea (typed, hash-checked wiring) kept; only its wrong
  mechanism (binary fusion) is dropped.
- **Composite** — {components} + {links} + residual host imports (provided at
  instantiate) + the composite's exports.

## The hinge: a single **multi-memory** module

The composite is one wasm module — but with **one memory per component**.
`app` → memory 0, `math` → memory 1, and shims copy across the gap. This is the
one detail that makes "single binary" and "isolated components" both true:

- The retired fusion **unified** memories → shared address space → reconciliation
  → bugs.
- Composition **keeps memories separate** → each component's `__stack_pointer`,
  data, heap live in its own memory → they **cannot collide** → the whole
  reconciliation bug class is structurally impossible, not merely fixed.

theater loads the composite as a normal actor: it exports memory 0 as `"memory"`,
exports the entry component's `__pack_alloc`/`__pack_free` + pact functions, and
imports only host functions. theater never sees the second memory. **Zero theater
changes.** packr already enables `wasm_multi_memory`.

Tables and globals stay separate too (multi-table + independent globals) — a
global only collides if it indexes shared memory, and nothing is shared. So there
is nothing to unify.

## Bridging shims

Each link gets a **statically-generated** shim function — pure codegen from the
pact signature. Say `app` (memory 0) imports `double: func(n: s64) -> s64`,
satisfied by `math`'s export (memory 1). The shim `__bridge_double` **is** `app`'s
import; when `app` calls it with `(in_ptr, in_len, out_ptr_ptr, out_len_ptr)`
(pointers into memory 0):

1. `math.__pack_alloc(in_len)` → `mptr` (memory 1).
2. `memory.copy` `in_len` bytes: memory 0 `[in_ptr]` → memory 1 `[mptr]`.
   (Multi-memory lets one instruction name both memories.)
3. Call `math`'s exported `double(mptr, in_len, …)` — math runs entirely in
   memory 1, own allocator/stack/data, unaware it isn't top-level.
4. Read math's result ptr/len from memory 1.
5. `app.__pack_alloc(result_len)` → `aptr` (memory 0).
6. `memory.copy` result: memory 1 → memory 0 `[aptr]`.
7. `math.__pack_free(...)`; write `aptr`/`alen` into memory 0 at the out slots;
   return.

It is exactly the marshalling theater already does host↔actor, instance-to-
instance, with an explicit `memory.copy` because the sides are different
memories. Both components use the identical pact ABI, so the shim calls `math`
the way theater calls any actor. Neither component's code is modified.

## The compose transform (build-time)

A `walrus` pass builds the composite:

1. Place `math`'s code with its memory as **index 1** — remap math's memory
   references to memory 1 (same shape as the retired `MemRemap`, but *redirecting*
   to a second memory rather than *unifying* into one). Keep math's table/globals
   as its own.
2. **Emit each shim** from its link's pact signature (alloc → copy → call → copy
   → free); rewrite the consumer's import to call it.
3. Export the entry component's `memory` / `__pack_alloc` / `__pack_free` / pact
   functions as the composite's surface; residual imports (host) pass through.

## Milestone 1 (acceptance)

Two toy Rust components — **`app`** imports `double: func(n: s64) -> s64` and
calls it; **`math`** exports `double` — composed into one multi-memory binary.
Instantiate it in the packr runtime, call `app`'s entry, and assert:

- the result came back **doubled**, and
- `app` and `math` are in **separate memories** (independent — the composite has
  two memories; neither's addresses collide).

Green = the model works end-to-end.

## Non-goals for M1

Other languages · async cross-component calls (M1 is synchronous; the shim is
straight-line) · resource/handle types · the single-file "packr component"
container format (a manifest + modules is enough to prove wiring; packaging is a
follow-on) · composing already-running actors (M1 instantiates a fresh
composite).

## Roadmap after M1

1. Async shims (step 3 can await — same pattern as the host-call path; the epoch
   guardrail keeps a runaway in any wired instance killable).
2. A single-file composite artifact / packaging.
3. A second language: a Graph-ABI codec + a `pact codegen` backend for it,
   composed with a Rust component — the moment "any language that produces wasm
   interoperates" becomes real rather than aspirational.

## Milestone 3: async component composition

**Status: DONE.** Composition works when a component does an ASYNC host call
(suspends). Acceptance test: `tests/compose_async.rs`, fixture
`packages/comp-async-math` (exports `math.double`, calls a residual `host.tick`
before returning `n*2`), composed against `comp-app` (entry) and run under the
`AsyncRuntime` with `tick` provided as a genuinely-awaiting host fn.

### The link shim is async-transparent (the hypothesis held)

The bridging shim (link shims *and* the host-bridge shim below) is plain
**synchronous** wasm — alloc → `memory.copy` → call → `memory.copy` → free, no
`await`. It needs no async-specific machinery because wasmtime's async support
suspends the **entire fiber** at an async host-import boundary: the whole wasm
call stack (entry frame → link shim → provider frame) and every component memory
freeze together and resume together. The suspend happens transparently *below*
the shim; the shim is just straight-line wasm on either side of a call that
happens to yield. So a provider that suspends on an async host import composes
with the **unchanged** sync link shim. (The "async shims" roadmap item — a shim
that itself awaits — is not needed for this; the fiber-level suspend subsumes it.)

### What DID need fixing: residual host imports from a non-entry component

A residual host import (one no link satisfies — the host provides it at
instantiate) that is declared by a **non-entry** component pointed at the wrong
memory. The provider calls `host.tick(in_ptr, in_len, …)` with pointers into its
**own** memory (memory 1), but the host resolves the guest memory + allocator
from the composite's canonical `memory`/`__pack_alloc` exports — i.e. the
**entry's** memory 0. So the host read the call's args from memory 0 and decoded
garbage (`Invalid magic`), then the guest hung on the error path. This is a
memory-routing bug, **not** an async bug: it would bite a synchronous residual
host import from a non-entry component just the same; the async fixture merely
surfaced it (M1/M2 had no non-entry residual host import).

The fix (in `compose.rs`) is a **host-bridge shim**, the exact mirror of the link
shim: for each non-entry component's residual host import, emit a shim that
copies the call's input from the component's memory into the entry's memory (via
the entry allocator), calls the **real** host import (which the host serves
against the entry memory — now correct), then copies the host's result back into
the component's memory. The residual import is **kept** (it is the composite's
residual surface, filled by the host); only the component's own call sites are
rewired to the bridge, and the bridge itself still calls the real import. An
entry component's own residual host imports need no bridge — they already target
memory 0.

## Reuse (not from scratch)

Graph ABI codec · the actor ABI (`in_ptr/in_len/out…`) · theater's host↔actor
marshalling pattern · interface hashing (hash-checked links) · the runtime +
epoch guardrail. New code: the compose transform (multi-memory + shim emission)
and the manifest.
