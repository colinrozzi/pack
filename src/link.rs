//! Package linking — matching a consumer's *required* interface against a
//! provider's *exported* interface, type-checked by structural hash.
//!
//! This is the primitive the package linker (and mock-testing) is built on:
//! "requires interface X" is satisfied by "provides interface X" **iff** their
//! structural hashes agree. Because packr's graph ABI is permissive (an `s64`
//! marshals into the value graph), two packages can wire and run without their
//! interfaces actually matching — so the hash check is a strictly stronger,
//! type-safe gate than "it happens to marshal." See `docs/package-linking.md`.

use crate::abi::{decode_prefix, encode, Value};
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

/// Link binaries per explicit `links`, fuse them (via [`crate::compose`]), and
/// **regenerate** the composite's `__pack_types` so the result is first-class and
/// re-linkable: the entry package's surface with the internally-satisfied imports
/// removed.
pub fn link(
    binaries: Vec<LinkBinary>,
    links: &[LinkEdge],
    layout: crate::Layout,
) -> anyhow::Result<Vec<u8>> {
    use std::collections::HashSet;

    // The entry is a consumer (appears as `from`) that no link satisfies (never a
    // `to`). Its surface, minus the satisfied imports, is the composite's surface.
    let providers: HashSet<&str> = links.iter().map(|l| l.to_alias.as_str()).collect();
    let entry_alias = links
        .iter()
        .map(|l| l.from_alias.as_str())
        .find(|a| !providers.contains(a))
        .map(String::from);

    // Capture the entry's wasm + satisfied interfaces before `resolve_links` moves
    // `binaries`.
    let entry = entry_alias.as_ref().and_then(|ea| {
        let wasm = binaries.iter().find(|b| &b.alias == ea)?.wasm.clone();
        let satisfied: Vec<String> = links
            .iter()
            .filter(|l| &l.from_alias == ea)
            .map(|l| l.from_interface.clone())
            .collect();
        Some((wasm, satisfied))
    });

    let packages = resolve_links(binaries, links)?;
    let fused = crate::compose(&crate::ComposeSpec { packages, layout })?;

    match entry {
        Some((wasm, satisfied)) => {
            let meta = composite_metadata(&wasm, &satisfied)?;
            // The composite's public lifecycle = the entry's pact exports; every
            // other leaked member export is trimmed to a clean contract. Match the
            // RAW wasm export symbol, which the guest macro names
            // `<interface>.<fn>` (e.g. `theater:simple/actor.init`) — NOT the bare
            // arena fn name. Using the bare name deletes every interface-qualified
            // lifecycle export (a real theater actor's init/handle-send/…); a
            // bare-export fixture like `host-actor`'s `.process` hides it because
            // there the qualified and bare names coincide.
            let lifecycle: Vec<String> = read_surface(&wasm)?
                .arena
                .exports()
                .iter()
                .map(|f| {
                    if f.interface.is_empty() {
                        f.name.clone()
                    } else {
                        format!("{}.{}", f.interface, f.name)
                    }
                })
                .collect();
            embed_pack_types(&fused, &meta, layout.metadata_base, &lifecycle)
        }
        None => Ok(fused),
    }
}

/// The composite's `__pack_types` bytes: the entry package's metadata with the
/// `satisfied` (now internally-linked) import interfaces removed.
fn composite_metadata(entry_wasm: &[u8], satisfied: &[String]) -> anyhow::Result<Vec<u8>> {
    let parsed = ParsedModule::parse("entry", entry_wasm)
        .map_err(|e| anyhow::anyhow!("parse entry: {e}"))?;
    let seg = parsed
        .data
        .iter()
        .find(|s| s.data.len() >= 4 && s.data[0..4] == CGRF_MAGIC)
        .ok_or_else(|| anyhow::anyhow!("entry has no __pack_types metadata"))?;
    let (mut value, _) =
        decode_prefix(&seg.data).map_err(|e| anyhow::anyhow!("decode entry metadata: {e}"))?;

    if let Value::Record { fields, .. } = &mut value {
        for (fname, fval) in fields.iter_mut() {
            match fname.as_str() {
                // `imports` are function-sig records keyed by their `interface`.
                "imports" => retain_records_without(fval, "interface", satisfied),
                // `import-hashes` are interface-hash records keyed by `name`.
                "import-hashes" => retain_records_without(fval, "name", satisfied),
                _ => {}
            }
        }
    }
    encode(&value).map_err(|e| anyhow::anyhow!("encode composite metadata: {e}"))
}

/// Retain only the list items whose record `field` value is NOT in `drop`.
fn retain_records_without(list: &mut Value, field: &str, drop: &[String]) {
    if let Value::List { items, .. } = list {
        items.retain(|item| {
            if let Value::Record { fields, .. } = item {
                if let Some((_, Value::String(v))) = fields.iter().find(|(n, _)| n == field) {
                    return !drop.iter().any(|d| d == v);
                }
            }
            true
        });
    }
}

