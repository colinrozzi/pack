//! Component composition — Milestone 2.
//!
//! Compose N isolated wasm "components" into ONE multi-memory wasm binary,
//! wiring each consumer's pact import to the providing component's pact export
//! through a statically-generated **bridging shim**.
//!
//! # The model (see `docs/component-composition.md`)
//!
//! The composite is a single module with **one memory per component**: the entry
//! component is memory 0, each other component gets memories 1, 2, …. Each
//! component keeps its own `__stack_pointer`, data and heap in its own memory, so
//! their addresses can never collide — the whole reconciliation bug class the
//! retired fusion suffered is structurally impossible here.
//!
//! # How it works
//!
//! `wasm-merge --enable-multimemory` does the module merge: with the entry module
//! placed FIRST it becomes memory 0 (its exports stay canonical: `memory`,
//! `__pack_alloc`, `__pack_free`, its pact functions — that is what theater
//! loads). Each subsequent component is placed under a unique merge name and gets
//! memories 1, 2, …; its exports are pre-renamed to a component-scoped prefix
//! (`__c_<name>_<export>`) so they survive the merge, are findable by name, and
//! never collide with any other component's.
//!
//! This module is the walrus post-pass that:
//!
//! 1. Pre-renames every non-entry component's exports to `__c_<name>_<export>`.
//! 2. Shells out to `wasm-merge` to produce the multi-memory module (entry first).
//! 3. For each link, builds a shim (alloc in the provider's memory → copy in →
//!    call the provider's renamed export → copy result out into the consumer's
//!    memory → free) and rewrites every call to the consumer's imported function
//!    to call the shim instead, then deletes the now-dead import.
//! 4. Removes the `__c_*` scaffolding exports so the composite presents only the
//!    entry component's surface (`memory`, `__pack_alloc`/`__pack_free`, pacts).
//!
//! Each shim is pure byte-shuffling over the actor ABI
//! (`fn(in_ptr, in_len, out_ptr_ptr, out_len_ptr) -> status`), copying between
//! the CONSUMER's memory and the PROVIDER's memory (any two of the N memories),
//! so it never needs to know the pact's high-level types.
//!
//! # Hash-checking (deferred)
//!
//! The design says a link is valid iff the consumer's import interface and the
//! provider's export interface have matching Merkle interface hashes. M2 wires
//! links **by name** (as M1 did). Extracting each component's pact interfaces from
//! its `__pack_types` (`CGRF`) data segment and comparing per-interface hashes is
//! a self-contained follow-on; see `docs/component-composition.md`. Wiring by
//! name is sound for the fixtures and the acceptance gate; the hash check is an
//! additional safety net, not a correctness prerequisite for the transform.

use anyhow::{anyhow, Context, Result};
use std::collections::HashMap;
use walrus::ir::{
    BinaryOp, Binop, Call, Const, IfElse, Instr, InstrSeqType, LoadKind, LocalGet, LocalSet,
    LocalTee, MemArg, MemoryCopy, Select, Store, StoreKind, Value as IrValue,
};
use walrus::{
    ExportItem, FunctionBuilder, FunctionId, ImportKind, LocalId, MemoryId, Module, ValType,
};

/// The canonical exports the entry component keeps (and every component has).
const EXPORT_MEMORY: &str = "memory";
const EXPORT_ALLOC: &str = "__pack_alloc";
const EXPORT_FREE: &str = "__pack_free";

/// A component to be composed: a plain actor wasm plus a name and an entry flag.
///
/// Exactly one component in a `compose` call must have `entry = true`; it becomes
/// memory 0 and keeps canonical export names — that is the surface theater loads.
#[derive(Debug, Clone)]
pub struct Component {
    /// A unique name within the composition (used to scope this component's
    /// renamed exports and to reference it from links).
    pub name: String,
    /// The component's wasm bytes.
    pub wasm: Vec<u8>,
    /// Whether this is the entry component (memory 0, canonical exports).
    pub entry: bool,
}

/// A single link in the N-component graph: the `consumer` component's import
/// `(import_module, import_name)` is satisfied by the `provider` component's
/// export `export_name`.
#[derive(Debug, Clone)]
pub struct GraphLink {
    /// The name of the component that declares the import.
    pub consumer: String,
    /// The import module the consumer declares (e.g. `"math"`).
    pub import_module: String,
    /// The import field the consumer declares (e.g. `"double"`).
    pub import_name: String,
    /// The name of the component that provides the export.
    pub provider: String,
    /// The provider's export that satisfies it (e.g. `"double"`).
    pub export_name: String,
}

/// A residual host import declared by a NON-ENTRY component that must be
/// bridged through the entry's memory (see [`compose`] and [`emit_host_shim`]).
///
/// The provider calls `(import_module, import_name)` with pointers into its OWN
/// memory; the host reads/writes the composite's canonical (entry) memory. The
/// bridge shim marshals between the two around the real host call.
#[derive(Debug, Clone)]
struct HostBridge {
    /// The non-entry component that declares the residual import.
    component: String,
    /// The residual import's module (e.g. `"host"`).
    import_module: String,
    /// The residual import's field (e.g. `"tick"`).
    import_name: String,
}

