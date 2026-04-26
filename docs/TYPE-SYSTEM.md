# Pact Type System

## Core Insight

Pact operates purely in the type space. There is no value language — values live
in Wasm guests, written in Rust or any other language. The Pact type checker
never sees runtime values; it sees interface boundaries and nothing behind them.

This means:

- **Dependent types are incoherent.** You can't depend on values you don't have.
- **Refinement types are incoherent.** You can't predicate on values you can't inspect.
- **The language is total by construction.** No halting problem, no infinite loops.
  Type structures are finite; computation over them always terminates.
- **Every well-formed Pact program is a proof** that the interface is structurally
  sound. There is no "compiles but crashes at runtime" for the IDL itself.

What Pact *does* have access to — and can compute over — is:

- **Types** — the full structure of every type definition
- **Interfaces** — which functions exist, their signatures
- **Names** — type names, field names, function names, case names
- **Structure** — how types compose (fields, cases, elements, parameters)

The type system is a functional language over these things.

## Pack as General Interface Layer

Pack is not specific to any single deployment model. It is a general
package-to-package interface layer. Different deployment contexts impose
different constraints on what can cross an interface boundary:

- **In-process packages** share a Wasm runtime. They can pass data, function
  references, handles, and capabilities across boundaries.
- **Actor-boundary packages** (theater) communicate via serialized messages.
  Everything crossing the boundary must be serializable data.

These are not different languages — they are different **constraint profiles**
over the same type system. Theater's requirement that all interface types be
serializable is expressed as a trait bound, not baked into the language.

```
┌─────────────────────────────────────────────┐
│             Pack Type System                │
│   Types, functions, HKT, traits, transforms │
│                                             │
│   ┌─────────────────┐  ┌─────────────────┐  │
│   │   In-process    │  │    Theater       │  │
│   │   packages      │  │    (actors)      │  │
│   │                 │  │                  │  │
│   │   Can pass:     │  │   Can pass:      │  │
│   │   - data        │  │   - data only    │  │
│   │   - functions   │  │   (serializable  │  │
│   │   - handles     │  │    bound)        │  │
│   │   - anything    │  │                  │  │
│   └─────────────────┘  └─────────────────┘  │
│                                             │
│   Same type system, different constraints    │
└─────────────────────────────────────────────┘
```

This has a critical design consequence: the type system must be rich enough to
describe function types, higher-kinded abstractions, and capability patterns —
even though some deployment contexts will restrict themselves to a serializable
subset. The constraints are expressed *within* the type system, not *outside* it.

## Foundations

### Paradigm: Hindley-Milner + Traits + Kinds + Structural Compatibility

Four interlocking systems:

**Hindley-Milner** provides parametric polymorphism with complete type inference.
Type variables range over types. Users write parameterized interfaces and the
system infers concrete types at use sites. This is the same core that ML had in
1978 — well-understood, well-implemented, proven.

**Traits** provide named abstractions over structural requirements. A trait says
"any type satisfying these properties." Trait bounds constrain type variables,
giving the system a language for expressing capability requirements. Deployment
constraints like serializability are expressed as traits.

**Kinds** classify types by their shape. Concrete types have kind `*`. Type
constructors like `list` and `option` have arrow kinds like `* -> *`. This
enables abstracting over type constructors — not just "any type" but "any
single-argument type constructor." Kinds are what make higher-order transforms
and generic wrappers possible.

**Structural compatibility** via Merkle hashing provides O(1) interface
compatibility checking at the ABI layer. Two types with the same structure are
compatible regardless of name. This is the runtime complement to the design-time
nominal system — Pact uses names for clarity, the ABI uses structure for
compatibility.

### What we explicitly do NOT include

- **Subtyping / inheritance.** Fights with structural hashing, variance is a tar
  pit, and traits give you everything subtyping does without the complexity.
- **Dependent types.** No access to values. Incoherent in a pure type space.
- **Refinement types.** Same reason. Can't predicate on invisible values.
- **Const generics.** `Array<N>` where N is a number requires value-level
  reasoning.
