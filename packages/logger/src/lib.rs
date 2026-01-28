//! A package that demonstrates using host imports.
//!
//! This package imports `host.log` to send log messages back to the host,
//! and `host.alloc` for memory allocation.

#![no_std]

extern crate alloc;

use alloc::boxed::Box;
use pack_guest::{export, Value};

// Set up allocator and panic handler
pack_guest::setup_guest!();

// Import host functions from "host" module
#[link(wasm_import_module = "host")]
extern "C" {
    /// Log a message to the host
    fn log(ptr: i32, len: i32);

    /// Allocate memory from the host (returns pointer)
    fn alloc(size: i32) -> i32;
}

/// Helper to log a string to the host
fn host_log(msg: &str) {
    unsafe {
        log(msg.as_ptr() as i32, msg.len() as i32);
    }
}

/// Process a value with logging.
/// Logs each step of processing and returns the transformed value.
#[export]
fn process(value: Value) -> Value {
    host_log("process: starting");
    host_log("process: decoded value successfully");

    // Log what kind of value we got
    match &value {
        Value::S64(n) => {
            host_log("process: got S64 value");
            let mut buf = [0u8; 20];
            let s = format_i64(*n, &mut buf);
            host_log(s);
        }
        Value::String(s) => {
            host_log("process: got String value");
            host_log(s);
        }
        Value::List { .. } => host_log("process: got List value"),
        Value::Record { .. } => host_log("process: got Record value"),
        Value::Variant { .. } => host_log("process: got Variant value"),
        _ => host_log("process: got other value type"),
    }

    // Transform: double any S64 values
    let transformed = transform_value(value);
    host_log("process: transformed value");
    host_log("process: done");

    transformed
}

/// Simple function to test host.alloc (raw WASM function, not Graph ABI)
#[no_mangle]
pub extern "C" fn test_alloc(size: i32) -> i32 {
    host_log("test_alloc: requesting memory from host");
    let ptr = unsafe { alloc(size) };
    host_log("test_alloc: got memory");
    ptr
}

/// Recursively transform values - double any S64
fn transform_value(value: Value) -> Value {
    match value {
        Value::S64(n) => Value::S64(n * 2),
        Value::List { elem_type, items } => Value::List {
            elem_type,
            items: items.into_iter().map(transform_value).collect(),
        },
        Value::Tuple(items) => Value::Tuple(items.into_iter().map(transform_value).collect()),
        Value::Option { inner_type, value: Some(inner) } => Value::Option {
            inner_type,
            value: Some(Box::new(transform_value(*inner))),
        },
        Value::Variant { type_name, case_name, tag, payload } => Value::Variant {
            type_name,
            case_name,
            tag,
            payload: payload.into_iter().map(transform_value).collect(),
        },
        Value::Record { type_name, fields } => Value::Record {
            type_name,
            fields: fields
                .into_iter()
                .map(|(name, val)| (name, transform_value(val)))
                .collect(),
        },
        other => other,
    }
}

/// Format an i64 as a string (no_std compatible)
fn format_i64(mut n: i64, buf: &mut [u8; 20]) -> &str {
    let negative = n < 0;
    if negative {
        n = -n;
    }

    let mut i = buf.len();
    if n == 0 {
        i -= 1;
        buf[i] = b'0';
    } else {
        while n > 0 {
            i -= 1;
            buf[i] = b'0' + (n % 10) as u8;
            n /= 10;
        }
    }

    if negative {
        i -= 1;
        buf[i] = b'-';
    }

    unsafe { core::str::from_utf8_unchecked(&buf[i..]) }
}
