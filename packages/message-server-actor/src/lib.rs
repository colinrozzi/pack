//! Canonical **stateful message-server actor** for packr-guest 0.18 — the
//! reference for a **typed actor-state** guest whose handlers thread that state
//! and, in some cases, ALSO return a response.
//!
//! This is the shape to mirror for the theater `message-server-client` exports
//! (handle-request / handle-channel-open with a response; handle-send /
//! handle-channel-message / handle-channel-close with state only). It builds to
//! wasm in CI (`tests/message_server_actor.rs`), so the shape can't drift.
//!
//! ## The load-bearing detail: the state-mode return shape
//!
//! `#[export(state = "S")]` wraps `Ok((new_state, output))` into the Value
//! `Result(Ok( Tuple([ new_state.into(), output.into() ]) ))`. So the ok-body is
//! a 2-tuple: **element 0 = new state, element 1 = `output.into()` verbatim**.
//!
//! The theater message-server host reads element 1 as the *response tuple*. So a
//! handler that returns a response of `option<list<u8>>` must make element 1 a
//! **1-tuple** `(response,)` — i.e. its `output` is the Rust 1-tuple
//! `(Option<Vec<u8>>,)`, giving `Result<(S, (Option<Vec<u8>>,)), String>`.
//! (Using a bare `Option<Vec<u8>>` would make element 1 a bare option, not a
//! 1-tuple — the wrong shape.)
//!
//! A state-only handler uses `output = ()`, i.e. `Result<(S, ()), String>`
//! (element 1 is the empty tuple).
//!
//! Note: the `#[export]` state-mode rebinds the state param as immutable, so
//! mutate a local (`let mut state = state;`) rather than a `mut` parameter.

#![no_std]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use packr_guest::{export, GraphValue};

packr_guest::setup_guest!();

// Typed actor state. A hand-written `#[derive(GraphValue)]` in a guest needs the
// ABI-crate path (the `pact!`/`pack_types!` codegen adds this for you).
#[derive(Debug, Clone, PartialEq, GraphValue)]
#[graph(crate = "packr_guest::composite_abi")]
pub struct SysState {
    pub seq: u64,
}

#[derive(Debug, Clone, PartialEq, GraphValue)]
#[graph(crate = "packr_guest::composite_abi")]
pub struct ChannelAccept {
    pub accepted: bool,
}

// Interface metadata. The message-server-client signatures are theater's; this
// mirrors them with the state slot as `value` (opaque to the interface,
// concrete `SysState` in the guest). The results ok-type is the 1-tuple
// response, matching the return shape below.
packr_guest::pack_types! {
    exports {
        theater:simple/message-server-client.handle-request:
            func(state: value, request-id: string, message: list<u8>)
            -> result<tuple<option<list<u8>>>, string>,
        theater:simple/message-server-client.handle-channel-open:
            func(state: value, channel-id: string, initial: list<u8>)
            -> result<tuple<value>, string>,
        theater:simple/message-server-client.handle-send:
            func(state: value, message: list<u8>) -> result<tuple<>, string>,
        theater:simple/message-server-client.handle-channel-close:
            func(state: value, channel-id: string) -> result<tuple<>, string>,
    }
}

// ---- MULTI-return handlers: state + a 1-tuple response ---------------------

// handle-request: reply with an optional body. Return element 1 is `(response,)`.
#[export(
    name = "theater:simple/message-server-client.handle-request",
    state = "SysState"
)]
fn handle_request(
    state: SysState,
    _request_id: String,
    message: Vec<u8>,
) -> Result<(SysState, (Option<Vec<u8>>,)), String> {
    let mut state = state;
    state.seq += 1;
    // echo the message back as the response
    Ok((state, (Some(message),)))
}

// handle-channel-open: accept/reject a channel. Return element 1 is `(accept,)`.
#[export(
    name = "theater:simple/message-server-client.handle-channel-open",
    state = "SysState"
)]
fn handle_channel_open(
    state: SysState,
    _channel_id: String,
    _initial: Vec<u8>,
) -> Result<(SysState, (ChannelAccept,)), String> {
    let mut state = state;
    state.seq += 1;
    Ok((state, (ChannelAccept { accepted: true },)))
}

// ---- SINGLE-return handlers: state only (output = ()) ----------------------

#[export(
    name = "theater:simple/message-server-client.handle-send",
    state = "SysState"
)]
fn handle_send(state: SysState, _message: Vec<u8>) -> Result<(SysState, ()), String> {
    let mut state = state;
    state.seq += 1;
    Ok((state, ()))
}

#[export(
    name = "theater:simple/message-server-client.handle-channel-close",
    state = "SysState"
)]
fn handle_channel_close(state: SysState, _channel_id: String) -> Result<(SysState, ()), String> {
    let mut state = state;
    state.seq += 1;
    Ok((state, ()))
}