- **General recursion in the type language.** Recursive *data types* yes.
  Recursive *type computation* no. This is what keeps type checking total.

## Type Language

### Primitive types

```
bool
u8  u16  u32  u64
s8  s16  s32  s64
f32  f64
char
string
```

### Compound types

```
list<T>
option<T>
result<T, E>
tuple<T, U, ...>
```

### Function types

Function types are first-class. A function type describes a callable signature
that can be passed across interface boundaries:

```pact
// Function types as type expressions
type predicate<T> = func(T) -> bool
type mapper<A, B> = func(A) -> B
type reducer<T, R> = func(R, T) -> R
type effect = func()
```

Function types can appear anywhere a type can — in records, variants, parameters,
return types:

```pact
record callback-pair<T> {
    on-success: func(T),
    on-error: func(string),
}

interface filter<T> {
    exports {
        filter: func(items: list<T>, pred: func(T) -> bool) -> list<T>
        map: func(items: list<T>, f: func(T) -> T) -> list<T>
    }
}
```

In deployment contexts where function references cannot cross the boundary (e.g.,
serialized actor messages), trait bounds enforce the restriction — the type system
itself does not limit what types can contain.

### Type definitions

```pact
// Alias
type id = string

// Record
record point {
    x: f64,
    y: f64,
}

// Variant (sum type with payloads)
variant shape {
    circle(f64),
    rect(point, point),
}

// Enum (sum type without payloads)
enum color { red, green, blue }

// Flags (bitfield)
flags permissions { read, write, execute }
```

### Recursive types

Recursion is allowed by default, no special syntax:

```pact
variant sexpr {
    sym(string),
    num(s64),
    lst(list<sexpr>),
}

// Mutual recursion
variant expr {
    literal(lit),
    binary(string, expr, expr),
}

variant lit {
    number(f64),
    quoted(expr),
}
```

### Type parameters

Types and interfaces can be parameterized over types of any kind:

```pact
// Kind *: parameterized over concrete types
record pair<A, B> {
    first: A,
    second: B,
}

variant tree<T> {
    leaf(T),
    node(tree<T>, tree<T>),
}

// Kind * -> *: parameterized over type constructors
record wrapped<F: * -> *, T> {
    value: F<T>,
}

// Usage
type int-pair = pair<s32, s32>
type string-tree = tree<string>
type optional-int = wrapped<option, s32>
```

Type parameters are universally quantified. `pair<A, B>` means "for all types A
and B, a record with fields of those types."

### Kinds

Kinds classify types by arity:

- `*` — a concrete, inhabited type. `s32`, `string`, `point`, `list<s32>`.
- `* -> *` — a type constructor taking one argument. `list`, `option`.
- `* -> * -> *` — a type constructor taking two arguments. `result`, `pair`.
- And so on for higher arities.

Kind checking ensures type constructors are applied correctly:

```pact
// list has kind * -> *
// list<s32> has kind * (fully applied)
// list alone cannot appear where kind * is expected

// result has kind * -> * -> *
// result<s32, string> has kind * (fully applied)
// result<_, string> has kind * -> * (partially applied)
```

Partial application of type constructors is supported. `result<_, string>` is a
type constructor of kind `* -> *` — it takes one more type argument to become
concrete. This enables patterns like:

```pact
// result<_, rpc-error> is kind * -> *, just like option
type rpc-result = result<_, rpc-error>

// Can be used anywhere a * -> * constructor is expected
transform wrap<I, F: * -> *> {
    map exports {
        func({params}) -> {R} => func({params}) -> F<{R}>
    }
}

// These are equivalent:
interface calc-client-a = wrap(calculator, result<_, rpc-error>)
interface calc-client-b = wrap(calculator, rpc-result)
```

## Traits

### Trait definitions

Traits name a set of structural requirements on a type:

```pact
trait serializable<T> {
    encode: func(value: T) -> list<u8>
    decode: func(bytes: list<u8>) -> result<T, string>
}

trait hashable<T> {
    hash: func(value: T) -> u64
}

trait eq<T> {
    equals: func(a: T, b: T) -> bool
}
```

