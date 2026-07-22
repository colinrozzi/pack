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
//! use packr_guest::export;
//! use packr_guest::Value;
//!
//! // Set up panic handler and allocator (uses dlmalloc for proper memory management)
//! packr_guest::setup_guest!();
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
pub use packr_guest_macros::{export, import, import_from, pack_types, wit, world};

// Re-export useful types from pack-abi
pub use packr_abi::{decode, encode, ConversionError, FromValue, KnownValueType, Value, ValueType};

// Re-export derive macro
#[cfg(feature = "derive")]
pub use packr_derive::GraphValue;

// Re-export pack-abi as composite_abi for derive macro compatibility
// The derive macro generates code that references composite_abi
pub use packr_abi as composite_abi;

// Re-export alloc for macro use
#[doc(hidden)]
pub use alloc as __alloc;

/// A `dlmalloc`-backed `#[global_allocator]` **linked into** the actor — no
/// imported `pack:alloc` provider. This is what makes an actor a plain
/// `cargo build`: the allocator's bookkeeping and the heap it manages live in
/// the actor's own linear memory (above its static data), which the actor grows
/// via `memory.grow` as needed. Because nothing is imported, the module needs no
/// composition/fusion step — it loads directly. `setup_guest!` installs it.
pub struct DlmallocAllocator;

// SAFETY: single-threaded wasm; no reentrant access to the arena.
static mut DLMALLOC: dlmalloc::Dlmalloc = dlmalloc::Dlmalloc::new();

unsafe impl core::alloc::GlobalAlloc for DlmallocAllocator {
    #[inline]
    unsafe fn alloc(&self, layout: core::alloc::Layout) -> *mut u8 {
        (*core::ptr::addr_of_mut!(DLMALLOC)).malloc(layout.size(), layout.align().max(1))
    }

    #[inline]
    unsafe fn dealloc(&self, ptr: *mut u8, layout: core::alloc::Layout) {
        (*core::ptr::addr_of_mut!(DLMALLOC)).free(ptr, layout.size(), layout.align().max(1));
    }
}

/// Internal implementation for the export macro.
///
/// This function handles the boilerplate of reading input, decoding,
/// calling the user's function, encoding output, and returning a pointer to it.
///
/// # ABI
///
/// ```text
/// fn export(in_ptr: i32, in_len: i32, out_ptr_ptr: i32, out_len_ptr: i32) -> i32
/// ```
///
/// - `in_ptr`, `in_len`: Input data location (Graph ABI encoded)
/// - `out_ptr_ptr`: Location where guest writes output pointer
/// - `out_len_ptr`: Location where guest writes output length
/// - Returns: 0 = success, -1 = error
///
/// On success, the output ptr/len point to the Graph ABI encoded result.
/// On error, the output ptr/len point to a UTF-8 error message.
///
/// The host must call `__pack_free(ptr, len)` to free the output buffer.
///
/// **Do not call this directly** - use the `#[export]` macro instead.
#[doc(hidden)]
pub fn __export_impl<F>(in_ptr: i32, in_len: i32, out_ptr_ptr: i32, out_len_ptr: i32, f: F) -> i32
where
    F: FnOnce(Value) -> Result<Value, &'static str>,
{
    // Helper to write error and return -1
    let write_error = |msg: &str| -> i32 {
        let bytes = msg.as_bytes();
        // Allocate exactly the size we need
        let mut buf = alloc::vec::Vec::with_capacity(bytes.len());
        buf.extend_from_slice(bytes);
        // Vec::with_capacity ensures capacity == length here

        let ptr = buf.as_ptr() as i32;
        let len = buf.len() as i32;
        core::mem::forget(buf);

        unsafe {
            core::ptr::write(out_ptr_ptr as *mut i32, ptr);
            core::ptr::write(out_len_ptr as *mut i32, len);
        }
        -1
    };

    // Read input bytes
    let input_bytes = unsafe {
        let ptr = in_ptr as *const u8;
        let len = in_len as usize;
        core::slice::from_raw_parts(ptr, len)
    };

    // Decode input
    let input_value = match decode(input_bytes) {
        Ok(v) => v,
        Err(e) => return write_error(&alloc::format!("decode error: {:?}", e)),
    };

    // Call user's function
    let output_value = match f(input_value) {
        Ok(v) => v,
        Err(e) => return write_error(e),
    };

    // Encode output
    let mut output_bytes = match encode(&output_value) {
        Ok(b) => b,
        Err(e) => return write_error(&alloc::format!("encode error: {:?}", e)),
    };

    // Shrink to fit ensures capacity == length for proper deallocation
    output_bytes.shrink_to_fit();
    let ptr = output_bytes.as_ptr() as i32;
    let len = output_bytes.len() as i32;
    core::mem::forget(output_bytes);

    unsafe {
        core::ptr::write(out_ptr_ptr as *mut i32, ptr);
        core::ptr::write(out_len_ptr as *mut i32, len);
    }

    0 // Success
}

