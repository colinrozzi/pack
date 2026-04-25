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

## Foundations

### Paradigm: Hindley-Milner + Traits + Structural Compatibility

Three interlocking systems:

**Hindley-Milner** provides parametric polymorphism with complete type inference.
Type variables range over types. Users write parameterized interfaces and the
system infers concrete types at use sites. This is the same core that ML had in
1978 — well-understood, well-implemented, proven.

**Traits** provide named abstractions over structural requirements. A trait says
"any type satisfying these properties." Trait bounds constrain type variables,
giving the system a language for expressing capability requirements.

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
- **General recursion in the type language.** Recursive *data types* yes (already
  supported). Recursive *type computation* no. This is what keeps type checking
  total.

## Type Language

### Primitive types

Unchanged from current Pact:

```
bool
u8  u16  u32  u64
s8  s16  s32  s64
f32  f64
char
string
```

### Compound types

Unchanged from current Pact:

```
list<T>
option<T>
result<T, E>
tuple<T, U, ...>
```

### Type definitions

Unchanged from current Pact:

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

Unchanged — recursion is allowed by default, no special syntax:

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

New. Types and interfaces can be parameterized:

```pact
// Parameterized type
record pair<A, B> {
    first: A,
    second: B,
}

// Parameterized variant
variant tree<T> {
    leaf(T),
    node(tree<T>, tree<T>),
}

// Usage — type arguments can be inferred or explicit
type int-pair = pair<s32, s32>
type string-tree = tree<string>
```

Type parameters are universally quantified. `pair<A, B>` means "for all types A
and B, a record with fields of those types." This is System F-style parametric
polymorphism.

### Trait definitions

New. Traits name a set of structural requirements on a type:

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

Traits can extend other traits:

```pact
trait ordered<T> : eq<T> {
    compare: func(a: T, b: T) -> s32
}
```

`ordered<T>` implies `eq<T>` — any type satisfying `ordered` must also satisfy
`eq`.

### Trait bounds

Type parameters can be constrained by trait bounds:

```pact
// Single bound
record cache<K: hashable, V> {
    // K must be hashable, V is unconstrained
    entries: list<pair<K, V>>,
}

// Multiple bounds
interface channel<T: serializable + eq> {
    exports {
        send: func(msg: T)
        recv: func() -> option<T>
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

Blanket implementations apply to parameterized types:

```pact
// Any list of serializable elements is itself serializable
impl<T: serializable> serializable<list<T>> {
    encode: func(value: list<T>) -> list<u8>
    decode: func(bytes: list<u8>) -> result<list<T>, string>
}
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

New. Interfaces can be combined algebraically:

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

Transforms are functions from interfaces to interfaces. They already exist in
Pact; the new type system makes them first-class and parameterizable:

```pact
// Transform definition
transform rpc<I> {
    // For each export in I:
    //   func(params...) -> T       becomes  func(params...) -> result<T, rpc-error>
    //   func(params...)            becomes  func(params...) -> result<_, rpc-error>

    type rpc-error = variant {
        timeout,
        actor-not-found(string),
        function-not-found(string),
        shutting-down,
        channel-closed,
        call-failed(string),
    }
}

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

// rpc is then just:
transform rpc<I> = fallible<I, rpc-error> + {
    type rpc-error = variant {
        timeout,
        actor-not-found(string),
        function-not-found(string),
        shutting-down,
        channel-closed,
        call-failed(string),
    }
}
```

The `map exports` block is pattern matching over function signatures. `{params}`
captures the parameter list, `{R}` captures the return type. This is the
type-level computation — it operates on the *structure of signatures*, not on
values.

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
- Trait bound declarations: `<T: serializable>` — you must state the constraints
- Top-level definitions: function signatures, type aliases — the public API is explicit

### Where annotations are inferred

- Type arguments at use sites: `pair<s32, s32>` can often be inferred
- Intermediate types in transform chains
- Trait satisfaction — if `T: serializable` is required and `my-type` has an impl, it's found automatically

## Interaction with Existing Systems

### Merkle Hashing

Structural hashing extends naturally to parameterized types:

- `pair<s32, s32>` and `pair<s32, s32>` have the same hash (trivially)
- `pair<s32, s32>` and `record foo { first: s32, second: s32 }` have the same
  hash (structural equivalence — type names excluded, field names included)
- A parameterized type `pair<A, B>` has no hash until instantiated — only
  concrete types are hashed

Trait bounds do not affect the hash. The hash captures *structure*, not
*constraints*. Constraints are a design-time property; compatibility is a
runtime property.

### Graph ABI (CGRF)

