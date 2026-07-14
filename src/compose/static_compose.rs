//! Static composition of PIC packages into one self-contained `.wasm`.
//!
//! Pipeline:
//! 1. **`wasm-merge`** (binaryen) fuses the package modules and internalizes the
//!    cross-package imports — `(import "math" "double")` becomes a direct call to
//!    the provider's `double`, `(import "pack:alloc" ...)` becomes a call to the
//!    merged-in allocator. Each package is named by the import-module-name its
//!    consumers use (`"math"`, `"pack:alloc"`, ...).
//! 2. A **`walrus`** pass does the one thing `wasm-merge` can't: unify the several
//!    `env.memory` imports into a single internal memory, turn the allocator's
//!    `__memory_base` / `__heap_base` / `__heap_end` imports into baked-in
//!    constants, and export the memory.
//!
//! The result has ZERO imports and validates on any stock runtime.
//!
//! Requires `wasm-merge` (binaryen) on `PATH` at compose time.

use std::path::PathBuf;
use std::process::Command;

use walrus::ir::VisitorMut;
use walrus::{ConstExpr, DataKind, GlobalKind, ImportKind, MemoryId, Module};

/// One package in a static composition.
pub struct PackageSpec {
    /// The `wasm-merge` module name — the import module-name that *consumers* use
    /// for this package's exports. E.g. `"math"` if a consumer does
    /// `(import "math" "double")`, or `"pack:alloc"` for the allocator.
    ///
    /// Providers must be listed before the packages that consume them.
    pub name: String,
    /// Compiled package bytes, built with the fixed-base composition recipe
    /// (`--global-base=<BASE> --stack-first -zstack-size=... --import-memory
    /// --no-entry`), with disjoint bases per package.
    pub wasm: Vec<u8>,
}

impl PackageSpec {
    pub fn new(name: impl Into<String>, wasm: Vec<u8>) -> Self {
        Self {
            name: name.into(),
            wasm,
        }
    }
}

/// Byte-offset layout of the single shared linear memory the composed module owns.
/// Package data segments sit at their link-time fixed bases; the allocator's
/// mstate + heap live above them.
#[derive(Clone, Copy, Debug)]
pub struct Layout {
    /// Internal memory size, in 64KiB pages.
    pub memory_pages: u32,
    /// Allocator `__memory_base` — where its `.data` (mstate) is placed. Must be
    /// above every package's data region.
    pub alloc_base: u32,
    /// dlmalloc heap start (above `alloc_base`).
    pub heap_base: u32,
    /// dlmalloc heap end.
    pub heap_end: u32,
    /// Where a regenerated `__pack_types` blob is placed. Must be a free region
    /// (default sits in the gap between the first package's data and the second's
    /// stack, `[0x51400, 0x90000)`).
    pub metadata_base: u32,
}

impl Default for Layout {
    fn default() -> Self {
        // Matches the two-package (doubler @0x50000, adder @0xD0000) recipe with
        // room to spare; callers with more/larger packages should widen this.
        Self {
            memory_pages: 128, // 8 MiB
            alloc_base: 0xE_0000,
            heap_base: 0xF_0000,
            heap_end: 0x80_0000,
            metadata_base: 0x6_0000,
        }
    }
}

/// A static-composition request: the packages (in provider-before-consumer order)
/// and the memory layout.
pub struct ComposeSpec {
    pub packages: Vec<PackageSpec>,
    pub layout: Layout,
}

impl ComposeSpec {
    pub fn new(packages: Vec<PackageSpec>) -> Self {
        Self {
            packages,
            layout: Layout::default(),
        }
    }
}

/// Compose `spec` into a single self-contained `.wasm` (zero imports).
pub fn compose(spec: &ComposeSpec) -> anyhow::Result<Vec<u8>> {
    anyhow::ensure!(!spec.packages.is_empty(), "no packages to compose");
    let merged = wasm_merge(&spec.packages)?;
    internalize(&merged, &spec.layout)
}

