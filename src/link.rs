//! Package linking — matching a consumer's *required* interface against a
//! provider's *exported* interface, type-checked by structural hash.
//!
//! This is the primitive the package linker (and mock-testing) is built on:
//! "requires interface X" is satisfied by "provides interface X" **iff** their
//! structural hashes agree. Because packr's graph ABI is permissive (an `s64`
//! marshals into the value graph), two packages can wire and run without their
//! interfaces actually matching — so the hash check is a strictly stronger,
//! type-safe gate than "it happens to marshal." See `docs/package-linking.md`.

use crate::metadata::{decode_metadata_with_hashes, MetadataWithHashes};
use crate::ParsedModule;

/// CGRF metadata magic: "CGRF".
const CGRF_MAGIC: [u8; 4] = [0x43, 0x47, 0x52, 0x46];

/// Read a package's typed interface surface (imports + exports with per-interface
/// structural hashes) statically from its `__pack_types` metadata — no
/// instantiation required.
pub fn read_surface(wasm: &[u8]) -> anyhow::Result<MetadataWithHashes> {
    let parsed =
        ParsedModule::parse("pkg", wasm).map_err(|e| anyhow::anyhow!("parse wasm: {e}"))?;
    let seg = parsed
        .data
        .iter()
        .find(|s| s.data.len() >= 4 && s.data[0..4] == CGRF_MAGIC)
        .ok_or_else(|| anyhow::anyhow!("no __pack_types metadata (CGRF segment) found"))?;
    decode_metadata_with_hashes(&seg.data).map_err(|e| anyhow::anyhow!("decode metadata: {e}"))
}

/// Why a proposed link is not type-safe.
#[derive(Debug, thiserror::Error)]
pub enum LinkError {
    #[error("consumer imports no interface named `{0}`")]
    NoSuchImport(String),
    #[error("provider exports no interface named `{0}`")]
    ProviderMissingInterface(String),
    #[error("interface `{interface}` hash mismatch: consumer requires {required}, provider offers {provided}")]
    HashMismatch {
        interface: String,
        required: String,
        provided: String,
    },
}

/// Check that `provider` exports `interface` in a way that satisfies what
/// `consumer` imports — by structural hash equality.
///
/// This is the type-safe gate for linking: a mock is a sound substitute for a
/// real provider **iff** this passes.
pub fn check_interface_link(
    consumer: &MetadataWithHashes,
    interface: &str,
    provider: &MetadataWithHashes,
) -> Result<(), LinkError> {
    let required = consumer
        .import_hashes
        .iter()
        .find(|h| h.name == interface)
        .ok_or_else(|| LinkError::NoSuchImport(interface.to_string()))?;
    let provided = provider
        .export_hashes
        .iter()
        .find(|h| h.name == interface)
        .ok_or_else(|| LinkError::ProviderMissingInterface(interface.to_string()))?;
    if required.hash != provided.hash {
        return Err(LinkError::HashMismatch {
            interface: interface.to_string(),
            required: format!("{}", required.hash),
            provided: format!("{}", provided.hash),
        });
    }
    Ok(())
}
