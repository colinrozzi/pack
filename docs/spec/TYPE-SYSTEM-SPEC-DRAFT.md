# Pact Type System Formal Spec (Draft 0)

This document starts the formal specification for Pact's type system. It
captures the semantic model, a precise core calculus, and open decisions that
must be resolved before implementation hardens.

## Status

- **Maturity:** Draft 0 (non-normative)
- **Intended audience:** language/runtime maintainers, codegen maintainers
- **Out of scope for this draft:** parser grammar details, full ABI binary spec

## 1. Design Goals and Non-Goals

### Goals

1. Specify a **total, terminating** type checker over interface declarations.
2. Support **parametric polymorphism** for types and interfaces.
3. Support **trait-constrained generics** with explicit coherence rules.
4. Specify **interface transforms** as typed, terminating rewrites.
5. Preserve **structural ABI compatibility** via canonicalized concrete forms.

### Non-goals

1. No value-level language in Pact.
2. No dependent, refinement, or const-generic typing.
3. No general recursion in type-level computation.
4. No subtyping/inheritance lattice.

## 2. Core Semantic Model

Pact type-checking operates over declaration environments:

- **Type environment**: named type constructors and aliases.
- **Trait environment**: trait declarations and supertrait edges.
- **Impl environment**: trait implementation instances.
- **Interface environment**: interface declarations and transformed interfaces.

There is no term typing judgment for expressions because Pact has no value-term
language. The spec only types declarations and interface signatures.

## 3. Syntactic Categories (Core)

This section defines a reduced, formal core independent of surface syntax.

- `τ` types: primitive | constructor application | tuple | list | option | result
- `σ` schemes: `∀ᾱ. C ⇒ τ`
- `C` constraints: conjunction of trait predicates `Trait(τ)`
- `I` interfaces: finite map from function names to signatures
- `f` signatures: parameter type list × optional return type

### 3.1 Kinds (initial)

Draft 0 adopts a minimal kind system:

- `*` for inhabited concrete types.
- Type constructors have arrow kinds, e.g. `list : * -> *`, `result : * -> * -> *`.

Kind checking is required before type checking.

## 4. Static Semantics

## 4.1 Well-formed types

Judgment form: `Γ ⊢ τ : κ`

- Primitive types are kind `*`.
- Type variables are looked up in `Γ`.
- Constructor application requires argument kinds matching constructor domain.
- Fully applied constructors that check kind-wise produce kind `*`.

## 4.2 Type declarations

Judgment form: `Δ; Γ ⊢ decl ok`

- Record/variant/enum/flags declarations produce type constructors.
- Recursive declarations are permitted if all referenced names resolve in `Δ`.
- Alias declarations are expanded during normalization (see §8).

## 4.3 Trait declarations

Judgment form: `Θ; Γ ⊢ trait decl ok`

- Trait parameters must be kind-correct.
- Supertrait graph must be acyclic.
- Trait members are signature-level contracts only.

## 4.4 Interface declarations

Judgment form: `Δ; Θ; Γ ⊢ interface I ok`

- Export names are unique.
- Each export signature is kind-correct and well-formed.
- Parameterized interfaces introduce universally quantified type variables.
- Trait-bounded parameters add constraints to the interface scheme.

## 5. Constraint System and Inference

Pact adopts HM-style inference for declaration-level polymorphism with
trait constraints.

Judgment form: `Δ; Θ; Ψ ⊢ use-site ⇒ (substitution, obligations)`

Where `Ψ` is the current set of in-scope assumptions.

### Inference principles

1. Generate type equations from constructor/interface use.
2. Unify equations (occurs-check required).
3. Accumulate trait obligations from bounds and required traits.
4. Discharge obligations using impl resolution (see coherence §6).

Inference completeness claim in this spec is limited to the HM core. For
trait-constrained programs, determinism depends on coherence guarantees.

## 6. Trait Implementations and Coherence

This section is **normative target**, pending final agreement.

## 6.1 Candidate rule (proposed)

- At most one applicable impl per fully concrete trait predicate.
- Overlapping impl heads are rejected unless one is strictly more specific and
  an unambiguous specialization rule is later adopted (not included in Draft 0).
- Blanket impls are allowed only if they do not create overlap under any
  satisfiable substitution.

## 6.2 Orphan-style locality (open)

Open decision: whether to require that either trait or target type constructor is
owned by the current package/module to prevent downstream coherence breakage.

## 7. Interfaces as Algebraic Objects

Treat interfaces as finite maps `Name -> Signature`.

### 7.1 Union `I1 + I2`

- Domain is the union of names.
- If a name appears in both, signatures must be equivalent after normalization.
- Otherwise: type error with both normalized forms reported.

### 7.2 Extension

`I + {exports...}` is sugar for union with an anonymous interface literal.

## 8. Normalization and Structural Equivalence

Structural comparisons must use a canonical normalized form:

1. Expand aliases.
2. Canonicalize declaration references.
3. Canonicalize generic binders (alpha-normalization).
4. Canonicalize record/variant/enum metadata ordering where required by ABI.

Two signatures/types are structurally equivalent iff their normalized forms are
syntactically equal.

## 9. Transforms as Typed Rewrites

A transform `T` is a total function from interfaces to interfaces:

`T : Interface -> Interface`

Transforms are defined by terminating pattern rewrites over normalized export
signatures.

### Safety constraints

1. Rewrites cannot inspect runtime values.
2. Rewrites must be structurally recursive over finite interface structure.
3. Rewrites must be hygienic (inserted identifiers must avoid collisions).

### Composition

`T2(T1(I))` is left-to-right in source form `T2(T1(I))`.

## 10. ABI Boundary Rule

All polymorphism is fully instantiated before ABI lowering.

- ABI encoding only sees concrete types and concrete interfaces.
- Trait bounds do not change ABI shape directly; only resulting type structure
  contributes to compatibility/hashing.

## 11. Soundness Targets (informal, for future proof work)

1. **Termination:** type checking and transform evaluation terminate.
2. **Coherence:** successful resolution yields a unique impl per concrete
   obligation.
3. **Boundary consistency:** equivalent normalized interfaces produce identical
   ABI compatibility identity.

## 12. Open Questions (to resolve in follow-up PRs)

1. Coherence model: strict global coherence vs scoped instances.
2. Orphan/locality restrictions for impls.
3. Precise overlap algorithm for blanket impl detection.
4. Canonicalization details that feed hashing identity.
5. Transform hygiene, especially parameter-name collisions.
6. Whether interface inheritance syntax is retained or reduced to algebraic union.

## 13. Proposed PR Sequence

1. **PR A:** Finalize core calculus + kinds + WF judgments.
2. **PR B:** Finalize trait coherence and impl resolution rules.
3. **PR C:** Finalize transform rewrite semantics and hygiene.
4. **PR D:** Finalize canonicalization and hashing-input definition.
5. **PR E:** Add conformance test corpus for all normative claims.

## 14. Change Log

- Draft 0: initial formalization skeleton with explicit open-decision list.
