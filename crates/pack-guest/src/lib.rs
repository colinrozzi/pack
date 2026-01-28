//! Guest-side helpers for Pack WASM packages.
//!
//! This crate provides macros and utilities for writing WASM packages
//! that use the Pack calling convention.
//!
//! # Example
//!
//! ```ignore
//! #![no_std]
//! extern crate alloc;
//!
//! use pack_guest::export;
//! use pack_guest::Value;
//!
//! // Set up panic handler and allocator (uses dlmalloc for proper memory management)
//! pack_guest::setup_guest!();
//!
//! #[export]
//! fn echo(input: Value) -> Value {
//!     input
//! }
//!
//! #[export]
//! fn double(n: i64) -> i64 {
//!     n * 2
//! }
//! ```

#![no_std]

pub extern crate alloc;

// Re-export the macros
pub use pack_guest_macros::{export, import, import_from, wit};

// Re-export useful types from composite-abi
pub use pack_abi::{decode, encode, ConversionError, Value};

// Re-export dlmalloc for the setup_guest macro
#[doc(hidden)]
pub use dlmalloc::GlobalDlmalloc as __GlobalDlmalloc;

// Re-export alloc for macro use
#[doc(hidden)]
pub use alloc as __alloc;

/// Internal implementation for the export macro.
///
/// This function handles the boilerplate of reading input, decoding,
/// calling the user's function, encoding output, and writing it back.
///
/// **Do not call this directly** - use the `#[export]` macro instead.
#[doc(hidden)]
pub fn __export_impl<F>(
    in_ptr: i32,
    in_len: i32,
    out_ptr: i32,
    out_cap: i32,
    f: F,
) -> i32
where
    F: FnOnce(Value) -> Result<Value, &'static str>,
{
    // Read input bytes
    let input_bytes = unsafe {
        let ptr = in_ptr as *const u8;
        let len = in_len as usize;
        core::slice::from_raw_parts(ptr, len)
    };

    // Decode input
    let input_value = match decode(input_bytes) {
        Ok(v) => v,
        Err(_) => return -1,
    };

    // Call user's function
    let output_value = match f(input_value) {
        Ok(v) => v,
        Err(_) => return -1,
    };

    // Encode output
    let output_bytes = match encode(&output_value) {
        Ok(b) => b,
        Err(_) => return -1,
    };

    // Check capacity
    if output_bytes.len() > out_cap as usize {
        return -1;
    }

    // Write output
    unsafe {
        let dst = out_ptr as *mut u8;
        core::ptr::copy_nonoverlapping(output_bytes.as_ptr(), dst, output_bytes.len());
    }

    output_bytes.len() as i32
}

/// Default output buffer size for imports (32KB)
const IMPORT_OUTPUT_BUFFER_SIZE: usize = 32 * 1024;

/// Internal implementation for the import macro.
///
/// This function handles the boilerplate of encoding input, calling
/// the raw import function, and decoding the result.
///
/// **Do not call this directly** - use the `#[import]` macro instead.
#[doc(hidden)]
pub fn __import_impl<F>(raw_fn: F, input: Value) -> Value
where
    F: FnOnce(i32, i32, i32, i32) -> i32,
{
    // Encode input
    let input_bytes = match encode(&input) {
        Ok(b) => b,
        Err(_) => panic!("failed to encode import input"),
    };

    // Prepare output buffer
    let mut output_buf = __alloc::vec![0u8; IMPORT_OUTPUT_BUFFER_SIZE];

    // Call the raw import function
    let result_len = raw_fn(
        input_bytes.as_ptr() as i32,
        input_bytes.len() as i32,
        output_buf.as_mut_ptr() as i32,
        output_buf.len() as i32,
    );

    if result_len < 0 {
        panic!("import function returned error");
    }

    // Decode the result
    let output_bytes = &output_buf[..result_len as usize];
    match decode(output_bytes) {
        Ok(v) => v,
        Err(_) => panic!("failed to decode import result"),
    }
}

/// A simple bump allocator for guest packages.
///
/// **Note**: For most use cases, prefer `setup_guest!()` which uses dlmalloc.
/// The bump allocator never deallocates memory, so it's only suitable for
/// short-lived packages or those with predictable memory usage.
///
/// # Example
///
/// ```ignore
/// pack_guest::bump_allocator!(64 * 1024); // 64KB heap
/// ```
#[macro_export]
macro_rules! bump_allocator {
    ($size:expr) => {
        mod __composite_allocator {
            use core::alloc::{GlobalAlloc, Layout};
            use core::cell::UnsafeCell;

            const HEAP_SIZE: usize = $size;

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
    };
}

/// Set up a panic handler that loops forever.
///
/// Use this in `no_std` packages.
///
/// # Example
///
/// ```ignore
/// pack_guest::panic_handler!();
/// ```
#[macro_export]
macro_rules! panic_handler {
    () => {
        #[panic_handler]
        fn panic(_info: &core::panic::PanicInfo) -> ! {
            loop {}
        }
    };
}

/// Convenience macro to set up dlmalloc allocator and panic handler.
///
/// This uses dlmalloc which properly supports deallocation, making it
/// suitable for long-running packages that allocate and free memory.
///
/// # Example
///
/// ```ignore
/// pack_guest::setup_guest!();
/// ```
#[macro_export]
macro_rules! setup_guest {
    () => {
        #[global_allocator]
        static __COMPOSITE_ALLOCATOR: $crate::__GlobalDlmalloc = $crate::__GlobalDlmalloc;
        $crate::panic_handler!();
    };
}
