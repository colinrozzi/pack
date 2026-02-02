//! A package with concretely typed exports for testing type-aware interop.
//!
//! Unlike the doubler/echo packages which use the dynamic Value type,
//! this package uses concrete scalar types (s32, s64, f64, bool) and strings.

#![no_std]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use pack_guest::export;

pack_guest::setup_guest!();

pack_guest::pack_types! {
    exports {
        add: func(a: s32, b: s32) -> s32,
        add64: func(a: s64, b: s64) -> s64,
        add_f64: func(a: f64, b: f64) -> f64,
        negate: func(n: s32) -> s32,
        is_positive: func(n: s32) -> bool,
        greet: func(name: string) -> string,
        str_len: func(s: string) -> s32,
    }
}

#[export]
fn add(a: i32, b: i32) -> i32 {
    a + b
}

#[export]
fn add64(a: i64, b: i64) -> i64 {
    a + b
}

#[export]
fn add_f64(a: f64, b: f64) -> f64 {
    a + b
}

#[export]
fn negate(n: i32) -> i32 {
    -n
}

#[export]
fn is_positive(n: i32) -> bool {
    n > 0
}

#[export]
fn greet(name: String) -> String {
    format!("Hello, {}!", name)
}

#[export]
fn str_len(s: String) -> i32 {
    s.len() as i32
}
