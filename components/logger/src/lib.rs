//! A component that demonstrates using host imports.
//!
//! This component imports `host.log` to send log messages back to the host,
//! and `host.alloc` for memory allocation.

#![no_std]

extern crate alloc;

use alloc::vec::Vec;
use composite_abi::{decode, encode, Value};
use core::panic::PanicInfo;

// Simple bump allocator for WASM
mod allocator {
    use core::alloc::{GlobalAlloc, Layout};
    use core::cell::UnsafeCell;

    const HEAP_SIZE: usize = 64 * 1024; // 64KB heap

    #[repr(C, align(16))]
    struct Heap {
        data: UnsafeCell<[u8; HEAP_SIZE]>,
        offset: UnsafeCell<usize>,
    }

    unsafe impl Sync for Heap {}

    static HEAP: Heap = Heap {
        data: UnsafeCell::new([0; HEAP_SIZE]),
        offset: UnsafeCell::new(0),
    };

    pub struct BumpAllocator;

    unsafe impl GlobalAlloc for BumpAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let offset = &mut *HEAP.offset.get();
            let align = layout.align();
            let size = layout.size();

            // Align up
            let aligned = (*offset + align - 1) & !(align - 1);
            let new_offset = aligned + size;

            if new_offset > HEAP_SIZE {
                core::ptr::null_mut()
            } else {
                *offset = new_offset;
                (HEAP.data.get() as *mut u8).add(aligned)
            }
        }

        unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {
            // Bump allocator doesn't deallocate
        }
    }

    #[global_allocator]
    static ALLOCATOR: BumpAllocator = BumpAllocator;
}

// Import host functions from "host" module
#[link(wasm_import_module = "host")]
extern "C" {
    /// Log a message to the host
    fn log(ptr: i32, len: i32);

    /// Allocate memory from the host (returns pointer)
    fn alloc(size: i32) -> i32;
}

/// Output buffer starts at this offset in linear memory
const OUTPUT_OFFSET: usize = 32 * 1024; // 32KB into memory

/// Panic handler (required for no_std)
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {}
}

/// Helper to log a string to the host
fn host_log(msg: &str) {
    unsafe {
        log(msg.as_ptr() as i32, msg.len() as i32);
    }
}

/// Read input bytes from linear memory
unsafe fn read_input(ptr: i32, len: i32) -> Vec<u8> {
    let ptr = ptr as usize;
    let len = len as usize;
    let slice = core::slice::from_raw_parts(ptr as *const u8, len);
    slice.to_vec()
}

/// Write output bytes to linear memory
unsafe fn write_output(data: &[u8]) -> (i32, i32) {
    let dst = OUTPUT_OFFSET as *mut u8;
    core::ptr::copy_nonoverlapping(data.as_ptr(), dst, data.len());
    (OUTPUT_OFFSET as i32, data.len() as i32)
}

/// Pack (ptr, len) into i64 for return
fn pack_result(ptr: i32, len: i32) -> i64 {
    (ptr as i64) | ((len as i64) << 32)
}

/// Process a value with logging
/// Logs each step of processing and returns the transformed value.
#[no_mangle]
pub extern "C" fn process(in_ptr: i32, in_len: i32) -> i64 {
    host_log("process: starting");

    let input_bytes = unsafe { read_input(in_ptr, in_len) };
    host_log("process: read input bytes");

    // Decode the value
    let value = match decode(&input_bytes) {
        Ok(v) => {
            host_log("process: decoded value successfully");
            v
        }
        Err(_) => {
            host_log("process: ERROR - failed to decode");
            return pack_result(0, 0);
        }
    };

    // Log what kind of value we got
    match &value {
        Value::S64(n) => {
            host_log("process: got S64 value");
            // Log the actual number (convert to string)
            let mut buf = [0u8; 20];
            let s = format_i64(*n, &mut buf);
            host_log(s);
        }
        Value::String(s) => {
            host_log("process: got String value");
            host_log(s);
        }
        Value::List(_) => host_log("process: got List value"),
        Value::Record(_) => host_log("process: got Record value"),
        Value::Variant { .. } => host_log("process: got Variant value"),
        _ => host_log("process: got other value type"),
    }

    // Transform: double any S64 values
    let transformed = transform_value(value);
    host_log("process: transformed value");

    // Re-encode
    let output_bytes = match encode(&transformed) {
        Ok(b) => {
            host_log("process: encoded result");
            b
        }
        Err(_) => {
            host_log("process: ERROR - failed to encode");
            return pack_result(0, 0);
        }
    };

    let (ptr, len) = unsafe { write_output(&output_bytes) };
    host_log("process: done");
    pack_result(ptr, len)
}

/// Simple function to test host.alloc
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
        Value::List(items) => Value::List(items.into_iter().map(transform_value).collect()),
        Value::Tuple(items) => Value::Tuple(items.into_iter().map(transform_value).collect()),
        Value::Option(Some(inner)) => {
            Value::Option(Some(alloc::boxed::Box::new(transform_value(*inner))))
        }
        Value::Variant { tag, payload } => Value::Variant {
            tag,
            payload: payload.map(|p| alloc::boxed::Box::new(transform_value(*p))),
        },
        Value::Record(fields) => Value::Record(
            fields
                .into_iter()
                .map(|(name, val)| (name, transform_value(val)))
                .collect(),
        ),
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