A trait is not a type — it's a *constraint on* a type. You cannot have a value of
type `serializable`; you can only require that a type variable satisfies
`serializable`.

### Supertraits

Traits can extend other traits:

```pact
trait ordered<T> : eq<T> {
    compare: func(a: T, b: T) -> s32
}
```

`ordered<T>` implies `eq<T>` — any type satisfying `ordered` must also satisfy
`eq`. The supertrait graph must be acyclic.

### Higher-kinded trait bounds

Traits can constrain type constructors, not just concrete types:

```pact
trait mappable<F: * -> *> {
    map: func(value: F<A>, f: func(A) -> B) -> F<B>
}

trait flat-mappable<F: * -> *> : mappable<F> {
    flat-map: func(value: F<A>, f: func(A) -> F<B>) -> F<B>
}

impl mappable<list> {
    map: func(value: list<A>, f: func(A) -> B) -> list<B>
}

impl mappable<option> {
    map: func(value: option<A>, f: func(A) -> B) -> option<B>
}
```

This enables abstracting over computational contexts — "any type constructor
that supports mapping."

### Trait bounds

Type parameters can be constrained by trait bounds:

```pact
// Single bound
record cache<K: hashable, V> {
    entries: list<pair<K, V>>,
}

// Multiple bounds
interface channel<T: serializable + eq> {
    exports {
        send: func(msg: T)
        recv: func() -> option<T>
    }
}

// Higher-kinded bound
interface stream-processor<F: * -> * + mappable, T> {
    exports {
        process: func(input: F<T>) -> F<T>
    }
}
```

Bounds are checked at instantiation: `channel<my-msg>` is valid only if `my-msg`
satisfies both `serializable` and `eq`.

### Trait implementations

Types satisfy traits via `impl` declarations:

```pact
impl serializable<point> {
    encode: func(value: point) -> list<u8>
    decode: func(bytes: list<u8>) -> result<point, string>
}
```

An `impl` is a claim that a concrete type meets a trait's requirements. The type
checker verifies that the function signatures match. The actual *implementations*
of those functions live in Wasm — the IDL only checks the shapes.

### Blanket implementations

Blanket impls apply to parameterized types:

```pact
// Any list of serializable elements is itself serializable
impl<T: serializable> serializable<list<T>> {
    encode: func(value: list<T>) -> list<u8>
    decode: func(bytes: list<u8>) -> result<list<T>, string>
}

// Any pair of serializable elements is serializable
impl<A: serializable, B: serializable> serializable<pair<A, B>> {
    encode: func(value: pair<A, B>) -> list<u8>
    decode: func(bytes: list<u8>) -> result<pair<A, B>, string>
}
```

### Coherence

Trait implementation resolution must be deterministic:

- At most one applicable impl per fully concrete trait predicate.
- Overlapping impl heads are rejected. No specialization.
- Orphan rule: either the trait or the type must be defined in the current
  package. This prevents downstream coherence breakage when packages are
  composed independently.

## Deployment Constraints

Different deployment contexts restrict what can cross interface boundaries. These
restrictions are expressed as trait bounds on interfaces, not as language-level
limitations.

### The `serializable` constraint

Theater requires all data crossing actor boundaries to be serializable:

```pact
// serializable is just a trait
trait serializable<T> {
    encode: func(value: T) -> list<u8>
    decode: func(bytes: list<u8>) -> result<T, string>
}

// Primitive types are inherently serializable
impl serializable<bool> { ... }
impl serializable<s32> { ... }
impl serializable<string> { ... }
// ... etc for all primitives

// Compound types propagate serializability
impl<T: serializable> serializable<list<T>> { ... }
impl<T: serializable> serializable<option<T>> { ... }
impl<A: serializable, B: serializable> serializable<result<A, B>> { ... }
```

Function types are NOT serializable — `func(A) -> B` has no `serializable` impl.
This is what naturally excludes functions from actor interfaces without any
special-case logic in the language.

