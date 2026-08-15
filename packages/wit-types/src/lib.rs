//! Regression fixture for `wit!` type codegen. Exercises exactly the shapes that
//! were broken before the switch to `#[derive(GraphValue)]`:
//!   - a RECORD (was `Value::Record(vec)`, a tuple variant — didn't compile),
//!     including an `option<...>` field (needs FromValue decode) and a `list`,
//!   - a VARIANT (was `Value::Variant { tag, payload }`, missing type_name/
//!     case_name and Option-payload) with a nested-record payload,
//!   - a C-like ENUM (same variant bug).
//!
//! An exported function uses the record so the derive's From/TryFrom impls are
//! actually instantiated and compiled. If this package builds, the codegen and
//! its marshalling are correct.
//!
//! This fixture deliberately still calls the deprecated `wit!` alias (rather
//! than `pact!`) so it also pins that the alias keeps forwarding to `pact!` —
//! hence the crate-level `#![allow(deprecated)]`.

#![no_std]
#![allow(deprecated)]

extern crate alloc;

use packr_guest::export;

packr_guest::setup_guest!();

packr_guest::wit! {
    record point {
        x: s32,
        y: s32,
        label: option<string>,
        tags: list<string>,
    }

    variant shape {
        circle(f32),
        rect(point),
        nothing,
    }

    enum color {
        red,
        green,
        blue,
    }

    // Recursive variant: a direct self-reference (neg -> Rec<Sexpr>) and a list
    // self-reference (lst -> Vec<Sexpr>, no indirection wrapper needed).
    variant sexpr {
        sym(string),
        num(s64),
        neg(self),
        lst(list<self>),
    }

    // Recursive STRUCT: the self-reference is optional (the base case), so the
    // field is `Option<Rec<Cons>>` — the shape a bare `Box` could never decode.
    record cons {
        head: s64,
        tail: option<self>,
    }

    // `map<K, V>` field: lowers to `BTreeMap<K, V>` and marshals as a
    // key-sorted `list<tuple<K, V>>`.
    record dict {
        name: string,
        entries: map<string, s32>,
    }

    world wit-types {
        export identity: func(p: point) -> point
        export eval: func(e: sexpr) -> sexpr
        export cons-id: func(c: cons) -> cons
        export dict-id: func(d: dict) -> dict
    }
}

/// Forces the generated `Point` From/TryFrom (via the derive) to compile.
#[export]
fn identity(p: Point) -> Point {
    // Touch a field and reconstruct, exercising both directions.
    Point {
        x: p.x + 1,
        y: p.y,
        label: p.label,
        tags: p.tags,
    }
}

/// Forces the recursive `Sexpr` marshalling (Rec<Sexpr> for `neg`, Vec<Sexpr>
/// for `lst`) to compile.
#[export]
fn eval(e: Sexpr) -> Sexpr {
    e
}

/// Forces the recursive-STRUCT `Cons` marshalling (Option<Rec<Cons>>) to
/// compile — the case a bare `Box` could not handle.
#[export]
fn cons_id(c: Cons) -> Cons {
    c
}

/// Forces the `map<string, s32>` -> `BTreeMap<String, i32>` field marshalling to
/// compile (encodes/decodes as a key-sorted list<tuple<string, s32>>).
#[export]
fn dict_id(d: Dict) -> Dict {
    let mut entries = d.entries;
    entries.entry(alloc::string::String::from("_count")).or_insert(0);
    Dict {
        name: d.name,
        entries,
    }
}

// Reference `Shape`/`Color` so their generated impls are compiled too.
#[allow(dead_code)]
fn _touch(s: Shape, c: Color) -> (packr_guest::Value, packr_guest::Value) {
    (s.into(), c.into())
}
