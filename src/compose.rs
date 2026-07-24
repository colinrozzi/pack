//! Component composition — Milestone 1.
//!
//! Compose two isolated wasm "components" (a consumer + a provider) into ONE
//! multi-memory wasm binary, wiring the consumer's pact import to the provider's
//! pact export through a statically-generated **bridging shim**.
//!
//! # The model (see `docs/component-composition.md`)
//!
//! The composite is a single module with **one memory per component**: memory 0
//! is the consumer's, memory 1 is the provider's. Each component keeps its own
//! `__stack_pointer`, data and heap in its own memory, so their addresses can
//! never collide — the whole reconciliation bug class the retired fusion suffered
//! is structurally impossible here.
//!
//! # How it works
//!
//! `wasm-merge --enable-multimemory` does the module merge: it places the
//! provider's code + memory as memory 1 and remaps the provider's own memory
//! accesses to memory 1, while leaving the consumer's import unwired (because the
//! provider is merged under a name that does not match the import module name).
//! This module is the walrus post-pass that:
//!
//! 1. Pre-renames the provider's colliding exports (`memory`, `__pack_alloc`,
//!    `__pack_free`, and the linked export) so they survive the merge findable.
//! 2. Shells out to `wasm-merge` to produce the two-memory module.
//! 3. Builds a shim function (alloc in memory 1 → copy in → call provider →
//!    copy result out → free) and rewrites every call to the consumer's imported
//!    function to call the shim instead, then deletes the now-dead import.
//! 4. Removes the `__prov_*` scaffolding exports so the composite presents only
//!    the consumer's surface (`memory`, `__pack_alloc`/`__pack_free`, pacts).
//!
//! The shim is pure byte-shuffling over the actor ABI
//! (`fn(in_ptr, in_len, out_ptr_ptr, out_len_ptr) -> status`), so it never needs
//! to know the pact's high-level types.

use anyhow::{anyhow, Context, Result};
use walrus::ir::{
    BinaryOp, Binop, Call, Const, Instr, LoadKind, LocalGet, LocalSet, LocalTee, MemArg,
    MemoryCopy, Store, StoreKind, Value as IrValue,
};
use walrus::{
    ExportItem, FunctionBuilder, FunctionId, ImportKind, LocalId, MemoryId, Module, ValType,
};

// Names the renamed provider exports get, so they survive the merge and are
// findable in the merged module. Chosen to not collide with anything a normal
// actor exports.
const PROV_ALLOC: &str = "__prov_pack_alloc";
const PROV_FREE: &str = "__prov_pack_free";
const PROV_EXPORT: &str = "__prov_export";
const PROV_MEMORY: &str = "__prov_memory";

/// A single link: the consumer's import `(import_module, import_name)` is
/// satisfied by the provider's export `export_name`.
#[derive(Debug, Clone)]
pub struct Link {
    /// The import module the consumer declares (e.g. `"math"`).
    pub import_module: String,
    /// The import field the consumer declares (e.g. `"double"`).
    pub import_name: String,
    /// The provider's export that satisfies it (e.g. `"double"`).
    pub export_name: String,
}

/// Compose a consumer and a provider into one multi-memory composite wasm.
///
/// The consumer's import `(link.import_module, link.import_name)` is wired to the
/// provider's export `link.export_name` via a bridging shim. The result exports
/// the consumer's memory as `memory`, the consumer's `__pack_alloc`/`__pack_free`,
/// and the consumer's pact functions; the provider lives entirely in memory 1.
pub fn compose_pair(consumer_wasm: &[u8], provider_wasm: &[u8], link: &Link) -> Result<Vec<u8>> {
    // Step 1: pre-rename the provider's colliding exports so they survive the
    // merge and stay findable by name in the merged module.
    let renamed_provider = rename_provider_exports(provider_wasm, &link.export_name)
        .context("renaming provider exports before merge")?;

    // Step 2: merge with wasm-merge (multi-memory). The provider name `prov2`
    // must differ from `link.import_module` so the consumer's import stays.
    let merged = merge_multimemory(consumer_wasm, &renamed_provider)
        .context("merging consumer + provider into a multi-memory module")?;

    // Step 3 + 4: emit the shim, rewire the consumer's import to it, and clean up.
    let composite = shim_and_rewire(&merged).context("emitting shim and rewiring import")?;

    Ok(composite)
}

