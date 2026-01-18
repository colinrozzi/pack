//! Guest-side helpers for Composite WASM components.
//!
//! This crate provides macros and utilities for writing WASM components
//! that use the Composite calling convention.
//!
//! # Example
//!
//! ```ignore
//! #![no_std]
//! extern crate alloc;
//!
//! use composite_guest::export;
//! use composite_guest::Value;
//!
//! // Set up panic handler and allocator
//! composite_guest::setup_guest!();
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

extern crate alloc;

// Re-export the macro
pub use composite_guest_macros::export;

// Re-export useful types from composite-abi
pub use composite_abi::{decode, encode, ConversionError, Value};

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

/// A simple bump allocator for guest components.
///
/// Use this in `no_std` components that need heap allocation.
///
/// # Example
///
/// ```ignore
/// composite_guest::bump_allocator!(64 * 1024); // 64KB heap
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
/// Use this in `no_std` components.
///
/// # Example
///
/// ```ignore
/// composite_guest::panic_handler!();
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

/// Convenience macro to set up both allocator and panic handler.
///
/// # Example
///
/// ```ignore
/// composite_guest::setup_guest!(); // Uses default 64KB heap
/// composite_guest::setup_guest!(128 * 1024); // Custom heap size
/// ```
#[macro_export]
macro_rules! setup_guest {
    () => {
        $crate::bump_allocator!(64 * 1024);
        $crate::panic_handler!();
    };
    ($heap_size:expr) => {
        $crate::bump_allocator!($heap_size);
        $crate::panic_handler!();
    };
}