/// Embed `meta` as the composite's `__pack_types`: place it in a data segment at
/// `meta_base` and synthesize a `__pack_types` export returning `(meta_base, len)`,
/// replacing any merged-in `__pack_types*` exports so the composite has exactly one
/// coherent surface.
fn embed_pack_types(
    wasm: &[u8],
    meta: &[u8],
    meta_base: u32,
    lifecycle: &[String],
) -> anyhow::Result<Vec<u8>> {
    use std::collections::HashSet;
    use walrus::ir::{MemArg, StoreKind, Value as IrValue};
    use walrus::{ConstExpr, DataKind, FunctionBuilder, Module, ValType};

    let mut m = Module::from_buffer(wasm)?;
    let memory = m
        .memories
        .iter()
        .next()
        .map(|mem| mem.id())
        .ok_or_else(|| anyhow::anyhow!("composite has no memory to hold metadata"))?;

    // Neutralize the members' original `__pack_types` blobs so the composite is
    // discoverable as carrying exactly one — ours. A member's CGRF metadata is
    // the PREFIX of its `.rodata` segment, which also holds live string literals;
    // the packages are fixed-base, so those literals sit at absolute addresses the
    // code reads directly with no relocation. Deleting the whole segment would
    // blank the strings (numeric fixtures never noticed — they read no rodata);
    // splicing out just the prefix would shift every following literal's address.
    // So we zero the magic IN PLACE: the metadata scanner (which keys on a
    // segment's leading CGRF) no longer matches it, its now-unexported getter is
    // dead, and every static string keeps its exact address.
    let stale: Vec<_> = m
        .data
        .iter()
        .filter(|d| d.value.len() >= 4 && d.value[0..4] == CGRF_MAGIC)
        .map(|d| d.id())
        .collect();
    for id in stale {
        m.data.get_mut(id).value[0..4].fill(0);
    }

    // The metadata blob, as an active data segment.
    m.data.add(
        DataKind::Active {
            memory,
            offset: ConstExpr::Value(IrValue::I32(meta_base as i32)),
        },
        meta.to_vec(),
    );

    // fn __pack_types(out_ptr_ptr: i32, out_len_ptr: i32) -> i32:
    //   *out_ptr_ptr = meta_base; *out_len_ptr = len; return 0
    let mut b = FunctionBuilder::new(&mut m.types, &[ValType::I32, ValType::I32], &[ValType::I32]);
    let out_ptr_ptr = m.locals.add(ValType::I32);
    let out_len_ptr = m.locals.add(ValType::I32);
    let mem_arg = MemArg {
        align: 2,
        offset: 0,
    };
    b.func_body()
        .local_get(out_ptr_ptr)
        .i32_const(meta_base as i32)
        .store(memory, StoreKind::I32 { atomic: false }, mem_arg)
        .local_get(out_len_ptr)
        .i32_const(meta.len() as i32)
        .store(memory, StoreKind::I32 { atomic: false }, mem_arg)
        .i32_const(0);
    let f = b.finish(vec![out_ptr_ptr, out_len_ptr], &mut m.funcs);

    // Drop the merged-in __pack_types* exports; add the single coherent one.
    let stale: Vec<_> = m
        .exports
        .iter()
        .filter(|e| e.name.starts_with("__pack_types"))
        .map(|e| e.id())
        .collect();
    for id in stale {
        m.exports.delete(id);
    }
    m.exports.add("__pack_types", f);

    // Trim the export surface to exactly the self-contained contract: memory +
    // one __pack_alloc/__pack_free + __pack_types (+ ctors) + the lifecycle. This
    // drops the merge's leaked/ambiguous exports — the redundant __pack_alloc_10,
    // the members' raw alloc/dealloc/double, and the vestigial __heap_base/
    // __data_end globals — so a host sees one unambiguous allocator + heap.
    let mut keep: HashSet<&str> = ["memory", "__pack_alloc", "__pack_free", "__pack_types"]
        .into_iter()
        .collect();
    let lifecycle: HashSet<&str> = lifecycle.iter().map(String::as_str).collect();
    keep.extend(&lifecycle);
    let trim: Vec<_> = m
        .exports
        .iter()
        .filter(|e| !keep.contains(e.name.as_str()) && e.name != "__wasm_call_ctors")
        .map(|e| e.id())
        .collect();
    for id in trim {
        m.exports.delete(id);
    }

    // `dylink.0` means "I am a dynamic-linking side module." A self-contained
    // object is not one, so the section is simply wrong here and we drop it —
    // true for any host (theater, for one, reads its presence as a PIC signal).
    let dylink: Vec<_> = m
        .customs
        .iter()
        .filter(|(_, c)| c.name() == "dylink.0")
        .map(|(id, _)| id)
        .collect();
    for id in dylink {
        m.customs.delete(id);
    }

    Ok(m.emit_wasm())
}