/// Shell out to binaryen's `wasm-merge` to fuse modules + wire cross-imports.
fn wasm_merge(packages: &[PackageSpec]) -> anyhow::Result<Vec<u8>> {
    // Unique per call (pid + a process-wide counter) so concurrent compositions —
    // e.g. parallel tests — don't clobber each other's scratch files.
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let token = format!(
        "{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    );
    let dir = std::env::temp_dir().join(format!("pack-compose-{token}"));
    std::fs::create_dir_all(&dir)?;

    let mut cmd = Command::new("wasm-merge");
    let mut inputs: Vec<PathBuf> = Vec::new();
    for (i, p) in packages.iter().enumerate() {
        let path = dir.join(format!("in{i}.wasm"));
        std::fs::write(&path, &p.wasm)?;
        cmd.arg(&path).arg(&p.name);
        inputs.push(path);
    }
    let out = dir.join("merged.wasm");
    cmd.arg("-o")
        .arg(&out)
        .arg("--enable-multimemory")
        .arg("--rename-export-conflicts");

    let status = cmd.status().map_err(|e| {
        anyhow::anyhow!("failed to run `wasm-merge` (is binaryen installed and on PATH?): {e}")
    })?;
    anyhow::ensure!(status.success(), "`wasm-merge` exited with failure");

    let bytes = std::fs::read(&out)?;
    let _ = std::fs::remove_dir_all(&dir);
    Ok(bytes)
}

/// Remaps every memory reference in a function body to a single canonical memory.
struct MemRemap {
    to: MemoryId,
}
impl VisitorMut for MemRemap {
    fn visit_memory_id_mut(&mut self, id: &mut MemoryId) {
        *id = self.to;
    }
}

/// The `walrus` pass: unify memories, internalize the allocator's base/heap
/// globals into constants, export the memory. Yields a zero-import module.
fn internalize(merged: &[u8], layout: &Layout) -> anyhow::Result<Vec<u8>> {
    let mut m = Module::from_buffer(merged)?;

    // 1. Turn the allocator's imported globals into baked-in constants.
    let mut global_fixups = Vec::new();
    for imp in m.imports.iter() {
        if let ImportKind::Global(g) = imp.kind {
            let val: i32 = match imp.name.as_str() {
                "__memory_base" => layout.alloc_base as i32,
                "__table_base" => 0,
                "__heap_base" => layout.heap_base as i32,
                "__heap_end" => layout.heap_end as i32,
                other => anyhow::bail!(
                    "unexpected global import `{}::{other}` — packages must use the \
                     fixed-base composition recipe",
                    imp.module
                ),
            };
            global_fixups.push((imp.id(), g, val));
        }
    }
    for (imp_id, g, val) in &global_fixups {
        m.globals.get_mut(*g).kind =
            GlobalKind::Local(ConstExpr::Value(walrus::ir::Value::I32(*val)));
        m.imports.delete(*imp_id);
    }

    // 2. Unify the several `env.memory` imports into one internal memory.
    let mut mem_imports = Vec::new();
    for imp in m.imports.iter() {
        if let ImportKind::Memory(mem) = imp.kind {
            mem_imports.push((imp.id(), mem));
        }
    }
    anyhow::ensure!(
        !mem_imports.is_empty(),
        "merged module has no memory import"
    );
    let canonical = mem_imports[0].1;

    let mut v = MemRemap { to: canonical };
    let fids: Vec<_> = m.funcs.iter_local().map(|(id, _)| id).collect();
    for fid in fids {
        let f = m.funcs.get_mut(fid).kind.unwrap_local_mut();
        let entry = f.entry_block();
        walrus::ir::dfs_pre_order_mut(&mut v, f, entry);
    }

    let data_ids: Vec<_> = m.data.iter().map(|d| d.id()).collect();
    for did in data_ids {
        let d = m.data.get_mut(did);
        if let DataKind::Active { memory, offset } = &mut d.kind {
            *memory = canonical;
            // A local `global.get` is not a valid data-segment offset; the
            // allocator segment used the (now-internalized) __memory_base global.
            if let ConstExpr::Global(_) = offset {
                *offset = ConstExpr::Value(walrus::ir::Value::I32(layout.alloc_base as i32));
            }
        }
    }

    {
        let mem = m.memories.get_mut(canonical);
        mem.import = None;
        mem.initial = layout.memory_pages as u64;
        mem.maximum = Some(layout.memory_pages as u64);
    }
    for (imp_id, mem_id) in mem_imports {
        m.imports.delete(imp_id);
        if mem_id != canonical {
            m.memories.delete(mem_id);
        }
    }

    // 3. Export the memory so a host can marshal args/results through it.
    if !m.exports.iter().any(|e| e.name == "memory") {
        m.exports.add("memory", canonical);
    }

    // 4. It must now stand alone.
    let remaining = m.imports.iter().count();
    anyhow::ensure!(
        remaining == 0,
        "expected zero imports after internalize, got {remaining}"
    );

    // Stamp the self-contained-composite marker: a forward-compat ABI-version tag
    // (v1 = exported memory + __pack_alloc/__pack_free + lifecycle). Not a
    // discriminator — with the universal self-contained contract, fail-loud is
    // intrinsic (an unprovided import fails at instantiate) — just a handle for a
    // host to read which export contract to validate.
    m.customs.add(walrus::RawCustomSection {
        name: "pack.composite".to_string(),
        data: {
            let mut d = Vec::with_capacity(8);
            d.extend_from_slice(&1u32.to_le_bytes()); // version
            d.extend_from_slice(&0u32.to_le_bytes()); // flags
            d
        },
    });

    Ok(m.emit_wasm())
}
