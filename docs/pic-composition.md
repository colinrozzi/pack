# PIC Package Composition — Design Spec

Status: **DRAFT / spec** (2026-07-12). Proposes porting `CompositionBuilder` onto
the 0.8.x PIC dynamic-linking loader so that composed packages share **one** linear
memory, and adds a two-package example. Supersedes the level-1 design in
[`package-linking-exploration.md`](./package-linking-exploration.md).

---

## 1. Goal

Let package **A** import and call package **B**'s functions across the wasm
boundary, and ultimately ship a set of composed packages as **one passable
`.wasm`** (static composition). This spec covers the concrete next step toward
that, building directly on the shared-memory substrate shipped in packr 0.8.0.

Three levels, in increasing "static"-ness:

| Level | What | State |
|---|---|---|
| **1. Runtime composition** | N packages, each its own memory; cross-calls copy bytes between memories. | **Exists today** (`CompositionBuilder`, `adder`+`doubler`). |
| **2. PIC shared-memory composition** | N packages share **one** memory via PIC dynamic linking; cross-calls pass pointers directly (no copy). | **This spec** — reuses the 0.8.0 loader. |
| **3. Static merge → one `.wasm`** | Merge the PIC side modules into a single relocatable artifact. | Future; the PIC side modules are its input. |

---

## 2. Where we are

### 2.1 Level 1 today — `CompositionBuilder` (runtime, cross-memory bridge)

`tests/composition.rs` + `packages/{adder,doubler}` demonstrate a working
level-1 composition. `adder` declares `imports { math { double } }` and calls it
as a local fn; the host wires it:

```rust
let mut comp = CompositionBuilder::new()
    .add_package("doubler", doubler_wasm)
    .add_package("adder", adder_wasm)
    .wire("adder", "math", "double", "doubler", "double") // A's import → B's export
    .build()?;
comp.call("adder", "process", &Value::S64(5))?; // → 11 (adder called doubler's double(5)=10, +1)
```

**How it works (and its cost).** `build()` instantiates each package in its **own**
`Store` with its **own** memory and its **own** bump allocator
(`register_default_alloc`). A cross-package import is satisfied by a `func_wrap`
shim, `cross_package_call`, that:

1. reads the encoded input bytes out of **A**'s memory,
2. re-allocates + writes them into **B**'s memory, calls B's export,
3. reads the encoded result out of **B**'s memory,
4. copies it back into **A**'s memory.

It works, but every cross-call is a **byte copy across two isolated memories**,
each package carries its **own allocator**, and the result is **N runtime
instances**, not one artifact. That's the thing PIC removes.

### 2.2 The 0.8.0 substrate — the PIC loader (allocator + **one** package)

packr 0.8.0 ships a mini dynamic linker (`pic_link` / `instantiate_pic` in
`src/runtime/mod.rs`). For {allocator + **one** package} it:

- creates **one** shared `Memory` + `Table` per instance,
- instantiates a shared **in-wasm allocator** side module (`assets/pack_alloc_module.wasm`),
- instantiates the package as a PIC side module at a **disjoint `__memory_base`**
  with its **own `__stack_pointer`** region, wired to the shared allocator via
  `pack:alloc`,
- resolves `GOT.mem.__data_end`, runs `__wasm_apply_data_relocs` then
  `__wasm_call_ctors`.

Proven by `tests/pic_composition.rs`: a real package round-trips every value
shape through the **shared** in-wasm allocator, with the disjoint-base +
data-relocation machinery working. 0.8.1 adds `assert_pic_module()`, so any
non-PIC input is rejected at instantiate.

**The observation this spec rests on:** "allocator + one package sharing one
memory" and "package A + package B sharing one memory" are the *same mechanism* —
disjoint bases in one shared memory, one shared allocator. Level 2 is
generalizing the loader from 1 package to N.

---

## 3. Design — Level 2: PIC `CompositionBuilder`

Keep the `CompositionBuilder` API (`add_package` / `wire` / `build` / `call`);
swap the backend from "separate `Store`s + `cross_package_call` bridge" to "one
shared-memory PIC loader."

### 3.1 One shared memory, N packages

```
shared linear memory (one Memory, one Table):

  [0,            ALLOC_BASE)      low guard (addr 0 never used)
  [ALLOC_BASE,   PKG_A_BASE)      allocator BSS / mstate
  [PKG_A_BASE,   A_STACK_TOP)     package A: static data (low) + stack (grows down)
  [PKG_B_BASE,   B_STACK_TOP)     package B: static data (low) + stack (grows down)
  ...            ...              one region per additional package
  [HEAP_BASE,    HEAP_END)        shared heap, owned by the one allocator
```

- The loader assigns each package a **disjoint `__memory_base`** and a **disjoint
  stack region** (`__stack_pointer` at that region's top), exactly as the
  single-package case does — just N of them instead of one.
- **One** allocator serves **all** packages: every package's `pack:alloc`
  imports wire to the same allocator instance, so they share **one heap**. No
  per-package allocator, no per-package memory.
- One shared `Table` (funcref), each package's functions installed at a disjoint
  `__table_base` (the loader already does this per-package via `__wasm_call_ctors`).

### 3.2 Cross-package wiring (the win)

A's import `math.double` is wired **directly to B's `double` export** in the same
shared `Store` — there is no `cross_package_call` bridge. Because A and B share
memory and allocator:

1. A's generated import stub (`#[import_from]`) encodes the input `Value` into the
   **shared** heap via the shared `pack:alloc`, and calls the raw import with
   `(in_ptr, in_len, out_ptr_ptr, out_len_ptr)`.
2. The raw import **is** B's export function. B decodes from `in_ptr` (same
   memory — no copy), computes, encodes its result into the **shared** heap via
   the **same** allocator, and writes the result ptr/len into A's slots.
