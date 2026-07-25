//! Provider for the `import_from`-decodes-Result regression test. Exports a
//! `Result`-returning function so the consumer's `#[import_from]` stub has to
//! decode a `result<...>` — the case that used to require a `TryFrom<Value>` shim.

#![no_std]

extern crate alloc;

use alloc::boxed::Box;
use alloc::string::String;
use packr_guest::{export, Value, ValueType};

packr_guest::setup_guest!();

packr_guest::pack_types! {
    exports {
        mathr {
            checked: func(n: s64) -> result<s64, string>,
        }
    }
}

/// `checked(n) = Ok(n*2)` for `n >= 0`, else `Err`.
#[export]
fn checked(input: Value) -> Value {
    let n = match input {
        Value::S64(n) => n,
        other => return other,
    };
    let value = if n < 0 {
        Err(Box::new(Value::String(String::from("negative"))))
    } else {
        Ok(Box::new(Value::S64(n * 2)))
    };
    Value::Result {
        ok_type: ValueType::S64,
        err_type: ValueType::String,
        value,
    }
}