### Constraint profiles

A constraint profile is a meta-level assertion that an entire interface satisfies
a set of requirements:

```pact
// All types in this interface must be serializable
constraint actor-safe<I: interface> =
    forall T in I.param-types + I.return-types: serializable<T>

// Assert that calculator can be used across actor boundaries
assert actor-safe<calculator>

// This would fail — filter passes function values
// assert actor-safe<filter<s32>>  // ERROR: func(s32) -> bool is not serializable
```

Constraint profiles make deployment requirements checkable and explicit. An
interface that type-checks in the general system can be validated against a
specific deployment context.

### Theater as a constraint profile

Theater interfaces are general pack interfaces with the `actor-safe` constraint:

```pact
// A theater handler is a handler where state is serializable
interface handler<S: serializable> {
    exports {
        init: func() -> S
        handle-message: func(state: S, msg: list<u8>) -> S
        handle-request: func(state: S, msg: list<u8>) -> tuple<S, list<u8>>
    }
}

// This works — counter-state is serializable, all params/returns are data
interface counter = handler<counter-state>

// A general-purpose interface with function passing
interface transformer<A, B> {
    exports {
        apply: func(f: func(A) -> B, items: list<A>) -> list<B>
    }
}

// transformer can be used between in-process packages
// transformer CANNOT be used across actor boundaries (func is not serializable)
// The type system catches this at interface instantiation, not at runtime
```

## Interfaces

### Parameterized interfaces

Interfaces can take type parameters, optionally bounded:

```pact
interface handler<State: serializable, Message: serializable> {
    exports {
        init: func() -> State
        handle: func(state: State, msg: Message) -> tuple<State, option<Message>>
    }
}
```

Instantiation:

```pact
// Explicit
interface my-handler = handler<my-state, my-msg>

// Via use (type arguments inferred from context or explicit)
interface caller {
    use handler<my-state, my-msg>
}
```

### Interface operations

Interfaces can be combined algebraically:

```pact
// Union: all exports from both interfaces
interface full = handler<S, M> + lifecycle

// Extension: add exports to an existing interface
interface extended = calculator + {
    exports {
        history: func() -> list<string>
    }
}
```

Union requires that overlapping function names have identical signatures (checked
via structural hash). Conflicting signatures are a type error.

### Transforms

Transforms are functions from interfaces to interfaces:

```pact
// Transform application
interface calc-client = rpc(calculator)

// Composition
interface traced-calc = traced(rpc(calculator))
```

Transforms compose left-to-right: `traced(rpc(calculator))` applies `rpc` first,
then `traced` to the result.

### Transform definition syntax

A transform declares how it modifies an interface's exports:

```pact
transform traced<I> {
    // Adds a span-id parameter to every export
    map exports {
        func({params}) -> {R} => func(span-id: string, {params}) -> {R}
    }
}

transform fallible<I, E> {
    // Wraps every return type in result<T, E>
    map exports {
        func({params}) -> {R} => func({params}) -> result<{R}, E>
        func({params})         => func({params}) -> result<_, E>
    }
}
```

The `map exports` block is pattern matching over function signatures. `{params}`
captures the parameter list, `{R}` captures the return type. This is type-level
computation — it operates on the *structure of signatures*, not on values.

### Generic wrapper transforms

Higher-kinded type parameters make transforms fully generic over wrapping
strategy:

```pact
transform wrap<I, F: * -> *> {
    map exports {
        func({params}) -> {R} => func({params}) -> F<{R}>
        func({params})         => func({params}) -> F<unit>
    }
}

// rpc is wrap with result<_, rpc-error>, plus the error type definition
transform rpc<I> = wrap<I, result<_, rpc-error>> + {
    type rpc-error = variant {
        timeout,
        actor-not-found(string),
        function-not-found(string),
        shutting-down,
        channel-closed,
        call-failed(string),
    }
}

// async wraps in future — for in-process packages
transform async<I> = wrap<I, future>

// observable wraps in a stream
transform observable<I> = wrap<I, stream>
```