/// A single link between exactly two components (the M1 API).
///
/// Retained for backwards compatibility; `compose_pair` builds a two-component
/// `compose` call from it.
#[derive(Debug, Clone)]
pub struct Link {
    /// The import module the consumer declares (e.g. `"math"`).
    pub import_module: String,
    /// The import field the consumer declares (e.g. `"double"`).
    pub import_name: String,
    /// The provider's export that satisfies it (e.g. `"double"`).
    pub export_name: String,
}

/// Compose a consumer and a provider into one multi-memory composite wasm (M1).
///
/// A thin wrapper over [`compose`] with two components (the consumer as entry,
/// the provider) and one link.
pub fn compose_pair(consumer_wasm: &[u8], provider_wasm: &[u8], link: &Link) -> Result<Vec<u8>> {
    let components = vec![
        Component {
            name: "app".to_string(),
            wasm: consumer_wasm.to_vec(),
            entry: true,
        },
        Component {
            name: "prov".to_string(),
            wasm: provider_wasm.to_vec(),
            entry: false,
        },
    ];
    let links = vec![GraphLink {
        consumer: "app".to_string(),
        import_module: link.import_module.clone(),
        import_name: link.import_name.clone(),
        provider: "prov".to_string(),
        export_name: link.export_name.clone(),
    }];
    compose(components, &links)
}

/// The component-scoped export prefix for a non-entry component's export.
fn scoped_name(component: &str, export: &str) -> String {
    format!("__c_{component}_{export}")
}

/// Compose N components + a link graph into one multi-memory composite wasm.
///
/// Exactly one component must be the entry; it becomes memory 0 and keeps its
/// canonical exports (`memory`, `__pack_alloc`, `__pack_free`, its pact
/// functions) — the surface theater loads. Every other component is merged with a
/// unique memory (1, 2, …) and its exports pre-renamed `__c_<name>_<export>`.
///
/// For each link, a bridging shim is generated that copies between the consumer
/// component's memory and the provider component's memory (whichever two of the N
/// they are), allocating with the provider's `__pack_alloc`, calling the
/// provider's export, then copying the result back into the consumer's memory.
pub fn compose(components: Vec<Component>, links: &[GraphLink]) -> Result<Vec<u8>> {
    // Exactly one entry.
    let entry_count = components.iter().filter(|c| c.entry).count();
    if entry_count != 1 {
        return Err(anyhow!(
            "exactly one component must have `entry = true`, found {entry_count}"
        ));
    }

    // Names must be unique.
    {
        let mut seen = std::collections::HashSet::new();
        for c in &components {
            if !seen.insert(c.name.as_str()) {
                return Err(anyhow!("duplicate component name `{}`", c.name));
            }
        }
    }

    // Validate links reference known components (consumer + provider).
    let names: std::collections::HashSet<&str> =
        components.iter().map(|c| c.name.as_str()).collect();
    for l in links {
        if !names.contains(l.consumer.as_str()) {
            return Err(anyhow!("link references unknown consumer `{}`", l.consumer));
        }
        if !names.contains(l.provider.as_str()) {
            return Err(anyhow!("link references unknown provider `{}`", l.provider));
        }
    }

    // Step 1: order entry first, pre-rename non-entry exports to `__c_<name>_*`.
    // The entry component keeps canonical export names untouched.
    let mut ordered: Vec<&Component> = Vec::with_capacity(components.len());
    ordered.push(components.iter().find(|c| c.entry).unwrap());
    for c in components.iter().filter(|c| !c.entry) {
        ordered.push(c);
    }

    let mut merge_inputs: Vec<(String, Vec<u8>)> = Vec::with_capacity(ordered.len());
    for c in &ordered {
        let bytes = if c.entry {
            c.wasm.clone()
        } else {
            rename_component_exports(&c.wasm, &c.name)
                .with_context(|| format!("renaming exports for component `{}`", c.name))?
        };
        merge_inputs.push((c.name.clone(), bytes));
    }

    // A residual host import declared by a NON-ENTRY component points into that
    // component's own memory (memory 1, 2, …), but the host resolves the guest
    // memory + allocator from the composite's canonical `memory`/`__pack_alloc`
    // exports — i.e. the ENTRY's memory 0. Left alone, the host would read the
    // call's args from the wrong memory ("Invalid magic" on decode). So for each
    // such import we emit a host-bridge shim (memory1↔memory0 marshalling around
    // the real host call) and rewire the component's calls to it — the async M3
    // analogue of the link shim. Collect them here (pre-merge, where each
    // component's imports are unambiguous), skipping any import a link satisfies.
    let mut host_bridges: Vec<HostBridge> = Vec::new();
    for c in ordered.iter().filter(|c| !c.entry) {
        let linked: std::collections::HashSet<(&str, &str)> = links
            .iter()
            .filter(|l| l.consumer == c.name)
            .map(|l| (l.import_module.as_str(), l.import_name.as_str()))
            .collect();
        for (module_name, field) in component_func_imports(&c.wasm)
            .with_context(|| format!("reading imports for component `{}`", c.name))?
        {
            if !linked.contains(&(module_name.as_str(), field.as_str())) {
                host_bridges.push(HostBridge {
                    component: c.name.clone(),
                    import_module: module_name,
                    import_name: field,
                });
            }
        }
    }

    // Step 2: merge with wasm-merge (multi-memory), entry first (→ memory 0).
    let merged = merge_multimemory(&merge_inputs)
        .context("merging components into a multi-memory module")?;

    // Step 3 + 4: locate every component's memory/funcs by name, emit one shim
    // per link, rewire the consumer's import, bridge non-entry residual host
    // imports through memory 0, and strip scaffolding exports.
    let entry_name = &ordered[0].name;
    let component_names: Vec<String> = ordered.iter().map(|c| c.name.clone()).collect();
    let composite = shim_and_rewire(&merged, entry_name, &component_names, links, &host_bridges)
        .context("emitting shims and rewiring imports")?;

    Ok(composite)
}