3. A decodes the result from that ptr (same memory — no copy).

So the ABI marshalling (encode/decode at the boundary) is unchanged, but the
**byte copy between memories disappears**, and there is **one** allocator instead
of two. This is also precisely the shape that a level-3 static merge collapses
into a single module.

**Return-buffer ownership.** B allocates its result buffer from the shared
`pack:alloc`; someone must free it. Mirror the host-return ownership rule from
0.8.0: A's import stub calls `__pack_free(out_ptr, out_len)` after it decodes the
result. Since the allocator is shared, the free is unambiguous (same heap). This
needs to be made explicit in the guest `__import_impl` for the cross-package case
(today's host-return path already frees; confirm the package-import path does the
same, or add it — same one-line pattern).

### 3.3 Instantiation order

Topological, as today: providers (no imports) first, then consumers, in
dependency order (DAG only, no cycles). For each package the loader: assigns its
base + stack, wires `pack:alloc` + its cross-package imports (to already-
instantiated providers' exports), resolves `GOT.mem.__data_end`, runs data relocs
+ ctors.

### 3.4 Interface compatibility at `wire()` time

A's import signature must match B's export signature. packr already has structural
interface hashing (O(1) compatibility check, `docs/INTERFACE-HASHING.md`); `wire()`
should validate the two pact interfaces match and error legibly if not — the
same "failed to convert parameter" class we guard against, caught at build time.

---

## 4. The two-package example (the deliverable to build first)

`adder` + `doubler` sharing **one** memory:

- Build both PIC (the `.cargo/config` recipe + `packr-guest` `pic` feature).
- Loader: allocator + `doubler` (base D) + `adder` (base A) in one shared memory;
  `adder`'s `math.double` wired directly to `doubler`'s `double` export.
- `comp.call("adder", "process", &S64(5))` → adder calls doubler's `double(5)=10`
  → `+1` = `11`, with **no cross-memory copy** and **one** allocator.
- Land as a `tests/` case (e.g. `pic_multi_package.rs`) asserting the round-trip +
  that both packages allocated from the same heap region.

This is the first concrete proof of package-to-package composition on the PIC
substrate, and the smallest useful validation of §3.

---

## 5. Open questions / risks

- **Cross-package return-buffer free** (§3.2): make the guest import stub free
  B's result buffer after decode. One-line pattern, but must be explicit so a
  long-lived composition doesn't leak per cross-call. (Regression: drive many
  cross-calls, assert shared-heap high-water stays flat — mirror the
  `async_host_fn_large_returns_do_not_leak` test.)
- **Memory floor scales with N.** Each package adds ~(stack + static data) to the
  shared memory (the ~1.5MB/package floor noted in the allocator work). N packages
  → N floors. Mitigations: trim per-package stack size; read exact `__data_end`
  to pack regions tightly. Worth measuring in the two-package example.
- **Composition × host functions × the async/theater path.** Today's builder
  supports host fns for all packages; the 0.8.0 async path supports host fns and
  is where theater lives. Composing packages that *also* import host functions,
  under the async loader, is a further integration — spec separately once §4 lands.
- **DAG only** (no cycles), as today — eager, all-at-once instantiation.
- **Table growth.** N packages' functions in one shared Table; ensure `TABLE_MIN`
  / growth accommodates the sum.

---

## 6. Level 3 (future): static merge → one `.wasm`

The PIC side modules (allocator + A + B) are relocatable objects. A wasm linker
step (`wasm-ld -r`, or a purpose-built merge) can combine them into a single
relocatable module with one fixed memory layout, finalized into **one passable
`.wasm`** — the original "package it up and pass it around" goal. This is a
larger, separate effort; §3 (runtime PIC composition) is its prerequisite and its
input.

Keep **both**: runtime PIC composition (level 2) stays valuable for loading /
swapping packages at runtime; the static merge (level 3) is for shipping a frozen
bundle. They share the same PIC side-module format.

---

## 7. Implementation steps (rough)

1. **Generalize the loader** from {allocator + 1 package} to {allocator + N
   packages}: assign disjoint `__memory_base` + stack per package; share the one
   allocator, `Memory`, and `Table`.
2. **Wire cross-package imports directly** (A's import → B's export) in the shared
   linker, with the return-buffer free rule (§3.2) and `wire()`-time interface
   compatibility check (§3.4).
3. **Port `CompositionBuilder`** onto this backend; keep the public API.
4. **Two-package PIC example + test** (§4).
5. **Docs + README**: land this doc; correct the README "Static Composition"
   checkbox (today it's *runtime* composition, not a merged module) and point at
   the level-1/2/3 roadmap.

---

## 8. Documentation home

- **This doc** (`docs/pic-composition.md`) — the level-2 spec + the roadmap.
- `docs/package-linking-exploration.md` — the level-1 design; now historical
  (add a superseded-by pointer at its top).
- `README.md` — the "Static Composition — compose multiple packages into one
  module" checkbox is overstated (the shipped builder does *runtime* composition
  with separate memories); reword to: runtime composition shipped, PIC
  shared-memory composition specced (this doc), static merge future.