One generic `wrap` transform, many specific wrappers. The kind system is what
makes this possible — `F: * -> *` says "any single-argument type constructor."

### Transform safety

Transforms must be total and terminating:

- Rewrites cannot inspect runtime values.
- Rewrites are structurally recursive over finite interface structure.
- Rewrites must be hygienic — inserted identifiers that collide with existing
  names in the base interface are a type error at transform application time.

## Type Inference

Hindley-Milner inference means users rarely need to write type annotations
explicitly. The system infers types from usage:

```pact
// The type parameter T is inferred from the argument
type points = list<point>          // list<T> instantiated with T = point

// Interface type arguments inferred from use context
interface my-handler = handler<my-state, my-msg>

// Trait bounds propagate through inference
// If handler requires State: serializable, then my-state
// must satisfy serializable — checked at this use site
```

Type inference is complete for HM: if a valid typing exists, the system finds it.
No annotations needed except at definition sites (where you declare the
parameters).

### Where annotations are required

- Type parameter declarations: `record pair<A, B>` — you must name the parameters
- Kind annotations on higher-kinded parameters: `<F: * -> *>` — kinds are not
  inferred
- Trait bound declarations: `<T: serializable>` — you must state the constraints
- Top-level definitions: function signatures, type aliases — the public API is
  explicit

### Where annotations are inferred

- Type arguments at use sites: `pair<s32, s32>` can often be inferred
- Intermediate types in transform chains
- Trait satisfaction — if `T: serializable` is required and `my-type` has an
  impl, it's found automatically

## Interaction with Existing Systems

### Merkle Hashing

Structural hashing extends naturally:

- Parameterized types have no hash until instantiated — only concrete types are
  hashed.
- Function types hash by their parameter and return type structure.
- Kind information is not part of the hash — kinds are a design-time property.
- Trait bounds do not affect the hash — bounds are constraints, not structure.

The hash captures *what crosses the boundary*. Constraints about what *may*
cross the boundary are a separate, design-time concern.

### Graph ABI (CGRF)

The ABI encodes concrete types. All polymorphism is fully instantiated before ABI
lowering — there are no type variables at the ABI layer.

Function types at the ABI layer require a calling convention for the deployment
context:

- **In-process:** function references are Wasm `funcref` values or table indices.
- **Actor boundary:** function types cannot appear (enforced by `serializable`
  bound). The ABI never needs to encode a function — the constraint system
  prevents it.

### Transforms and Hashing

A transformed interface has its own hash, computed from the *result* of the
transform. `rpc(calculator)` produces a concrete interface with `result`-wrapped
returns; that interface is hashed like any other. The transform itself is not
part of the hash — only the resulting structure matters.

An interface written by hand that happens to match the output of
`rpc(calculator)` is compatible with it. Structure is what matters at the
boundary.

### Code Generation

Rust codegen from parameterized types produces generic Rust:

```pact
record pair<A, B> {
    first: A,
    second: B,
}
```

Generates:

```rust
#[derive(Debug, Clone, PartialEq)]
struct Pair<A, B> {
    first: A,
    second: B,
}
```

Higher-kinded parameters require codegen strategies specific to the target
language. In Rust, HKT is typically encoded via associated types or GATs.
Other codegen targets may have direct support.

## Examples

### Theater handler pattern

Every theater handler has the same shape — parameterize it once:

```pact
interface handler<S: serializable> {
    exports {
        init: func() -> S
        handle-message: func(state: S, msg: list<u8>) -> S
        handle-request: func(state: S, msg: list<u8>) -> tuple<S, list<u8>>
    }
}

interface supervisable<S: serializable> : handler<S> {
    exports {
        handle-child-event: func(state: S, event: child-event) -> S
    }
}
```

A concrete actor:

```pact
record counter-state {
    count: s32,
    name: string,
}

impl serializable<counter-state> {
    encode: func(value: counter-state) -> list<u8>
    decode: func(bytes: list<u8>) -> result<counter-state, string>
}

interface counter = handler<counter-state> + {
    exports {
        get-count: func(state: counter-state) -> tuple<counter-state, s32>
    }
}
```