/// Rename ALL of a non-entry component's exports to `__c_<name>_<orig>` so none
/// collides on merge and each is findable by a unique, component-scoped name.
///
/// The load-bearing three (`__pack_alloc`, `__pack_free`, `memory`) plus every
/// linked export end up as `__c_<name>_<orig>`; the shim looks them up by that
/// name. Every other export (`__pack_types`, `__heap_base`, …) is renamed too so
/// it survives the merge harmlessly and is dropped later.
fn rename_component_exports(component_wasm: &[u8], component: &str) -> Result<Vec<u8>> {
    let mut module = Module::from_buffer(component_wasm)?;

    // Sanity: the three required exports must exist.
    for required in [EXPORT_ALLOC, EXPORT_FREE, EXPORT_MEMORY] {
        if !module.exports.iter().any(|e| e.name == required) {
            return Err(anyhow!(
                "component `{component}` is missing required export `{required}`"
            ));
        }
    }

    let ids: Vec<_> = module.exports.iter().map(|e| e.id()).collect();
    for id in ids {
        let name = module.exports.get(id).name.clone();
        module.exports.get_mut(id).name = scoped_name(component, &name);
    }

    Ok(module.emit_wasm())
}

/// Shell out to `wasm-merge --enable-multimemory`, writing inputs to temp files.
/// `inputs` are `(merge_name, wasm)` in order; the FIRST becomes memory 0.
fn merge_multimemory(inputs: &[(String, Vec<u8>)]) -> Result<Vec<u8>> {
    let dir = std::env::temp_dir();
    let pid = std::process::id();
    // Per-call nonce: two `compose()` calls in the SAME process (e.g. parallel
    // test threads) share the PID, so PID alone is not unique — they would clobber
    // each other's temp inputs/output and wasm-merge would read a half-written
    // file. The monotonic counter makes every call's paths distinct.
    static COMPOSE_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = COMPOSE_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let out_path = dir.join(format!("packr-compose-out-{pid}-{seq}.wasm"));

    // Write each input to a temp file, remembering the paths for cleanup.
    let mut paths: Vec<std::path::PathBuf> = Vec::with_capacity(inputs.len());
    for (i, (_name, wasm)) in inputs.iter().enumerate() {
        let p = dir.join(format!("packr-compose-in-{pid}-{seq}-{i}.wasm"));
        std::fs::write(&p, wasm)?;
        paths.push(p);
    }

    let status = run_wasm_merge(inputs, &paths, &out_path);

    let cleanup = || {
        for p in &paths {
            let _ = std::fs::remove_file(p);
        }
        let _ = std::fs::remove_file(&out_path);
    };

    match status {
        Ok(true) => {}
        Ok(false) => {
            cleanup();
            return Err(anyhow!("wasm-merge failed"));
        }
        Err(e) => {
            cleanup();
            return Err(e);
        }
    }

    let merged = std::fs::read(&out_path)?;
    cleanup();
    Ok(merged)
}

/// Invoke `wasm-merge <in0> <name0> <in1> <name1> … -o <out> --enable-multimemory`.
///
/// Tries a bare `wasm-merge` first; if that isn't on PATH, retries through
/// `nix shell nixpkgs#binaryen -c wasm-merge ...` so the transform works in the
/// dev environment without a global binaryen install.
fn run_wasm_merge(
    inputs: &[(String, Vec<u8>)],
    paths: &[std::path::PathBuf],
    out: &std::path::Path,
) -> Result<bool> {
    use std::process::Command;

    // Build the positional `<file> <merge-name>` pairs plus the output flags.
    let merge_args: Vec<String> = {
        let mut v = Vec::new();
        for (i, (name, _)) in inputs.iter().enumerate() {
            v.push(paths[i].display().to_string());
            v.push(name.clone());
        }
        v.push("-o".to_string());
        v.push(out.display().to_string());
        v.push("--enable-multimemory".to_string());
        v
    };

    // First: bare `wasm-merge`.
    let direct = Command::new("wasm-merge").args(&merge_args).output();
    match direct {
        Ok(o) if o.status.success() => return Ok(true),
        Ok(o) => {
            return Err(anyhow!(
                "wasm-merge failed: {}",
                String::from_utf8_lossy(&o.stderr)
            ));
        }
        Err(_) => { /* not on PATH — fall through to nix */ }
    }

    // Fallback: through nix.
    let nix_args = {
        let mut v = vec![
            "shell".to_string(),
            "nixpkgs#binaryen".to_string(),
            "-c".to_string(),
            "wasm-merge".to_string(),
        ];
        v.extend(merge_args);
        v
    };
    let via_nix = Command::new("nix").args(&nix_args).output().context(
        "could not run wasm-merge directly or via `nix shell nixpkgs#binaryen`; \
         install binaryen or make nix available",
    )?;
    if via_nix.status.success() {
        Ok(true)
    } else {
        Err(anyhow!(
            "wasm-merge (via nix) failed: {}",
            String::from_utf8_lossy(&via_nix.stderr)
        ))
    }
}

