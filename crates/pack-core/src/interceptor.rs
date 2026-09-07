//! Call interception for record/replay — backend-agnostic.
//!
//! An interceptor observes (or short-circuits) both directions: host-function
//! calls the guest makes (imports) and export calls made into the guest. A
//! recording interceptor returns `None` from the `before_*` hooks and records in
//! the `after_*` hooks; a replay interceptor returns `Some(recorded)` from
//! `before_*` to skip the real call.
//!
//! All hooks are `async` (async-first): a recorder can apply real back-pressure
//! (e.g. `.await` a bounded channel send to a chain subscriber). Unlike the old
//! `packr` runtime there is no sync bridge, so no `block_in_place` /
//! current-tokio-runtime requirement — the whole path is already async.

use async_trait::async_trait;
use packr_abi::Value;

/// Intercepts calls at the pack runtime level. Implementations record calls
/// (audit/replay) or short-circuit them with previously recorded values.
#[async_trait]
pub trait CallInterceptor: Send + Sync {
    /// Before a host function (import) executes. `Some` short-circuits with a
    /// recorded value (replay); `None` proceeds normally.
    async fn before_import(&self, interface: &str, function: &str, input: &Value) -> Option<Value>;

    /// After a host function (import) returns.
    async fn after_import(&self, interface: &str, function: &str, input: &Value, output: &Value);

    /// Before an export executes. `Some` short-circuits with a recorded value.
    async fn before_export(&self, function: &str, input: &Value) -> Option<Value>;

    /// After an export returns.
    async fn after_export(&self, function: &str, input: &Value, output: &Value);
}