### In-process higher-order interface

A package that operates on functions — only usable in-process:

```pact
interface combinators<A, B, C> {
    exports {
        compose: func(f: func(A) -> B, g: func(B) -> C) -> func(A) -> C
        apply: func(f: func(A) -> B, value: A) -> B
        map-all: func(items: list<A>, f: func(A) -> B) -> list<B>
        filter: func(items: list<A>, pred: func(A) -> bool) -> list<A>
    }
}

// This is valid for in-process use
// assert actor-safe<combinators<s32, s32, s32>> would FAIL
// because func types are not serializable
```

### Generic wrapper transforms

```pact
// The universal wrapper transform
transform wrap<I, F: * -> *> {
    map exports {
        func({params}) -> {R} => func({params}) -> F<{R}>
        func({params})         => func({params}) -> F<unit>
    }
}

// Specific wrappers are partial applications
transform rpc<I> = wrap<I, result<_, rpc-error>> + {
    type rpc-error = variant {
        timeout,
        actor-not-found(string),
        function-not-found(string),
        shutting-down,
        channel-closed,
        call-failed(string),
    }
}

transform async<I> = wrap<I, future>
transform observable<I> = wrap<I, stream>

// Compose freely
interface live-calc = observable(rpc(calculator))
```

### Mappable abstraction

```pact
trait mappable<F: * -> *> {
    map: func(value: F<A>, f: func(A) -> B) -> F<B>
}

impl mappable<list> {
    map: func(value: list<A>, f: func(A) -> B) -> list<B>
}

impl mappable<option> {
    map: func(value: option<A>, f: func(A) -> B) -> option<B>
}

// An interface generic over any mappable container
interface processor<F: * -> * + mappable, T, R> {
    exports {
        process: func(input: F<T>, step: func(T) -> R) -> F<R>
    }
}
```

### Constraint profiles in practice

```pact
constraint actor-safe<I: interface> =
    forall T in I.param-types + I.return-types: serializable<T>

// Validate at definition time
interface calculator { ... }
assert actor-safe<calculator>  // passes — all types are primitives

interface transformer<A, B> {
    exports {
        apply: func(f: func(A) -> B, items: list<A>) -> list<B>
    }
}
// assert actor-safe<transformer<s32, s32>>  // FAILS — func(s32) -> s32 not serializable

// The error is caught at interface design time, not at deployment time
```

## Summary

| Feature | Rationale |
|---------|-----------|
| Primitive types | Already complete |
| Records, variants, enums, flags | Already complete |
| Recursive types | Already complete |
| **Function types** | First-class types enabling higher-order interfaces |
| **Type parameters** | Parameterize types and interfaces |
| **Kind system** | Classify types by arity, enable HKT |
| **Partial application** | `result<_, E>` as a `* -> *` constructor |
| **Trait definitions** | Name structural requirements |
| **Trait bounds** | Constrain type parameters |
| **Trait implementations** | Declare that types satisfy traits |
| **Blanket impls** | Generic trait satisfaction |
| **Higher-kinded traits** | Abstract over type constructors |
| **Coherence rules** | Deterministic impl resolution, orphan rule |
| **Deployment constraints** | `serializable` bound for actor contexts |
| **Constraint profiles** | Validate interfaces against deployment contexts |
| **Interface parameterization** | Generic interfaces |
| **Interface union** | Combine interfaces algebraically |
| **Interface extension** | Add exports to existing interfaces |
| **First-class transforms** | Transforms defined in Pact, not Rust |
| **Generic transforms** | HKT-parameterized transforms (`wrap<I, F>`) |
| **Transform composition** | Composable and parameterizable |
| **Transform patterns** | Pattern matching over signatures |
| **HM type inference** | Infer type arguments at use sites |
| Subtyping | **Excluded** — conflicts with structural hashing |
| Dependent types | **Excluded** — no access to values |
| Refinement types | **Excluded** — no access to values |
| General type recursion | **Excluded** — keeps type checking total |
