# Package Linking — Design Spec

**Status:** proposed. **Scope:** static (build-time) linking. **Relationship:**
generalizes `pack compose` — today's composer is the *zero-residual, standalone*
special case of the linker described here.

## 1. Goal

A **package** is a WASM binary with a typed surface: the interfaces it *imports*
(requires) and the interfaces it *exports* (provides), carried in its
`__pack_types` metadata. A **package linker** takes an explicit spec that
**matches** one package's import interfaces against another's export interfaces,
fuses the binaries (matched imports become direct internal calls), and emits a
**new package** whose surface is the *residual* (unmatched) imports plus the
exports you choose to expose.

The output is itself a first-class package — the same shape as its inputs — so
linking is **closed**: a linked result can be fed straight into another link.
That closure gives us, from one mechanism:

- **Shared code** — a library package (exports `foo`) linked into any actor that
  imports `foo`; authored once, linked many.
- **Actor assembly** — an entry package + its helper packages linked into one
  deployable actor.
- **Mocks for testing** — an actor imports `db`; link it against `real-db.wasm`
  for production or `mock-db.wasm` for a test, by pointing one link at a
  different binary. Same actor, swapped provider.

## 2. Model

| term | meaning |
|---|---|
| **package** | a WASM binary + `__pack_types` (typed import/export interfaces) |
| **interface** | a named, pact-typed set of functions, with a structural **hash** |
| **surface** | a package's `(imports, exports)` interface sets |
| **link** | satisfy an importer's interface with an exporter's interface (hash-checked) |
| **residual** | imports left unmatched after linking — the composite still requires them |
| **closure** | the composite is a valid package: residual imports + chosen exports + regenerated metadata |

We already have most of the substrate: pact interfaces; per-package
import/export metadata (`__pack_types`); per-interface structural hashes
(`decode_metadata_with_hashes` → `InterfaceHash`); and the fuse itself
(`pack compose`: binaryen `wasm-merge` + a `walrus` pass).

## 3. Spec format (explicit — v1)

> **Format note.** TOML is the *interim* v1 surface — enough to build against, but
> verbose (three tables to read one wiring). The **model** (§2, §4) is
> format-independent; the target is a small composition DSL (§8). Explicit links
> only for now; auto-matching by hash is a deliberate later relaxation (§10).

```toml
name   = "user-actor-test"
output = "user-actor-test.wasm"
mode   = "hosted"              # "hosted": memory/allocator stay imports (default)
                              # "standalone": memory/allocator internalized

[[binary]]
alias = "actor"               # imports: db, theater:runtime ; exports: handle-send
wasm  = "user_actor.wasm"

[[binary]]
alias = "mockdb"              # exports: db
wasm  = "mock_db.wasm"

# satisfy actor's `db` import with mockdb's `db` export (hash-checked)
[[link]]
from = "actor.db"             #  importer.interface  ←  exporter.interface
to   = "mockdb.db"

# the composite's public exports:  <composite-interface> = "<alias>.<interface>"
[exports]
handle-send = "actor.handle-send"

# residual imports are automatic: any unmatched import (here actor's
# `theater:runtime`) flows through to the composite's surface unchanged.
```

Fields:

- `name`, `output`, `mode`.
- `[[binary]]` — `alias` (local handle) + `wasm` (path, relative to the spec file).
- `[[link]]` — `from = "<importer-alias>.<interface>"`, `to = "<exporter-alias>.<interface>"`.
- `[exports]` — what the result exposes.
- Anything neither exported nor internally linked → **residual import** (or a
  dropped export), automatically.

## 4. Matching semantics

- **Explicit only (v1).** Every wire is a `[[link]]`; no implicit matching.
- **Interface granularity.** A link binds a *whole* interface — all of its
  functions at once — not individual functions.
- **Hash-checked.** `from`'s import interface and `to`'s export interface must
  have equal structural hashes, else the link is a hard error. This type-safe
  substitution is exactly what makes mock-swapping sound.
- **Errors:** unknown alias/interface; hash mismatch; an exported name that does
  not resolve. An import named in no link is *not* an error — it becomes a
  residual (that is the point).

## 5. Mechanics

1. **Parse surfaces** — read each binary's `__pack_types` → its import/export
   interfaces + hashes.
2. **Validate links** — resolve aliases/interfaces; check hashes.
3. **Fuse** — `wasm-merge`, naming each provider so the linked imports resolve to
   its exports; matched imports become direct calls.
4. **Partial internalization** — only *linked* imports are internalized; residual
   imports stay as imports. (Today's `pack compose` internalizes everything → the
   zero-residual special case; this generalizes it.)
5. **Memory / allocator substrate** — the fused packages share one linear memory +
   one allocator (disjoint fixed bases per package, as `pack compose` does).
   `mode` decides the boundary:
   - `hosted` (default) — memory + allocator stay imports; theater (or a later
     link step) provides them. The composite is a PIC-style side-module; theater
     loads it like any actor, unchanged.
   - `standalone` — memory + allocator internalized (as `pack compose` does
     today); the composite runs on a bare runtime. Good for self-contained test
     binaries.
