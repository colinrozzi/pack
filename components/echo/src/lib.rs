//! A minimal echo component written in Rust.
//!
//! Takes (ptr, len) pointing to input bytes, copies them to output area,
//! and returns (out_ptr, out_len) packed as i64.

#![no_std]

use core::panic::PanicInfo;

/// Output buffer starts at this offset
const OUTPUT_OFFSET: usize = 4096;

/// Panic handler (required for no_std)
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {}
}

/// Echo: copy input bytes to output location and return (out_ptr, out_len)
/// Returns i64 where low 32 bits = out_ptr, high 32 bits = out_len
#[no_mangle]
pub extern "C" fn echo(in_ptr: i32, in_len: i32) -> i64 {
    let in_ptr = in_ptr as usize;
    let in_len = in_len as usize;
    let out_ptr = OUTPUT_OFFSET;

    // Copy bytes from input to output
    // Safety: we're in WASM, memory is linear and bounds-checked by the runtime
    unsafe {
        let src = in_ptr as *const u8;
        let dst = out_ptr as *mut u8;
        core::ptr::copy_nonoverlapping(src, dst, in_len);
    }

    // Pack (out_ptr, out_len) into i64: low 32 = ptr, high 32 = len
    (out_ptr as i64) | ((in_len as i64) << 32)
}