/// Rename ALL of the provider's exports so none collides with the consumer's on
/// merge (`wasm-merge` aborts on any duplicate export name — not just `memory`).
/// The four load-bearing ones (`__pack_alloc`, `__pack_free`, the linked
/// `export_name`, `memory`) get the fixed `__prov_*` names the shim looks up by;
/// every other export (`__pack_types`, `__heap_base`, `__data_end`, …) gets a
/// generic `__prov__<orig>` prefix so it survives merge harmlessly and is dropped
/// later. The provider's exports are internal to the composite regardless.
fn rename_provider_exports(provider_wasm: &[u8], export_name: &str) -> Result<Vec<u8>> {
    let mut module = Module::from_buffer(provider_wasm)?;

    // The four we must be able to find again by name after the merge.
    let mapped = [
        ("__pack_alloc", PROV_ALLOC),
        ("__pack_free", PROV_FREE),
        (export_name, PROV_EXPORT),
        ("memory", PROV_MEMORY),
    ];
    for (from, _) in mapped {
        if !module.exports.iter().any(|e| e.name == from) {
            return Err(anyhow!("provider is missing required export `{from}`"));
        }
    }

    let ids: Vec<_> = module.exports.iter().map(|e| e.id()).collect();
    for id in ids {
        let name = module.exports.get(id).name.clone();
        let new = mapped
            .iter()
            .find(|(from, _)| *from == name)
            .map(|(_, to)| to.to_string())
            .unwrap_or_else(|| format!("__prov__{name}"));
        module.exports.get_mut(id).name = new;
    }

    Ok(module.emit_wasm())
}

/// Shell out to `wasm-merge --enable-multimemory`, writing inputs to temp files.
fn merge_multimemory(consumer_wasm: &[u8], provider_wasm: &[u8]) -> Result<Vec<u8>> {
    let dir = std::env::temp_dir();
    let pid = std::process::id();
    let cons_path = dir.join(format!("packr-compose-cons-{pid}.wasm"));
    let prov_path = dir.join(format!("packr-compose-prov-{pid}.wasm"));
    let out_path = dir.join(format!("packr-compose-out-{pid}.wasm"));

    std::fs::write(&cons_path, consumer_wasm)?;
    std::fs::write(&prov_path, provider_wasm)?;

    // `wasm-merge` is provided via nix. Prefer a bare `wasm-merge` on PATH;
    // fall back to invoking it through `nix shell` if it is not.
    let status = run_wasm_merge(&cons_path, &prov_path, &out_path)?;

    let cleanup = |paths: &[&std::path::Path]| {
        for p in paths {
            let _ = std::fs::remove_file(p);
        }
    };

    if !status {
        cleanup(&[&cons_path, &prov_path, &out_path]);
        return Err(anyhow!("wasm-merge failed"));
    }

    let merged = std::fs::read(&out_path)?;
    cleanup(&[&cons_path, &prov_path, &out_path]);
    Ok(merged)
}

