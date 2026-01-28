# Pack `world!` Macro Design

## Overview

The `world!` macro reads the WIT+ world definition and generates:
1. All type definitions (records, variants, enums, flags)
2. Typed import function stubs
3. Export signature metadata for `#[export]` validation

## Example WIT+ World

```wit
// wit/world.wit+
package theater:simple

interface runtime {
    log: func(msg: string)
    get-time: func() -> u64
    shutdown: func(error: option<string>)
}

interface actor {
    init: func(state: option<list<u8>>, params: tuple<string>) -> result<option<list<u8>>, string>
    handle: func(state: option<list<u8>>, msg: list<u8>) -> option<list<u8>>
}

world my-actor {
    import theater:simple/runtime
    export theater:simple/actor
}
```

## User Code

```rust
#![no_std]
extern crate alloc;

use pack_guest::export;

// Parse world.wit+ and generate types + imports
pack_guest::world!();

#[export]
fn init(state: Option<Vec<u8>>, params: (String,)) -> Result<Option<Vec<u8>>, String> {
    // Use generated import - fully typed!
    runtime::log("Actor starting!");

    Ok(state)
}

#[export]
fn handle(state: Option<Vec<u8>>, msg: Vec<u8>) -> Option<Vec<u8>> {
    runtime::log("Got message");
    state
}
```

## Macro Expansion

The `world!()` macro expands to:

```rust
// ============================================================================
// Generated Types (from WIT+ type definitions)
// ============================================================================

// (No custom types in this example, but records/variants would appear here)

// ============================================================================
// Generated Imports Module
// ============================================================================

pub mod runtime {
    //! Import functions from theater:simple/runtime

    use super::*;

    /// Log a message
    pub fn log(msg: &str) {
        #[link(wasm_import_module = "theater:simple/runtime")]
        extern "C" {
            #[link_name = "log"]
            fn __raw_log(in_ptr: i32, in_len: i32, out_ptr: i32, out_cap: i32) -> i32;
        }

        let input = pack_guest::Value::String(::alloc::string::String::from(msg));
        let _ = pack_guest::__import_impl(
            |a, b, c, d| unsafe { __raw_log(a, b, c, d) },
            input,
        );
    }

    /// Get current time
    pub fn get_time() -> u64 {
        #[link(wasm_import_module = "theater:simple/runtime")]
        extern "C" {
            #[link_name = "get-time"]
            fn __raw_get_time(in_ptr: i32, in_len: i32, out_ptr: i32, out_cap: i32) -> i32;
        }

        let input = pack_guest::Value::Tuple(::alloc::vec![]);
        let result = pack_guest::__import_impl(
            |a, b, c, d| unsafe { __raw_get_time(a, b, c, d) },
            input,
        );
        match result {
            pack_guest::Value::U64(v) => v,
            _ => panic!("unexpected return type from get-time"),
        }
    }

    /// Shutdown the actor
    pub fn shutdown(error: Option<&str>) {
        #[link(wasm_import_module = "theater:simple/runtime")]
        extern "C" {
            #[link_name = "shutdown"]
            fn __raw_shutdown(in_ptr: i32, in_len: i32, out_ptr: i32, out_cap: i32) -> i32;
        }

        let input = match error {
            Some(e) => pack_guest::Value::Option {
                inner_type: pack_guest::ValueType::String,
                value: Some(::alloc::boxed::Box::new(
                    pack_guest::Value::String(::alloc::string::String::from(e))
                )),
            },
            None => pack_guest::Value::Option {
                inner_type: pack_guest::ValueType::String,
                value: None,
            },
        };
        let _ = pack_guest::__import_impl(
            |a, b, c, d| unsafe { __raw_shutdown(a, b, c, d) },
            input,
        );
    }
}

// ============================================================================
// Export Metadata (for #[export] validation)
// ============================================================================

#[doc(hidden)]
mod __pack_world_exports {
    //! Metadata about expected exports for compile-time validation

    pub const EXPORTS: &[(&str, &str)] = &[
        ("init", "func(state: option<list<u8>>, params: tuple<string>) -> result<option<list<u8>>, string>"),
        ("handle", "func(state: option<list<u8>>, msg: list<u8>) -> option<list<u8>>"),
    ];
}
```