6. **Regenerate metadata** — strip the inputs' `__pack_types`, compute the
   composite's surface (`imports = ∪ inputs.imports − linked`, `exports = the
   [exports] table`, carrying interface identity + hashes from the sources), and
   embed fresh `__pack_types`. **This is the load-bearing new piece** — it is what
   makes the result first-class and re-linkable.
7. **Emit** the composite `.wasm`.

## 6. Use-case walkthroughs

**Mocks** (the driving case; spec in §3) — actor imports `db` + `theater:runtime`,
exports `handle-send`. Link `actor.db ← mockdb.db`. Result: an actor with `db`
mocked, `theater:runtime` residual, `handle-send` exported. Run it in a test
harness, or `standalone` for a bare test binary.

**Shared code** — library `strfmt.wasm` exports `format`; two actors import
`format`. Link each actor against `strfmt`. `format` is authored once; each actor
ships it fused in.

**Actor assembly** — `core.wasm` (exports the actor `handle-*` interface, imports
`math`) + `math-impl.wasm` (exports `math`). Link `core.math ← math-impl.math`,
export `core.handle-*`, `mode = "hosted"`. One actor artifact; theater loads it
unchanged.

## 7. Build plan

`pack compose` already does fuse + full-internalize + the memory/allocator
substrate. The linker adds, in order:

1. **Surface parsing + explicit link resolution + hash checks** — read
   `__pack_types`, validate the spec.
2. **Partial internalization** (linked-only; residual imports preserved) + the
   `hosted` mode.
3. **Metadata regeneration** — the composite's `__pack_types`.
4. **CLI + spec format** — `packr link <spec.toml>`; `pack compose` becomes the
   zero-link / `standalone` convenience case.

**First proof:** the mock-testing case — the smallest slice that exercises partial
linking + hash check + residual imports + metadata regen.

## 8. Format: TOML now, a DSL next

The TOML in §3 is the interim surface — chosen because we already parse TOML and
it is enough to build against. It is verbose: separate `[[binary]]`, `[[link]]`,
and `[exports]` tables force you to read three places to see one wiring, and it
reads nothing like the graph it describes.

The target is a small **composition DSL** where you instantiate a binary and wire
its imports inline — the way Racket `units` and WebAssembly's `wac` language read:

```
// user-actor-test — pack link DSL (sketch)
let mockdb = load "mock_db.wasm";           // exports: db

let test = load "user_actor.wasm" with {    // imports: db, theater:runtime
    db = mockdb.db,                          // satisfy `db`; theater:runtime left as a hole
};

export test.handle-send;
```

One place per instantiation, wiring reads left-to-right, and holes are simply
whatever you do not fill. This is the same config→language arc `wac` went through,
for the same reason. **Not now** — but the TOML and the DSL should desugar to the
same internal link graph, so v1 is not throwaway.

## 9. Prior art & inspiration

- **WebAssembly Component Model + `wac`** — the direct analog: typed import/export
  *worlds*, composed by satisfying imports with exports, closed. `wasi-virt`
  (virtualize an interface with a provider component) is our mock case, shipping.
  Take: the world abstraction, subtyping, and the config→DSL trajectory.
- **Backpack (Haskell)** — packages with **holes** (= our residual imports) filled
  by **mixin linking**; *definite* (fully linked) vs *indefinite* (has holes). We
  adopt "holes" as a synonym for residual imports, and "must be definite" as the
  surface-assertion check (§4).
- **ML functors / signatures** — modules parameterized by a signature-matched
  module. Take: **sharing constraints** — decide early whether a lib linked into
  two actors is shared or duplicated.
- **Unix `ld` / `wasm-ld`** — symbol resolution; residual undefined
  (`--allow-undefined`); **weak/overridable symbols** = the mock pattern. We are
  the *typed* version — names + hashes, not bare symbols.
- **Unison** — content-hash as identity; validates our interface hashes.
- **Nix overlays / DI containers** — `override`/rebind a dependency; the mock UX to
  emulate — swapping a provider should be one line.

## 10. Open questions / future

- **Auto-matching** — connect any import to any export with an equal interface
  hash, with explicit links only to disambiguate. Deferred by choice; the format
  reserves room for it.
- **Runtime backend** — the *same* interface/hash model, but theater *routes* A's
  calls to a live actor B instead of fusing B in. One spec, two backends (fuse vs.
  route). theater-dev's turf; worth designing the interface layer so both fit.
- **Multi-interface providers** — `wasm-merge`'s one-module-name-per-binary cannot
  route a binary's several export interfaces to different importers under
  different names; that needs a `walrus` export-rename pass. Fine for
  one-interface providers (the common case) now.
- **Nested layout** — recursive linking must keep memory bases disjoint across
  levels; the layout allocator has to compose (reserve a region per subtree).
- **Name collisions** — two providers exporting the same interface name; the
  explicit `[[link]]` disambiguates the wiring, but the export/metadata side needs
  a namespacing rule.
- **Nominal vs. structural identity** — hashes give structural compatibility; we
  may also want nominal interface identity (same pact name) as a readability
  guard. Likely both: nominal for intent, hash for safety.

## 11. Documentation home

- **This doc** (`docs/package-linking.md`) — the linker model + spec + roadmap.
- `docs/pic-composition.md` — the runtime PIC composition (level 2) + `pack
  compose` (the standalone static merge this generalizes).