/// Invoke `wasm-merge <cons> app <prov> prov2 -o <out> --enable-multimemory`.
///
/// Tries a bare `wasm-merge` first; if that isn't on PATH, retries through
/// `nix shell nixpkgs#binaryen -c wasm-merge ...` so the transform works in the
/// dev environment without a global binaryen install.
fn run_wasm_merge(
    cons: &std::path::Path,
    prov: &std::path::Path,
    out: &std::path::Path,
) -> Result<bool> {
    use std::process::Command;

    let args = |bin: &str| -> Vec<String> {
        vec![
            bin.to_string(),
            cons.display().to_string(),
            "app".to_string(),
            prov.display().to_string(),
            "prov2".to_string(),
            "-o".to_string(),
            out.display().to_string(),
            "--enable-multimemory".to_string(),
        ]
    };

    // First: bare `wasm-merge`.
    let direct = Command::new("wasm-merge")
        .args(&args("wasm-merge")[1..])
        .output();
    match direct {
        Ok(o) if o.status.success() => return Ok(true),
        Ok(o) => {
            // ran but failed — surface stderr
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
        ];
        v.extend(args("wasm-merge"));
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

/// Handles the shim emission + import rewire on the merged module.
struct Merged {
    module: Module,
    mem_consumer: MemoryId,
    mem_provider: MemoryId,
    consumer_alloc: FunctionId,
    prov_alloc: FunctionId,
    prov_free: FunctionId,
    prov_export: FunctionId,
    import_func: FunctionId,
}

fn shim_and_rewire(merged_wasm: &[u8]) -> Result<Vec<u8>> {
    let module = Module::from_buffer(merged_wasm)?;
    let ctx = locate(module)?;
    let ctx = build_shim_and_rewire(ctx)?;
    let mut module = strip_prov_exports(ctx.module);
    Ok(module.emit_wasm())
}

/// Find the memories, provider funcs, consumer alloc, and consumer import.
fn locate(module: Module) -> Result<Merged> {
    // memory 0 = consumer's (exported `memory`); memory 1 = provider's
    // (exported `__prov_memory`).
    let mem_consumer = export_memory(&module, "memory")
        .or_else(|| module.memories.iter().next().map(|m| m.id()))
        .ok_or_else(|| anyhow!("merged module has no memory"))?;
    let mem_provider = export_memory(&module, PROV_MEMORY).ok_or_else(|| {
        anyhow!("merged module is missing the renamed provider memory `{PROV_MEMORY}`")
    })?;
    if mem_consumer == mem_provider {
        return Err(anyhow!(
            "consumer and provider memories resolved to the same memory — merge did not \
             produce two memories"
        ));
    }

    let consumer_alloc = export_func(&module, "__pack_alloc")
        .ok_or_else(|| anyhow!("merged module is missing consumer export `__pack_alloc`"))?;
    let prov_alloc = export_func(&module, PROV_ALLOC)
        .ok_or_else(|| anyhow!("merged module is missing provider export `{PROV_ALLOC}`"))?;
    let prov_free = export_func(&module, PROV_FREE)
        .ok_or_else(|| anyhow!("merged module is missing provider export `{PROV_FREE}`"))?;
    let prov_export = export_func(&module, PROV_EXPORT)
        .ok_or_else(|| anyhow!("merged module is missing provider export `{PROV_EXPORT}`"))?;

    // The consumer's still-unwired imported function.
    let import_func = find_sole_func_import(&module)?;

    Ok(Merged {
        module,
        mem_consumer,
        mem_provider,
        consumer_alloc,
        prov_alloc,
        prov_free,
        prov_export,
        import_func,
    })
}

/// Find the (single) remaining imported function — the consumer's unwired pact
/// import. After a clean merge the only surviving function import is the link's.
fn find_sole_func_import(module: &Module) -> Result<FunctionId> {
    let mut found = None;
    for imp in module.imports.iter() {
        if let ImportKind::Function(f) = imp.kind {
            if found.is_some() {
                return Err(anyhow!(
                    "expected exactly one function import after merge, found several — \
                     multi-link composition is not supported in M1"
                ));
            }
            found = Some(f);
        }
    }
    found.ok_or_else(|| {
        anyhow!("no function import found after merge — the consumer's import was already wired?")
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

/// Build the bridging shim and rewrite the consumer's import to call it.
fn build_shim_and_rewire(mut m: Merged) -> Result<Merged> {
    let shim_id = emit_shim(&mut m);

    // Rewire: every `Call { func: import_func }` → `Call { func: shim_id }`.
    let import_func = m.import_func;
    for (_id, func) in m.module.funcs.iter_local_mut() {
        let entry = func.entry_block();
        let mut rewriter = CallRewriter {
            from: import_func,
            to: shim_id,
        };
        walrus::ir::dfs_pre_order_mut(&mut rewriter, func, entry);
    }

    // Delete the now-dead import (both the import entry and the func stub).
    let import_id = m
        .module
        .imports
        .iter()
        .find(|imp| matches!(imp.kind, ImportKind::Function(f) if f == import_func))
        .map(|imp| imp.id());
    if let Some(id) = import_id {
        m.module.imports.delete(id);
    }
    m.module.funcs.delete(import_func);

    Ok(m)
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

/// Emit the bridging shim function and return its id.
///
/// Signature `(in_ptr, in_len, out_ptr_ptr, out_len_ptr) -> status`, all i32,
/// pointers into memory 0 (the consumer's). The shim marshals across to the
/// provider in memory 1, calls its export, and marshals the result back.
fn emit_shim(m: &mut Merged) -> FunctionId {
    let i32 = ValType::I32;
    let mut builder = FunctionBuilder::new(&mut m.module.types, &[i32, i32, i32, i32], &[i32]);
    builder.name("__bridge_shim".to_string());

    // Params.
    let in_ptr = m.module.locals.add(i32);
    let in_len = m.module.locals.add(i32);
    let out_ptr_ptr = m.module.locals.add(i32);
    let out_len_ptr = m.module.locals.add(i32);

    // Locals.
    let mptr = m.module.locals.add(i32); // provider input buffer (memory 1)
    let mslots = m.module.locals.add(i32); // provider out-ptr/out-len slots (memory 1)
    let status = m.module.locals.add(i32);
    let m_out_ptr = m.module.locals.add(i32); // provider result ptr (memory 1)
    let m_out_len = m.module.locals.add(i32); // provider result len
    let aptr = m.module.locals.add(i32); // consumer result buffer (memory 0)

    let src = m.mem_consumer;
    let dst = m.mem_provider;
    let mem0 = m.mem_consumer;
    let mem1 = m.mem_provider;

    let prov_alloc = m.prov_alloc;
    let prov_free = m.prov_free;
    let prov_export = m.prov_export;
    let consumer_alloc = m.consumer_alloc;

    let mem4 = MemArg {
        align: 2,
        offset: 0,
    };

    let mut body = builder.func_body();

    // mptr = __prov_pack_alloc(in_len)
    body.instr(LocalGet { local: in_len })
        .instr(Call { func: prov_alloc })
        .instr(LocalSet { local: mptr });

    // memory.copy dst=mem1 src=mem0 : dst=mptr, src=in_ptr, len=in_len
    body.instr(LocalGet { local: mptr })
        .instr(LocalGet { local: in_ptr })
        .instr(LocalGet { local: in_len })
        .instr(MemoryCopy { src, dst });

    // mslots = __prov_pack_alloc(8)
    body.instr(Const {
        value: IrValue::I32(8),
    })
    .instr(Call { func: prov_alloc })
    .instr(LocalSet { local: mslots });

    // status = __prov_export(mptr, in_len, mslots, mslots+4)
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

    // m_out_ptr = i32.load memory1 [mslots]
    body.instr(LocalGet { local: mslots })
        .instr(walrus::ir::Load {
            memory: mem1,
            kind: LoadKind::I32 { atomic: false },
            arg: mem4,
        })
        .instr(LocalSet { local: m_out_ptr });

    // m_out_len = i32.load memory1 [mslots+4]
    body.instr(LocalGet { local: mslots })
        .instr(walrus::ir::Load {
            memory: mem1,
            kind: LoadKind::I32 { atomic: false },
            arg: MemArg {
                align: 2,
                offset: 4,
            },
        })
        .instr(LocalSet { local: m_out_len });

    // aptr = __pack_alloc(m_out_len)   (consumer, memory 0)
    body.instr(LocalGet { local: m_out_len })
        .instr(Call {
            func: consumer_alloc,
        })
        .instr(LocalTee { local: aptr })
        // memory.copy dst=mem0 src=mem1 : dst=aptr, src=m_out_ptr, len=m_out_len
        // (aptr already on stack from the tee)
        .instr(LocalGet { local: m_out_ptr })
        .instr(LocalGet { local: m_out_len })
        .instr(MemoryCopy {
            src: mem1,
            dst: mem0,
        });

    // i32.store memory0 [out_ptr_ptr] = aptr
    body.instr(LocalGet { local: out_ptr_ptr })
        .instr(LocalGet { local: aptr })
        .instr(Store {
            memory: mem0,
            kind: StoreKind::I32 { atomic: false },
            arg: mem4,
        });

    // i32.store memory0 [out_len_ptr] = m_out_len
    body.instr(LocalGet { local: out_len_ptr })
        .instr(LocalGet { local: m_out_len })
        .instr(Store {
            memory: mem0,
            kind: StoreKind::I32 { atomic: false },
            arg: mem4,
        });

    // __prov_pack_free(mptr, in_len)
    body.instr(LocalGet { local: mptr })
        .instr(LocalGet { local: in_len })
        .instr(Call { func: prov_free });

    // __prov_pack_free(m_out_ptr, m_out_len)
    body.instr(LocalGet { local: m_out_ptr })
        .instr(LocalGet { local: m_out_len })
        .instr(Call { func: prov_free });

    // __prov_pack_free(mslots, 8)
    body.instr(LocalGet { local: mslots })
        .instr(Const {
            value: IrValue::I32(8),
        })
        .instr(Call { func: prov_free });

    // return status
    body.instr(LocalGet { local: status });

    let args: Vec<LocalId> = vec![in_ptr, in_len, out_ptr_ptr, out_len_ptr];
    builder.finish(args, &mut m.module.funcs)
}

/// Remove the `__prov_*` scaffolding exports so the composite presents only the
/// consumer's surface. Harmless to leave, but cleaner to drop.
fn strip_prov_exports(mut module: Module) -> Module {
    let to_remove: Vec<_> = module
        .exports
        .iter()
        .filter(|e| {
            matches!(
                e.name.as_str(),
                PROV_ALLOC | PROV_FREE | PROV_EXPORT | PROV_MEMORY
            ) || e.name.starts_with("__prov__")
        })
        .map(|e| e.id())
        .collect();
    for id in to_remove {
        module.exports.delete(id);
    }
    module
}
