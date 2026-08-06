# User-defined generics

Pact supports **user-defined generic types** — parameterized records, variants,
and aliases — and **generic interfaces** that composition can bind to concrete
implementations. This is the reference for what actually ships (as of v0.13.0);
`TYPE-SYSTEM.md` describes the broader (partly still-aspirational) design.

## The key property: type-parameter erasure

Pact's ABI is *structural and schema-driven* — a value on the wire is a tree of
self-describing nodes (a record is `type_name + field names + child nodes`), and
the field *types* are never written. Type parameters are just placeholders for
field types, so they are **erased on the wire**: a value of `pair<u32, string>`
encodes byte-identically to the equivalent hand-written monomorphic record.

Two consequences:

- **No wire-format change.** Generics ship as a backward-compatible minor.
  Existing packages and non-generic code are byte-for-byte unaffected.
- **No monomorphization.** A pact generic maps 1:1 onto a Rust generic; there is
  no `pair_u32_string` type explosion.

## Generic type definitions

Records, variants, and aliases can take type parameters:

```pact
record pair<a, b> {
    first: a,
    second: b,
}

variant tree<t> {
    leaf(t),
    branch(tuple<tree<t>, tree<t>>),   // recursion + generics
}

type boxed<t> = list<t>
```

Instantiate them by applying type arguments:

```pact
type int-string = pair<u32, string>
handle-request: func(t: tree<u32>) -> pair<string, list<u8>>
```

The resolver checks arity (a `pair<u32>` — one argument for two parameters — is
rejected) and substitutes the arguments through the definition, including
recursive uses. Only **first-order, fully-applied** generics are supported;
higher-kinded parameters (`<f: * -> *>`) and const generics are not.

### On the guest (Rust) side

A generic pact type lowers to a generic Rust type, and `#[derive(GraphValue)]`
works on generic types directly:

```rust
#[derive(GraphValue)]
struct Pair<A, B> {
    first: A,
    second: B,
}
```

The derive injects the bounds each parameter needs to round-trip through the
ABI (`A: Into<Value> + TryFrom<Value, Error = ConversionError> + KnownValueType`),
so a generic type Just Works. packr ships the *machinery*, not a data-structure
library: you write your own `OrSet<T>`, `Either<T, E>`, etc. and derive
`GraphValue` on them.

> Known gap: a generic `Option<T>` *field* is not yet supported — packr's
> `Option<T>` decode goes through a separate `FromValue` trait (to avoid a
> coherence clash) rather than `TryFrom<Value>`. Concrete `Option<Foo>` fields
> are unaffected.

## Generic interfaces + composition

An interface can be parameterized over a type, declared at interface scope:

```pact
type s: serializable
exports {
    initial-state: func() -> s
    apply: func(event: sm-event, state: s) -> s
    members: func(state: s) -> list<pubkey>
}
```

The constraint (`serializable`) is currently *carried but not enforced* — every
Graph-ABI type satisfies it, so it is effectively a marker for now.

### Composing a generic interface with a concrete one

Composition can wire a **generic** side (a component written against `s`) to a
**concrete** side that pins `s` to an actual type — for example an RSM node
generic over its state machine's state, composed with a concrete SM:

```
node   imports  sm   over   s          (generic)
sm     exports  sm   over   chat-state (concrete)
```

Their interface hashes differ (a generic `s` never hashes equal to a concrete
`chat-state`), so composition does **compose-time unification**: it structurally
unifies the generic signatures against the concrete ones, binds `s := chat-state`
consistently across every function, and — because unification succeeds only when
`substitute(generic) == concrete` — the interfaces are guaranteed to agree once
bound. No runtime cost: the cross-memory copy shim was already structural, so it
marshals the concrete values unchanged.

Only **generic ↔ concrete** is supported (one side must pin the parameters);
generic ↔ generic is rejected.

### Authoring the guest side

Declare an interface parameter at the top level of a `pack_types!` block:

```rust
packr_guest::pack_types! {
    type s: serializable
    imports {
        sm {
            apply: func(event: sm-event, state: s) -> s,
        }
    }
}
```

The guest embeds the parameter (and the references to it in signatures) into the
package's `__pack_types` metadata, which is what composition reads to reconcile
the link. A hand-written generic component needs nothing more than this.

## What is not (yet) implemented

- Higher-kinded parameters (`<f: * -> *>`) and const generics.
- Constraint *enforcement* — constraints are parsed and carried, not checked.
- `wit!`-macro codegen of a generic component *trait* (hand-written components
  work today; macro-generated ones do not).
- A generic `Option<T>` field in the `GraphValue` derive (see above).