/// A component's resolved handles in the merged module.
struct ResolvedComponent {
    memory: MemoryId,
    alloc: FunctionId,
    free: FunctionId,
    /// Exported functions by their ORIGINAL (unscoped) name.
    exports: HashMap<String, FunctionId>,
}

fn shim_and_rewire(
    merged_wasm: &[u8],
    entry_name: &str,
    component_names: &[String],
    links: &[GraphLink],
    host_bridges: &[HostBridge],
) -> Result<Vec<u8>> {
    let mut module = Module::from_buffer(merged_wasm)?;

    // Resolve each component's memory + alloc/free + exported funcs by the unique
    // names we assigned (entry = canonical, others = `__c_<name>_*`).
    let mut resolved: HashMap<String, ResolvedComponent> = HashMap::new();
    for name in component_names {
        let is_entry = name == entry_name;
        let r = resolve_component(&module, name, is_entry)
            .with_context(|| format!("locating component `{name}` in the merged module"))?;
        resolved.insert(name.clone(), r);
    }

    // Bridge each non-entry component's residual host import through memory 0.
    // The host reads/writes the composite's canonical (entry) memory + allocator;
    // the component's call passes pointers into its OWN memory. The bridge copies
    // input component-mem → entry-mem, calls the REAL host import (which the host
    // fills, resolving the entry memory), then copies the result entry-mem →
    // component-mem. The import is KEPT (the host provides it); only the
    // component's own call sites are rewired to the bridge — excluding the
    // bridge itself, so it still calls the real import.
    let entry = resolved
        .get(entry_name)
        .ok_or_else(|| anyhow!("entry component `{entry_name}` not resolved"))?;
    let entry_mem = entry.memory;
    let entry_alloc = entry.alloc;
    let entry_free = entry.free;
    for bridge in host_bridges {
        let component = resolved
            .get(&bridge.component)
            .ok_or_else(|| anyhow!("host-bridge component `{}` not resolved", bridge.component))?;
        let comp_mem = component.memory;
        let comp_alloc = component.alloc;

        let import_func = find_func_import(&module, &bridge.import_module, &bridge.import_name)
            .ok_or_else(|| {
                anyhow!(
                    "no residual host import `{}.{}` found for component `{}`",
                    bridge.import_module,
                    bridge.import_name,
                    bridge.component
                )
            })?;

        let params = HostShimParams {
            comp_mem,
            comp_alloc,
            entry_mem,
            entry_alloc,
            entry_free,
            host_import: import_func,
        };
        let shim_id = emit_host_shim(&mut module, &params);

        // Rewire the component's calls to the import → the bridge, but leave the
        // bridge's own call to the import intact, and KEEP the import.
        rewire_import_except(&mut module, import_func, shim_id, shim_id);
    }

    // For each link, emit a shim bridging the consumer's import to the provider's
    // export, then rewire the consumer's import calls to the shim.
    for link in links {
        let consumer = resolved
            .get(&link.consumer)
            .ok_or_else(|| anyhow!("link consumer `{}` not resolved", link.consumer))?;
        let provider = resolved
            .get(&link.provider)
            .ok_or_else(|| anyhow!("link provider `{}` not resolved", link.provider))?;

        let prov_export = *provider.exports.get(&link.export_name).ok_or_else(|| {
            anyhow!(
                "provider `{}` has no export `{}`",
                link.provider,
                link.export_name
            )
        })?;

        // Find the consumer's still-unwired import `(import_module, import_name)`.
        let import_func = find_func_import(&module, &link.import_module, &link.import_name)
            .ok_or_else(|| {
                anyhow!(
                    "no import `{}.{}` found for consumer `{}` (already wired, or the \
                     consumer does not declare it)",
                    link.import_module,
                    link.import_name,
                    link.consumer
                )
            })?;

        let params = ShimParams {
            consumer_mem: consumer.memory,
            consumer_alloc: consumer.alloc,
            provider_mem: provider.memory,
            provider_alloc: provider.alloc,
            provider_free: provider.free,
            provider_export: prov_export,
        };
        let shim_id = emit_shim(&mut module, &params);

        rewire_import(&mut module, import_func, shim_id);
    }

    let module = strip_scoped_exports(module, component_names, entry_name);
    let mut module = module;
    Ok(module.emit_wasm())
}