/// Placeholder symbols for PIC side-module builds (the `pic` feature).
///
/// Newer rustc (>1.92) injects `--export=__heap_base` and `--export=__data_end`
/// into every wasm-cdylib link. Under `-shared` (the PIC recipe) lld does not
/// synthesize those symbols — the real heap base is supplied by the loader via
/// the GOT — so the injected exports fail to resolve and the link aborts. These
/// zero placeholders satisfy the exports; nothing reads them (the package drives
/// allocation through `pack:alloc`, and the loader owns the heap via GOT globals).
///
/// Gated behind `pic` because a NON-PIC (non-`-shared`) build's lld defines these
/// symbols itself, and a second definition here would be a duplicate-symbol error.
#[cfg(feature = "pic")]
#[no_mangle]
pub static __heap_base: u8 = 0;

#[cfg(feature = "pic")]
#[no_mangle]
pub static __data_end: u8 = 0;

/// Allocate a buffer in guest memory.
///
/// The host calls this to allocate space for input data before calling an export.
/// Returns a pointer to the allocated buffer, or 0 on allocation failure.
///
/// The host must call `__pack_free(ptr, len)` to free this buffer after use.
#[no_mangle]
pub extern "C" fn __pack_alloc(size: i32) -> i32 {
    if size <= 0 {
        return 0;
    }
    unsafe {
        // Use raw allocation to ensure size matches exactly what we'll dealloc
        let layout = core::alloc::Layout::from_size_align_unchecked(size as usize, 1);
        let ptr = alloc::alloc::alloc_zeroed(layout);
        if ptr.is_null() {
            0
        } else {
            ptr as i32
        }
    }
}

/// Free a buffer allocated by the guest.
///
/// The host must call this after reading the output from an export call,
/// and after using input buffers allocated via `__pack_alloc`.
///
/// # Safety
///
/// The `ptr` and `len` must have been returned from `__pack_alloc` or
/// from an export call's result slots. Calling with invalid values is UB.
#[no_mangle]
pub extern "C" fn __pack_free(ptr: i32, len: i32) {
    if ptr == 0 || len == 0 {
        return;
    }
    unsafe {
        // Use the global allocator directly to deallocate.
        // Create a layout matching what was allocated.
        let layout = core::alloc::Layout::from_size_align_unchecked(len as usize, 1);
        alloc::alloc::dealloc(ptr as *mut u8, layout);
    }
}

/// Internal implementation for the import macro.
///
/// This function handles the boilerplate of encoding input, calling
/// the raw import function, and decoding the result.
///
/// Uses the guest-allocates ABI:
/// - `fn(in_ptr, in_len, out_ptr_ptr, out_len_ptr) -> status`
/// - status `< 0` = error
/// - status `0` = success, return buffer is host-owned (do not free)
/// - status `1` = success, return buffer is guest-allocated (we own it, free it)
/// - The callee writes result ptr/len to the provided slots
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

    // Prepare slots for the callee to write result ptr/len
    let mut out_ptr: i32 = 0;
    let mut out_len: i32 = 0;

    // Call the raw import function with guest-allocates ABI
    let status = raw_fn(
        input_bytes.as_ptr() as i32,
        input_bytes.len() as i32,
        &mut out_ptr as *mut i32 as i32,
        &mut out_len as *mut i32 as i32,
    );

    if status < 0 {
        panic!("import function returned error");
    }

    // The status code also signals ownership of the return buffer:
    //   1 = the host guest-allocated it via `__pack_alloc` (we own it, must free);
    //   0 = the host wrote it into its own fixed scratch buffer (must NOT free).
    // Async host fns (theater's large, unbounded returns) take the guest-allocated
    // path; sync host fns use the host-owned scratch. Freeing the scratch buffer
    // would hand the allocator a pointer it never allocated, so this guard is
    // load-bearing. See `write_host_output_async` on the host side.
    let guest_owned = status == 1;

    // Decode the result, scoping the borrow so the buffer is no longer aliased
    // before we hand it back to the allocator.
    let decoded = {
        let output_bytes =
            unsafe { core::slice::from_raw_parts(out_ptr as *const u8, out_len as usize) };
        decode(output_bytes)
    };

    // Release the guest-allocated buffer now that its bytes have been decoded.
    if guest_owned {
        __pack_free(out_ptr, out_len);
    }

    match decoded {
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
/// packr_guest::bump_allocator!(64 * 1024); // 64KB heap
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
/// packr_guest::panic_handler!();
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

/// Convenience macro to set up the linked-in allocator and panic handler.
///
/// The actor **links in** its own allocator: it installs
/// [`DlmallocAllocator`] as its `#[global_allocator]`, so the allocator's
/// bookkeeping and the heap it manages live in the actor's own linear memory.
/// Nothing is imported, so the module needs no composition/fusion step — it
/// loads directly as a plain `cargo build` cdylib.
///
/// # Example
///
/// ```ignore
/// packr_guest::setup_guest!();
/// ```
#[macro_export]
macro_rules! setup_guest {
    () => {
        #[global_allocator]
        static __PACK_ALLOCATOR: $crate::DlmallocAllocator = $crate::DlmallocAllocator;
        $crate::panic_handler!();
    };
}
