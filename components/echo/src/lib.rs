//! A component that can decode, inspect, and re-encode graph values.
//!
//! This demonstrates using the composite-abi crate in a WASM component.

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

/// Output buffer starts at this offset in linear memory
const OUTPUT_OFFSET: usize = 32 * 1024; // 32KB into memory

/// Panic handler (required for no_std)
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {}
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

/// Echo: decode input, re-encode unchanged, return result
/// This proves we can decode and encode values in the component.
#[no_mangle]
pub extern "C" fn echo(in_ptr: i32, in_len: i32) -> i64 {
    unsafe {
        let input_bytes = read_input(in_ptr, in_len);

        // Decode the value
        let value = match decode(&input_bytes) {
            Ok(v) => v,
            Err(_) => return pack_result(0, 0), // Return empty on error
        };

        // Re-encode unchanged
        let output_bytes = match encode(&value) {
            Ok(b) => b,
            Err(_) => return pack_result(0, 0),
        };

        let (ptr, len) = write_output(&output_bytes);
        pack_result(ptr, len)
    }
}

/// Transform: decode input, modify the value, re-encode
/// Example: if it's an S64, double it; otherwise pass through
#[no_mangle]
pub extern "C" fn transform(in_ptr: i32, in_len: i32) -> i64 {
    unsafe {
        let input_bytes = read_input(in_ptr, in_len);

        let value = match decode(&input_bytes) {
            Ok(v) => v,
            Err(_) => return pack_result(0, 0),
        };

        // Transform: double any S64 values
        let transformed = transform_value(value);

        let output_bytes = match encode(&transformed) {
            Ok(b) => b,
            Err(_) => return pack_result(0, 0),
        };

        let (ptr, len) = write_output(&output_bytes);
        pack_result(ptr, len)
    }
}

/// Recursively transform values - double any S64
fn transform_value(value: Value) -> Value {
    match value {
        Value::S64(n) => Value::S64(n * 2),
        Value::List(items) => Value::List(items.into_iter().map(transform_value).collect()),
        Value::Tuple(items) => Value::Tuple(items.into_iter().map(transform_value).collect()),
        Value::Option(Some(inner)) => Value::Option(Some(alloc::boxed::Box::new(transform_value(*inner)))),
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
        // Other types pass through unchanged
        other => other,
    }
}