/// Resolve a component's memory, alloc, free and all exported functions by name.
///
/// For the entry component the names are canonical; for others they are
/// `__c_<name>_<orig>`. Exported functions are keyed by their ORIGINAL name so
/// links can reference `export_name` directly.
fn resolve_component(module: &Module, name: &str, is_entry: bool) -> Result<ResolvedComponent> {
    let key = |orig: &str| -> String {
        if is_entry {
            orig.to_string()
        } else {
            scoped_name(name, orig)
        }
    };

    let memory = export_memory(module, &key(EXPORT_MEMORY))
        .ok_or_else(|| anyhow!("component `{name}` memory export not found in merged module"))?;
    let alloc = export_func(module, &key(EXPORT_ALLOC))
        .ok_or_else(|| anyhow!("component `{name}` `__pack_alloc` not found in merged module"))?;
    let free = export_func(module, &key(EXPORT_FREE))
        .ok_or_else(|| anyhow!("component `{name}` `__pack_free` not found in merged module"))?;

    // Every exported function, keyed by ORIGINAL name (strip the scope prefix for
    // non-entry components).
    let mut exports = HashMap::new();
    let prefix = scoped_name(name, "");
    for e in module.exports.iter() {
        if let ExportItem::Function(f) = e.item {
            let orig = if is_entry {
                Some(e.name.clone())
            } else {
                e.name.strip_prefix(&prefix).map(|s| s.to_string())
            };
            if let Some(orig) = orig {
                exports.insert(orig, f);
            }
        }
    }

    Ok(ResolvedComponent {
        memory,
        alloc,
        free,
        exports,
    })
}

/// List a component's function imports as `(module, field)` pairs, in module
/// order. Used pre-merge to identify residual host imports (those a link does
/// not satisfy) that need memory-bridging.
fn component_func_imports(component_wasm: &[u8]) -> Result<Vec<(String, String)>> {
    let module = Module::from_buffer(component_wasm)?;
    Ok(module
        .imports
        .iter()
        .filter_map(|imp| match imp.kind {
            ImportKind::Function(_) => Some((imp.module.clone(), imp.name.clone())),
            _ => None,
        })
        .collect())
}

/// Find an imported function by `(module, name)`.
fn find_func_import(module: &Module, imp_module: &str, imp_name: &str) -> Option<FunctionId> {
    module.imports.iter().find_map(|imp| match imp.kind {
        ImportKind::Function(f) if imp.module == imp_module && imp.name == imp_name => Some(f),
        _ => None,
    })
}

fn export_memory(module: &Module, name: &str) -> Option<MemoryId> {
    module.exports.iter().find_map(|e| match e.item {
        ExportItem::Memory(m) if e.name == name => Some(m),
        _ => None,
    })
}

fn export_func(module: &Module, name: &str) -> Option<FunctionId> {
    module.exports.iter().find_map(|e| match e.item {
        ExportItem::Function(f) if e.name == name => Some(f),
        _ => None,
    })
}

/// Rewrite every `Call { func: import_func }` to `Call { func: shim_id }`, then
/// delete the now-dead import (both the import entry and the func stub).
fn rewire_import(module: &mut Module, import_func: FunctionId, shim_id: FunctionId) {
    for (_id, func) in module.funcs.iter_local_mut() {
        let entry = func.entry_block();
        let mut rewriter = CallRewriter {
            from: import_func,
            to: shim_id,
        };
        walrus::ir::dfs_pre_order_mut(&mut rewriter, func, entry);
    }

    let import_id = module
        .imports
        .iter()
        .find(|imp| matches!(imp.kind, ImportKind::Function(f) if f == import_func))
        .map(|imp| imp.id());
    if let Some(id) = import_id {
        module.imports.delete(id);
    }
    module.funcs.delete(import_func);
}

/// Rewrite every `Call { func: import_func }` to `Call { func: shim_id }`,
/// EXCEPT inside `skip` (the bridge itself, which must still call the import),
/// and KEEP the import (the host provides it). Used for host-import bridging,
/// where the residual import survives as the composite's residual surface.
fn rewire_import_except(
    module: &mut Module,
    import_func: FunctionId,
    shim_id: FunctionId,
    skip: FunctionId,
) {
    for (id, func) in module.funcs.iter_local_mut() {
        if id == skip {
            continue;
        }
        let entry = func.entry_block();
        let mut rewriter = CallRewriter {
            from: import_func,
            to: shim_id,
        };
        walrus::ir::dfs_pre_order_mut(&mut rewriter, func, entry);
    }
}

/// A VisitorMut that rewrites `Call{from}` to `Call{to}`.
struct CallRewriter {
    from: FunctionId,
    to: FunctionId,
}

impl walrus::ir::VisitorMut for CallRewriter {
    fn visit_instr_mut(&mut self, instr: &mut Instr, _loc: &mut walrus::ir::InstrLocId) {
        if let Instr::Call(Call { func }) = instr {
            if *func == self.from {
                *func = self.to;
            }
        }
    }
}

/// The handles a shim needs: the consumer's memory + alloc, and the provider's
/// memory + alloc/free + the exported target function.
struct ShimParams {
    consumer_mem: MemoryId,
    consumer_alloc: FunctionId,
    provider_mem: MemoryId,
    provider_alloc: FunctionId,
    provider_free: FunctionId,
    provider_export: FunctionId,
}

