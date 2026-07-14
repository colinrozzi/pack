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

/// Check that `provider`'s export interface `provider_iface` satisfies what
/// `consumer` imports as `consumer_iface` — by structural hash equality. The two
/// names may differ (a provider can export an interface under a different name
/// than a consumer imports it); the *structure* must match.
pub fn check_link(
    consumer: &MetadataWithHashes,
    consumer_iface: &str,
    provider: &MetadataWithHashes,
    provider_iface: &str,
) -> Result<(), LinkError> {
    let required = consumer
        .import_hashes
        .iter()
        .find(|h| h.name == consumer_iface)
        .ok_or_else(|| LinkError::NoSuchImport(consumer_iface.to_string()))?;
    let provided = provider
        .export_hashes
        .iter()
        .find(|h| h.name == provider_iface)
        .ok_or_else(|| LinkError::ProviderMissingInterface(provider_iface.to_string()))?;
    if required.hash != provided.hash {
        return Err(LinkError::HashMismatch {
            interface: consumer_iface.to_string(),
            required: format!("{}", required.hash),
            provided: format!("{}", provided.hash),
        });
    }
    Ok(())
}

/// Check that `provider` exports `interface` compatibly with how `consumer`
/// imports it (same interface name on both sides). The type-safe gate: a mock is
/// a sound substitute for a real provider **iff** this passes.
pub fn check_interface_link(
    consumer: &MetadataWithHashes,
    interface: &str,
    provider: &MetadataWithHashes,
) -> Result<(), LinkError> {
    check_link(consumer, interface, provider, interface)
}

/// One binary in a link spec, with a local `alias`.
pub struct LinkBinary {
    pub alias: String,
    pub wasm: Vec<u8>,
    /// The allocator provides `pack:alloc` and is wired to every package.
    pub allocator: bool,
}

/// An explicit link: `<from_alias>` imports interface `<from_interface>`, and it
/// is satisfied by `<to_alias>`'s exported interface `<to_interface>`.
pub struct LinkEdge {
    pub from_alias: String,
    pub from_interface: String,
    pub to_alias: String,
    pub to_interface: String,
}

/// Validate the explicit links (hash-check each) and resolve the binaries into a
/// `wasm-merge`-ordered [`PackageSpec`] list ready for [`crate::compose`]:
/// each provider is named by the import-module-name its consumer uses, the
/// allocator is named `pack:alloc`, and providers/allocator precede consumers.
///
/// Errors (with a [`LinkError`]) if any link is not type-safe.
pub fn resolve_links(
    binaries: Vec<LinkBinary>,
    links: &[LinkEdge],
) -> anyhow::Result<Vec<crate::PackageSpec>> {
    use std::collections::HashMap;

    // Read every pact surface once. The allocator is raw substrate (no
    // `__pack_types`); it is never a consumer or provider in a link.
    let mut surface = HashMap::new();
    for b in &binaries {
        if b.allocator {
            continue;
        }
        surface.insert(
            b.alias.clone(),
            read_surface(&b.wasm)
                .map_err(|e| anyhow::anyhow!("reading `{}` surface: {e}", b.alias))?,
        );
    }

    // Validate each link and record how each provider must be named for wasm-merge
    // (the consumer's import module-name).
    let mut merge_name: HashMap<String, String> = HashMap::new();
    for l in links {
        let consumer = surface
            .get(&l.from_alias)
            .ok_or_else(|| anyhow::anyhow!("link references unknown binary `{}`", l.from_alias))?;
        let provider = surface
            .get(&l.to_alias)
            .ok_or_else(|| anyhow::anyhow!("link references unknown binary `{}`", l.to_alias))?;
        check_link(consumer, &l.from_interface, provider, &l.to_interface).map_err(|e| {
            anyhow::anyhow!(
                "link `{}.{}` <- `{}.{}` is not type-safe: {e}",
                l.from_alias,
                l.from_interface,
                l.to_alias,
                l.to_interface
            )
        })?;
        // The provider must be named after the interface the consumer imports so
        // wasm-merge wires the consumer's imports to it.
        if let Some(prev) = merge_name.insert(l.to_alias.clone(), l.from_interface.clone()) {
            anyhow::ensure!(
                prev == l.from_interface,
                "binary `{}` provides two differently-named interfaces (`{prev}`, `{}`) — \
                 wasm-merge names a whole module once; split the provider",
                l.to_alias,
                l.from_interface
            );
        }
    }

    // Order: allocator, then providers, then everyone else (consumers). Within a
    // group, preserve input order.
    let mut ordered: Vec<crate::PackageSpec> = Vec::new();
    let name_of = |b: &LinkBinary| -> String {
        if b.allocator {
            "pack:alloc".to_string()
        } else if let Some(iface) = merge_name.get(&b.alias) {
            iface.clone()
        } else {
            b.alias.clone()
        }
    };
    for pass in 0..3 {
        for b in &binaries {
            let is_alloc = b.allocator;
            let is_provider = merge_name.contains_key(&b.alias);
            let bucket = if is_alloc {
                0
            } else if is_provider {
                1
            } else {
                2
            };
            if bucket == pass {
                ordered.push(crate::PackageSpec::new(name_of(b), b.wasm.clone()));
            }
        }
    }
    Ok(ordered)
}
