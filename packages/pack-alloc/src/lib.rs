//! The default in-wasm allocator module for Pack (1b).
//!
//! Exports `alloc` / `dealloc` backed by a real free-list allocator
//! (`dlmalloc`), so memory is **reclaimed** — unlike the 1a host bump provider,
//! whose no-op `dealloc` makes a long-lived actor's memory climb every call.
//!
//! The runtime wires these exports to a package's `pack:alloc` imports. The
//! allocator manages the linear memory it runs on; when it shares a package's
//! memory (via an imported `pack:mem.memory`), its heap sits above the
//! package's static data, so it reclaims and reuses freed blocks and keeps
//! total memory bounded under sustained call volume.

#![no_std]

use core::panic::PanicInfo;
use dlmalloc::Dlmalloc;

/// The allocator arena. Its bookkeeping and the heap it manages live in this
/// module's linear memory (above its own static data / `__heap_base`).
static mut ALLOCATOR: Dlmalloc = Dlmalloc::new();

/// Allocate `size` bytes with `align` alignment. Returns 0 (null) on failure.
///
/// Signature matches the `pack:alloc.alloc` import: `(size, align) -> ptr`.
#[no_mangle]
pub extern "C" fn alloc(size: usize, align: usize) -> *mut u8 {
    if size == 0 {
        return core::ptr::null_mut();
    }
    // SAFETY: single-threaded wasm; no reentrant access to ALLOCATOR.
    unsafe { (*core::ptr::addr_of_mut!(ALLOCATOR)).malloc(size, align.max(1)) }
}

/// Free a block previously returned by [`alloc`].
///
/// Signature matches `pack:alloc.dealloc`: `(ptr, size, align)`.
#[no_mangle]
pub extern "C" fn dealloc(ptr: *mut u8, size: usize, align: usize) {
    if ptr.is_null() || size == 0 {
        return;
    }
    // SAFETY: ptr/size/align must match a prior `alloc`; single-threaded wasm.
    unsafe { (*core::ptr::addr_of_mut!(ALLOCATOR)).free(ptr, size, align.max(1)) }
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    core::arch::wasm32::unreachable()
}