/// Emit a bridging shim function and return its id.
///
/// Signature `(in_ptr, in_len, out_ptr_ptr, out_len_ptr) -> status`, all i32,
/// pointers into the CONSUMER's memory. The shim marshals across to the PROVIDER's
/// memory, calls its export, and marshals the result back into the consumer's
/// memory. The two memories may be any two of the composite's N memories.
fn emit_shim(module: &mut Module, p: &ShimParams) -> FunctionId {
    let i32 = ValType::I32;
    let mut builder = FunctionBuilder::new(&mut module.types, &[i32, i32, i32, i32], &[i32]);
    builder.name("__bridge_shim".to_string());

    // Params (pointers into the consumer's memory).
    let in_ptr = module.locals.add(i32);
    let in_len = module.locals.add(i32);
    let out_ptr_ptr = module.locals.add(i32);
    let out_len_ptr = module.locals.add(i32);

    // Locals.
    let mptr = module.locals.add(i32); // provider input buffer
    let mslots = module.locals.add(i32); // provider out-ptr/out-len slots
    let status = module.locals.add(i32);
    let m_out_ptr = module.locals.add(i32); // provider result ptr
    let m_out_len = module.locals.add(i32); // provider result len
    let aptr = module.locals.add(i32); // consumer result buffer

    let mem_cons = p.consumer_mem;
    let mem_prov = p.provider_mem;
    let prov_alloc = p.provider_alloc;
    let prov_free = p.provider_free;
    let prov_export = p.provider_export;
    let cons_alloc = p.consumer_alloc;

    let mem4 = MemArg {
        align: 2,
        offset: 0,
    };

    let mut body = builder.func_body();

    // mptr = provider.__pack_alloc(in_len)
    body.instr(LocalGet { local: in_len })
        .instr(Call { func: prov_alloc })
        .instr(LocalSet { local: mptr });

    // memory.copy dst=provider src=consumer : dst=mptr, src=in_ptr, len=in_len
    body.instr(LocalGet { local: mptr })
        .instr(LocalGet { local: in_ptr })
        .instr(LocalGet { local: in_len })
        .instr(MemoryCopy {
            src: mem_cons,
            dst: mem_prov,
        });

    // mslots = provider.__pack_alloc(8)
    body.instr(Const {
        value: IrValue::I32(8),
    })
    .instr(Call { func: prov_alloc })
    .instr(LocalSet { local: mslots });

    // status = provider.export(mptr, in_len, mslots, mslots+4)
    body.instr(LocalGet { local: mptr })
        .instr(LocalGet { local: in_len })
        .instr(LocalGet { local: mslots })
        .instr(LocalGet { local: mslots })
        .instr(Const {
            value: IrValue::I32(4),
        })
        .instr(Binop {
            op: BinaryOp::I32Add,
        })
        .instr(Call { func: prov_export })
        .instr(LocalSet { local: status });

    // m_out_ptr = i32.load provider [mslots]
    body.instr(LocalGet { local: mslots })
        .instr(walrus::ir::Load {
            memory: mem_prov,
            kind: LoadKind::I32 { atomic: false },
            arg: mem4,
        })
        .instr(LocalSet { local: m_out_ptr });

    // m_out_len = i32.load provider [mslots+4]
    body.instr(LocalGet { local: mslots })
        .instr(walrus::ir::Load {
            memory: mem_prov,
            kind: LoadKind::I32 { atomic: false },
            arg: MemArg {
                align: 2,
                offset: 4,
            },
        })
        .instr(LocalSet { local: m_out_len });

    // aptr = consumer.__pack_alloc(m_out_len)
    body.instr(LocalGet { local: m_out_len })
        .instr(Call { func: cons_alloc })
        .instr(LocalTee { local: aptr })
        // memory.copy dst=consumer src=provider : dst=aptr, src=m_out_ptr, len
        .instr(LocalGet { local: m_out_ptr })
        .instr(LocalGet { local: m_out_len })
        .instr(MemoryCopy {
            src: mem_prov,
            dst: mem_cons,
        });

    // i32.store consumer [out_ptr_ptr] = aptr
    body.instr(LocalGet { local: out_ptr_ptr })
        .instr(LocalGet { local: aptr })
        .instr(Store {
            memory: mem_cons,
            kind: StoreKind::I32 { atomic: false },
            arg: mem4,
        });

    // i32.store consumer [out_len_ptr] = m_out_len
    body.instr(LocalGet { local: out_len_ptr })
        .instr(LocalGet { local: m_out_len })
        .instr(Store {
            memory: mem_cons,
            kind: StoreKind::I32 { atomic: false },
            arg: mem4,
        });

    // provider.__pack_free(mptr, in_len)
    body.instr(LocalGet { local: mptr })
        .instr(LocalGet { local: in_len })
        .instr(Call { func: prov_free });

    // provider.__pack_free(m_out_ptr, m_out_len)
    body.instr(LocalGet { local: m_out_ptr })
        .instr(LocalGet { local: m_out_len })
        .instr(Call { func: prov_free });

    // provider.__pack_free(mslots, 8)
    body.instr(LocalGet { local: mslots })
        .instr(Const {
            value: IrValue::I32(8),
        })
        .instr(Call { func: prov_free });

    // Return the ownership status to the consumer's `__import_impl`. On success
    // return 1 (guest-allocated), NOT the provider export's status: `aptr` lives
    // in the CONSUMER's memory and was allocated by THIS shim via
    // `consumer.__pack_alloc`, so the consumer must free it — and `__import_impl`
    // frees the result buffer only for status == 1. The provider's pact export
    // returns 0 on success; passing that 0 through would leak `aptr` in the
    // consumer's memory on every call. On error (status < 0) propagate unchanged.
    //   result = (status < 0) ? status : 1
    body.instr(LocalGet { local: status }) // select val1 (returned when cond true)
        .instr(Const {
            value: IrValue::I32(1),
        }) // select val2 (returned when cond false)
        .instr(LocalGet { local: status })
        .instr(Const {
            value: IrValue::I32(0),
        })
        .instr(Binop {
            op: BinaryOp::I32LtS,
        }) // cond = status < 0
        .instr(Select { ty: None });

    let args: Vec<LocalId> = vec![in_ptr, in_len, out_ptr_ptr, out_len_ptr];
    builder.finish(args, &mut module.funcs)
}