## How `#[export]` Uses This

The `#[export]` macro:
1. Looks up the function name in `__pack_world_exports::EXPORTS`
2. Parses the expected WIT signature
3. Validates the Rust function signature matches
4. Generates the wrapper with correct Value conversion

```rust
// User writes:
#[export]
fn init(state: Option<Vec<u8>>, params: (String,)) -> Result<Option<Vec<u8>>, String> {
    runtime::log("Actor starting!");
    Ok(state)
}

// Macro generates:
fn __init_inner(state: Option<Vec<u8>>, params: (String,)) -> Result<Option<Vec<u8>>, String> {
    runtime::log("Actor starting!");
    Ok(state)
}

#[export_name = "theater:simple/actor.init"]
pub extern "C" fn __init_export(
    in_ptr: i32, in_len: i32, out_ptr: i32, out_cap: i32
) -> i32 {
    pack_guest::__export_impl(in_ptr, in_len, out_ptr, out_cap, |value| {
        // Extract params from tuple (validated at compile time to match WIT)
        let items = match value {
            pack_guest::Value::Tuple(items) if items.len() == 2 => items,
            _ => return Err("expected tuple of 2 params"),
        };

        let state: Option<Vec<u8>> = items[0].clone().try_into()
            .map_err(|_| "failed to convert state")?;
        let params: (String,) = items[1].clone().try_into()
            .map_err(|_| "failed to convert params")?;

        // Call user function
        let result = __init_inner(state, params);

        // Convert to Value
        Ok(result.into())
    })
}
```

## Compile-Time Validation

If the user's function signature doesn't match the WIT:

```rust
#[export]
fn init(state: String) -> i32 {  // WRONG types!
    42
}
```

Compile error:
```
error: export function `init` signature mismatch
  --> src/lib.rs:10:1
   |
10 | fn init(state: String) -> i32 {
   | ^^^^^^^^^^^^^^^^^^^^^^^^^^^^
   |
   = expected: func(state: option<list<u8>>, params: tuple<string>) -> result<option<list<u8>>, string>
   = got: fn(String) -> i32
   = help: state should be Option<Vec<u8>>, not String
   = help: missing parameter: params (tuple<string>)
   = help: return type should be Result<Option<Vec<u8>>, String>, not i32
```

## Benefits

1. **Single source of truth** - WIT+ defines the contract
2. **Type-safe imports** - `runtime::log(msg)` is fully typed
3. **Compile-time validation** - Mismatched exports fail at compile time
4. **No signature duplication** - Types derived from WIT+
5. **Clean DX** - Just write typed Rust functions

## Type Mapping

| WIT+ Type | Rust Type |
|-----------|-----------|
| `bool` | `bool` |
| `u8/u16/u32/u64` | `u8/u16/u32/u64` |
| `s8/s16/s32/s64` | `i8/i16/i32/i64` |
| `f32/f64` | `f32/f64` |
| `char` | `char` |
| `string` | `String` (owned) or `&str` (for import params) |
| `list<T>` | `Vec<T>` |
| `option<T>` | `Option<T>` |
| `result<T, E>` | `Result<T, E>` |
| `tuple<A, B>` | `(A, B)` |
| Custom record | Generated struct |
| Custom variant | Generated enum |

## Implementation Steps

1. Extend `wit_parser.rs` to resolve interface paths and merge interfaces
2. Add `generate_imports_module()` to `codegen.rs`
3. Add export metadata generation
4. Update `#[export]` to validate against the world
5. Better error messages with span info
