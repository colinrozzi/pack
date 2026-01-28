//! Example actor using the world! macro for typed imports and exports.
//!
//! This demonstrates how to use the new `world!()` macro which:
//! - Generates typed import modules from WIT+ interfaces
//! - Auto-discovers export names for the `#[export]` macro
//!
//! The WIT+ world is defined in `wit/world.wit+`.

#![no_std]
extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use pack_guest::export;

// Set up panic handler and allocator
pack_guest::setup_guest!();

// Parse the WIT+ world and generate:
// - Typed import modules (runtime::log, runtime::get_time)
// - Export metadata for #[export] validation
pack_guest::world!();

/// Initialize the actor.
///
/// The `#[export]` macro automatically discovers that this function
/// matches the `actor.init` export in the WIT+ world and uses the
/// correct export name.
#[export]
fn init(state: Option<Vec<u8>>, actor_id: String) -> Result<Option<Vec<u8>>, String> {
    // Use the generated typed import - no Value conversion needed!
    runtime::log("Actor initializing!");

    // Get current time using typed import
    let time = runtime::get_time();
    runtime::log(&alloc::format!("Current time: {}", time));

    runtime::log(&alloc::format!("Actor ID: {}", actor_id));

    // Return the initial state
    Ok(state)
}

/// Handle an incoming message.
///
/// Again, the export name is auto-discovered from the WIT+ world.
#[export]
fn handle(state: Option<Vec<u8>>, msg: Vec<u8>) -> Option<Vec<u8>> {
    runtime::log(&alloc::format!("Received {} bytes", msg.len()));

    // Echo back the message as the new state
    Some(msg)
}