/// The handles a host-bridge shim needs: the calling COMPONENT's memory + alloc
/// (where the guest's call args live), the ENTRY's memory + alloc/free (which the
/// host reads/writes), and the real host import to invoke.
struct HostShimParams {
    comp_mem: MemoryId,
    comp_alloc: FunctionId,
    entry_mem: MemoryId,
    entry_alloc: FunctionId,
    entry_free: FunctionId,
    host_import: FunctionId,
}

/// Emit a host-bridge shim and return its id.
///
/// Signature `(in_ptr, in_len, out_ptr_ptr, out_len_ptr) -> status`, all i32,
/// pointers into the CALLING COMPONENT's memory (memory 1, 2, …). A residual
/// host import is served by the host against the composite's canonical (entry,
/// memory 0) memory + allocator, so this shim marshals the call across:
///
/// 1. `entry.__pack_alloc(in_len)` → `e_in` (entry memory).
/// 2. copy input: component memory `[in_ptr]` → entry memory `[e_in]`.
/// 3. `entry.__pack_alloc(8)` → `e_slots` for the host's out ptr/len.
/// 4. call the real host import `(e_in, in_len, e_slots, e_slots+4)` — the host
///    reads its args from the entry memory (correct now), writes the result into
///    the entry memory, and stores its ptr/len into the entry slots.
/// 5. read `h_out_ptr`/`h_out_len` from the entry slots.
/// 6. `component.__pack_alloc(h_out_len)` → `c_out` (component memory).
/// 7. copy result: entry memory `[h_out_ptr]` → component memory `[c_out]`.
/// 8. store `c_out`/`h_out_len` into the component's out slots; free the entry
///    scratch; return the host's status.
///
/// This is exactly [`emit_shim`] with the provider role played by the entry
/// (memory 0) and the "export call" replaced by the residual host import — so a
/// component's residual host call marshals identically to a link call, just in
/// the other direction across the memory gap.
fn emit_host_shim(module: &mut Module, p: &HostShimParams) -> FunctionId {
    let i32 = ValType::I32;
    let mut builder = FunctionBuilder::new(&mut module.types, &[i32, i32, i32, i32], &[i32]);
    builder.name("__host_bridge_shim".to_string());

    // Params (pointers into the calling component's memory).
    let in_ptr = module.locals.add(i32);
    let in_len = module.locals.add(i32);
    let out_ptr_ptr = module.locals.add(i32);
    let out_len_ptr = module.locals.add(i32);

    // Locals.
    let e_in = module.locals.add(i32); // entry input buffer
    let e_slots = module.locals.add(i32); // entry out-ptr/out-len slots
    let status = module.locals.add(i32);
    let h_out_ptr = module.locals.add(i32); // host result ptr (entry mem)
    let h_out_len = module.locals.add(i32); // host result len
    let c_out = module.locals.add(i32); // component result buffer

    let comp_mem = p.comp_mem;
    let comp_alloc = p.comp_alloc;
    let entry_mem = p.entry_mem;
    let entry_alloc = p.entry_alloc;
    let entry_free = p.entry_free;
    let host_import = p.host_import;

    let mem4 = MemArg {
        align: 2,
        offset: 0,
    };

    // Pre-build the dangling sequences for the conditional free, before the main
    // body takes its mutable borrow of the builder. `free_host_buf` frees the
    // host's result buffer; `skip_free` is the empty else-branch. Which one runs
    // is decided at the tail by the host's return status.
    let free_host_buf = {
        let mut seq = builder.dangling_instr_seq(InstrSeqType::Simple(None));
        seq.instr(LocalGet { local: h_out_ptr })
            .instr(LocalGet { local: h_out_len })
            .instr(Call { func: entry_free });
        seq.id()
    };
    let skip_free = builder.dangling_instr_seq(InstrSeqType::Simple(None)).id();

    let mut body = builder.func_body();

    // e_in = entry.__pack_alloc(in_len)
    body.instr(LocalGet { local: in_len })
        .instr(Call { func: entry_alloc })
        .instr(LocalSet { local: e_in });

    // memory.copy dst=entry src=component : dst=e_in, src=in_ptr, len=in_len
    body.instr(LocalGet { local: e_in })
        .instr(LocalGet { local: in_ptr })
        .instr(LocalGet { local: in_len })
        .instr(MemoryCopy {
            src: comp_mem,
            dst: entry_mem,
        });

    // e_slots = entry.__pack_alloc(8)
    body.instr(Const {
        value: IrValue::I32(8),
    })
    .instr(Call { func: entry_alloc })
    .instr(LocalSet { local: e_slots });

    // status = host_import(e_in, in_len, e_slots, e_slots+4)
    body.instr(LocalGet { local: e_in })
        .instr(LocalGet { local: in_len })
        .instr(LocalGet { local: e_slots })
        .instr(LocalGet { local: e_slots })
        .instr(Const {
            value: IrValue::I32(4),
        })
        .instr(Binop {
            op: BinaryOp::I32Add,
        })
        .instr(Call { func: host_import })
        .instr(LocalSet { local: status });

    // h_out_ptr = i32.load entry [e_slots]
    body.instr(LocalGet { local: e_slots })
        .instr(walrus::ir::Load {
            memory: entry_mem,
            kind: LoadKind::I32 { atomic: false },
            arg: mem4,
        })
        .instr(LocalSet { local: h_out_ptr });

    // h_out_len = i32.load entry [e_slots+4]
    body.instr(LocalGet { local: e_slots })
        .instr(walrus::ir::Load {
            memory: entry_mem,
            kind: LoadKind::I32 { atomic: false },
            arg: MemArg {
                align: 2,
                offset: 4,
            },
        })
        .instr(LocalSet { local: h_out_len });

    // c_out = component.__pack_alloc(h_out_len)
    body.instr(LocalGet { local: h_out_len })
        .instr(Call { func: comp_alloc })
        .instr(LocalTee { local: c_out })
        // memory.copy dst=component src=entry : dst=c_out, src=h_out_ptr, len
        .instr(LocalGet { local: h_out_ptr })
        .instr(LocalGet { local: h_out_len })
        .instr(MemoryCopy {
            src: entry_mem,
            dst: comp_mem,
        });

    // i32.store component [out_ptr_ptr] = c_out
    body.instr(LocalGet { local: out_ptr_ptr })
        .instr(LocalGet { local: c_out })
        .instr(Store {
            memory: comp_mem,
            kind: StoreKind::I32 { atomic: false },
            arg: mem4,
        });

    // i32.store component [out_len_ptr] = h_out_len
    body.instr(LocalGet { local: out_len_ptr })
        .instr(LocalGet { local: h_out_len })
        .instr(Store {
            memory: comp_mem,
            kind: StoreKind::I32 { atomic: false },
            arg: mem4,
        });

    // entry.__pack_free(e_in, in_len)
    body.instr(LocalGet { local: e_in })
        .instr(LocalGet { local: in_len })
        .instr(Call { func: entry_free });

    // entry.__pack_free(e_slots, 8)
    body.instr(LocalGet { local: e_slots })
        .instr(Const {
            value: IrValue::I32(8),
        })
        .instr(Call { func: entry_free });

    // Free the host's result buffer (`h_out_ptr`, entry memory) IFF the host
    // guest-allocated it. The raw host-import ABI signals ownership via the status:
    //   status == 1 => host guest-allocated the return via entry.__pack_alloc, the
    //                  guest (this bridge is that guest-side consumer) owns + frees it;
    //   status == 0 => host wrote its own fixed scratch buffer — must NOT free it
    //                  (that pointer was never handed out by entry.__pack_alloc).
    // See packr-guest `__import_impl`, which frees on exactly the same condition.
    // Without this, `h_out_ptr` leaked on the async / guest-allocated path — a
    // GROWING leak (entry alloc is dlmalloc, not a bump) per residual host call.
    body.instr(LocalGet { local: status })
        .instr(Const {
            value: IrValue::I32(1),
        })
        .instr(Binop {
            op: BinaryOp::I32Eq,
        })
        .instr(IfElse {
            consequent: free_host_buf,
            alternative: skip_free,
        });

    // Return the ownership status to the calling component's `__import_impl`. On
    // success we return 1 (guest-allocated), NOT the host's raw status: `c_out`
    // lives in the COMPONENT's memory and was allocated by THIS bridge via
    // `comp_alloc`, so the component must free it — and `__import_impl` frees the
    // result buffer only for status == 1. Passing the host's raw 0 through here
    // (host-owned scratch) would leak `c_out` in the component's memory. On error
    // (status < 0) propagate the host's status unchanged.
    //   result = (status < 0) ? status : 1
    body.instr(LocalGet { local: status }) // select val1 (returned when cond true)
        .instr(Const {
            value: IrValue::I32(1),
        }) // select val2 (returned when cond false)
        .instr(LocalGet { local: status })
        .instr(Const {
            value: IrValue::I32(0),
        })
        .instr(Binop {
            op: BinaryOp::I32LtS,
        }) // cond = status < 0
        .instr(Select { ty: None });

    let args: Vec<LocalId> = vec![in_ptr, in_len, out_ptr_ptr, out_len_ptr];
    builder.finish(args, &mut module.funcs)
}

/// Remove the `__c_<name>_*` scaffolding exports so the composite presents only
/// the entry component's surface. Harmless to leave, but cleaner to drop.
fn strip_scoped_exports(
    mut module: Module,
    component_names: &[String],
    entry_name: &str,
) -> Module {
    let prefixes: Vec<String> = component_names
        .iter()
        .filter(|n| *n != entry_name)
        .map(|n| scoped_name(n, ""))
        .collect();

    let to_remove: Vec<_> = module
        .exports
        .iter()
        .filter(|e| prefixes.iter().any(|p| e.name.starts_with(p)))
        .map(|e| e.id())
        .collect();
    for id in to_remove {
        module.exports.delete(id);
    }
    module
}