The ABI encodes concrete types. Parameterized types are always fully instantiated
before encoding — there are no type variables at the ABI layer. This means the
ABI is unchanged. `handler<my-state, my-msg>` becomes a concrete interface with
concrete function signatures; those signatures encode exactly as they do today.

### Transforms and Hashing

A transformed interface has its own hash, computed from the *result* of the
transform. `rpc(calculator)` produces a concrete interface with `result`-wrapped
returns; that interface is hashed like any other. The transform itself is not
part of the hash — only the resulting structure matters.

This means an interface written by hand that happens to match the output of
`rpc(calculator)` is compatible with it. Structure is what matters at the
boundary.

### Code Generation

Rust codegen from parameterized types produces generic Rust types:

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

Trait bounds in Pact map to trait bounds in Rust codegen. The generated code
carries the same constraints.

## Examples

### Theater handler pattern

The motivating example — every theater handler has the same shape:

```pact
trait serializable<T> {
    encode: func(value: T) -> list<u8>
    decode: func(bytes: list<u8>) -> result<T, string>
}

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

### Interface algebra

```pact
interface calculator {
    exports {
        add: func(a: s32, b: s32) -> s32
        sub: func(a: s32, b: s32) -> s32
        mul: func(a: s32, b: s32) -> s32
        div: func(a: s32, b: s32) -> result<s32, string>
    }
}

// A caller sees the rpc-wrapped version
interface calc-client = rpc(calculator)
// calc-client has:
//   add: func(a: s32, b: s32) -> result<s32, rpc-error>
//   sub: func(a: s32, b: s32) -> result<s32, rpc-error>
//   mul: func(a: s32, b: s32) -> result<s32, rpc-error>
//   div: func(a: s32, b: s32) -> result<result<s32, string>, rpc-error>

// Combine interfaces
interface scientific = calculator + {
    exports {
        sqrt: func(x: f64) -> result<f64, string>
        pow: func(base: f64, exp: f64) -> f64
    }
}

// Transform the combined interface
interface sci-client = rpc(scientific)
```

### Generic data structures in interfaces

```pact
record entry<K, V> {
    key: K,
    value: V,
}

interface key-value<K: hashable + serializable, V: serializable> {
    exports {
        get: func(key: K) -> option<V>
        put: func(key: K, value: V)
        delete: func(key: K) -> option<V>
        list: func() -> list<entry<K, V>>
    }
}

// Concrete instantiation
interface user-store = key-value<string, user-record>
```

### Transform composition

```pact
transform rpc<I> {
    map exports {
        func({params}) -> {R} => func({params}) -> result<{R}, rpc-error>
        func({params})         => func({params}) -> result<_, rpc-error>
    }
    type rpc-error = variant {
        timeout,
        actor-not-found(string),
        function-not-found(string),
        shutting-down,
        channel-closed,
        call-failed(string),
    }
}

transform traced<I> {
    map exports {
        func({params}) -> {R} => func(trace-id: string, {params}) -> {R}
    }
}

transform metered<I> {
    map exports {
        func({params}) -> {R} => func({params}) -> tuple<{R}, duration>
    }
    type duration = u64
}

// Compose: traced first, then rpc, then metered
interface observable-calc = metered(rpc(traced(calculator)))
// Result:
//   add: func(trace-id: string, a: s32, b: s32)
//        -> tuple<result<s32, rpc-error>, duration>
```

## Summary

| Feature | Status | Rationale |
|---------|--------|-----------|
| Primitive types | Unchanged | Already complete |
| Records, variants, enums, flags | Unchanged | Already complete |
| Recursive types | Unchanged | Already complete |
| Type parameters | **New** | Parameterize types and interfaces |
| Trait definitions | **New** | Name structural requirements |
| Trait bounds | **New** | Constrain type parameters |
| Trait implementations | **New** | Declare that types satisfy traits |
| Blanket impls | **New** | Generic trait satisfaction |
| Interface parameterization | **New** | Generic interfaces |
| Interface union | **New** | Combine interfaces algebraically |
| Interface extension | **New** | Add exports to existing interfaces |
| First-class transforms | **New** | Transforms defined in Pact, not Rust |
| Transform composition | Exists partially | Make composable and parameterizable |
| Transform patterns | **New** | Pattern matching over signatures |
| HM type inference | **New** | Infer type arguments at use sites |
| Subtyping | **Excluded** | Conflicts with structural hashing |
| Dependent types | **Excluded** | No access to values |
| Refinement types | **Excluded** | No access to values |
| General type recursion | **Excluded** | Keeps type checking total |
