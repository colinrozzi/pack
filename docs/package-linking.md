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
memory = "import"             # "import": PIC side-module, host provides memory (theater)
                             # "own": internal memory, bare-runnable (default)

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

- `name`, `output`, `memory` (`own` | `import`; see §5.5).
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
5. **Resolve the residual surface (no "mode").** There is *no* hosted/standalone
   flag — that was a false dichotomy. A composite is defined by what it *doesn't*
   link. Every import is a requirement you either **satisfy** (link a provider →
   internalized) or **leave residual**, uniformly — pact interfaces, host imports,
   and even the allocator (`pack:alloc` is just a provider you link or don't).

   The one genuinely special axis is the **linear memory**: a module has exactly
   one memory, either **owned** (defined + exported) or **imported**. That single
   choice is what makes a composite bare-runnable vs host-loaded, and it is
   independent of the rest:
   - **own memory** — internalize the PIC linkage globals (`__memory_base` etc.) to
     constants + define the memory internally + export it. Zero-substrate,
     bare-runnable (what `pack compose` emits today). For self-contained binaries.
   - **import memory (PIC)** — keep the PIC linkage as imports so a host places the
     composite. For **theater**, this is a hard contract (confirmed with
     theater-dev): the composite must be a proper **PIC side-module** whose import
     surface equals a *lone PIC actor's*, unified to one of each — `env.memory`,
     `env.__memory_base`, `env.__table_base`, `env.__indirect_function_table`,
     `env.__stack_pointer`, `GOT.mem.*`/`GOT.func.*` — plus residual `pack:alloc` +
     `theater:simple/*` + the lifecycle exports. It then rides theater's existing
     single-actor loader unchanged (one `__memory_base = 0x10000` into a fresh
     per-instance memory; data laid from offset 0). *Not* fixed-base — theater
     rejects a fixed-base module at instantiate (the 0.8.1 PIC assert), by design.

   **Build implication:** build members **PIC** and the two corners fall out of one
   fuse — own memory = internalize the linkage, import memory = keep it. The
   fixed-base recipe stays a shortcut for the bare-runnable corner only. One
   constraint for host-loaded composites: the fused static data + heap must fit
   under `MEM_PAGES` from `0x10000` (a multi-package composite is larger than a
   lone actor — make it configurable).
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
harness, or `memory = "own"` for a bare, self-contained test binary.

**Shared code** — library `strfmt.wasm` exports `format`; two actors import
`format`. Link each actor against `strfmt`. `format` is authored once; each actor
ships it fused in.

**Actor assembly** — `core.wasm` (exports the actor `handle-*` interface, imports
`math`) + `math-impl.wasm` (exports `math`). Link `core.math ← math-impl.math`,
export `core.handle-*`, `memory = "import"`. One actor artifact; theater loads it
unchanged.

## 7. Build plan & status

`pack compose` already does fuse + full-internalize + the memory/allocator
substrate. The linker adds, in order:

1. **Surface parsing + explicit link resolution + hash checks** — read
   `__pack_types`, validate the spec. **✅ landed** (`abi::decode_prefix` static
   reader #42; `link::{read_surface, check_link, resolve_links}` #44/#45).
2. **Partial internalization** (linked-only; residual imports preserved) + the
   `memory = "import"` corner. **⏳ TODO — contract resolved** (theater-dev,
   2026-07-14): host-loaded composites must be **PIC side-modules** with an import
   surface equal to a lone PIC actor's (see §5.5); build members PIC so both
   memory corners fall out of one fuse. Remaining work is the PIC fuse + linkage
   unification, not a design question.
3. **Metadata regeneration** — the composite's `__pack_types`. **⏳ TODO** — the
   load-bearing piece for closure (re-linkable results).
4. **CLI + spec format** — `packr link <spec.toml>`; `pack compose` is the
   zero-link / `standalone` convenience case. **✅ landed** (#45).

**Interfaces on both sides (landed, #43).** A package *provides* and *requires*
interfaces symmetrically — `exports { math { double: func.. } }` is now valid,
and a provider's `math` export-interface hash equals a consumer's `math`
import-interface hash. Matching is **interface-to-interface** by structural hash,
not per-function name mapping. The flat/grouped split was only ever missing
export-side sugar; the metadata/hashing model was always symmetric.

**Mock proof (landed, #44).** The smallest slice that exercises matching + hash
check + fuse-and-run: `adder` requires `math`; `math-real`/`math-mock` provide it
(accepted, hashes equal); `doubler` (`value->value`, no `math`) is **rejected** —
the type-unsafe link raw composition silently fuses. Swapping in `math-mock`
makes the mock take effect. Remaining for the *full* proof: residual imports +
metadata regen (items 2–3).

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
