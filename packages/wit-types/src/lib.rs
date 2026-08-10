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

#![no_std]

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

    // Recursive variant: a direct self-reference (neg -> Box<Sexpr>, handled by
    // the derive's Box field decode) and a list self-reference (lst ->
    // Vec<Sexpr>, no Box needed).
    variant sexpr {
        sym(string),
        num(s64),
        neg(self),
        lst(list<self>),
    }

    world wit-types {
        export identity: func(p: point) -> point
        export eval: func(e: sexpr) -> sexpr
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

/// Forces the recursive `Sexpr` marshalling (Box<Sexpr> for `neg`, Vec<Sexpr>
/// for `lst`) to compile.
#[export]
fn eval(e: Sexpr) -> Sexpr {
    e
}

// Reference `Shape`/`Color` so their generated impls are compiled too.
#[allow(dead_code)]
fn _touch(s: Shape, c: Color) -> (packr_guest::Value, packr_guest::Value) {
    (s.into(), c.into())
}
