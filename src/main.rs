//! Pack CLI - tools for working with Pack packages
//!
//! Commands:
//!   packr inspect <wasm>       - Display a package's metadata
//!   packr compose <manifest>   - Statically fuse packages into one `.wasm`
//!   packr link <manifest>      - Hash-checked interface linking into one `.wasm`
//!   packr build [crate]        - Actor crate → theater-loadable composite

use clap::{Parser, Subcommand};
use packr::{decode_metadata_with_hashes, Arena, Function, Param, ParsedModule, Type};
use std::path::PathBuf;

/// CGRF magic bytes: "CGRF" in little-endian
const CGRF_MAGIC: [u8; 4] = [0x43, 0x47, 0x52, 0x46];

#[derive(Parser)]
#[command(name = "pack")]
#[command(about = "Tools for working with Pack packages", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Inspect a WASM package and display its metadata
    Inspect {
        /// Path to the WASM file
        wasm_file: PathBuf,

        /// Show interface hashes
        #[arg(long, short = 'H')]
        hashes: bool,

        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Statically compose packages into one self-contained `.wasm` (zero imports)
    Compose {
        /// Path to the compose manifest (TOML)
        manifest: PathBuf,

        /// Output path (overrides `output` in the manifest)
        #[arg(long, short = 'o')]
        output: Option<PathBuf>,
    },

    /// Link packages via explicit, hash-checked interface links into one `.wasm`
    Link {
        /// Path to the link manifest (TOML)
        manifest: PathBuf,

        /// Output path (overrides `output` in the manifest)
        #[arg(long, short = 'o')]
        output: Option<PathBuf>,
    },

    /// Build an actor crate into a theater-loadable self-contained composite.
    ///
    /// One command in place of the hand-authored fixed-base recipe + separate
    /// link step: runs `cargo build` for `wasm32-unknown-unknown` with the recipe
    /// injected (no `.cargo/config` needed), then links the member against packr's
    /// bundled allocator into one self-contained `.wasm`. Requires the
    /// `wasm32-unknown-unknown` target and `wasm-merge` (binaryen) on `PATH`.
    Build {
        /// Actor crate directory (single actor), OR a multi-member build manifest
        /// (`.toml`: member crates + link edges). Default: current directory.
        #[arg(default_value = ".")]
        path: PathBuf,

        /// Output composite path (default: `<name>.composite.wasm` beside the
        /// cargo artifact, or the manifest's `output`)
        #[arg(long, short = 'o')]
        output: Option<PathBuf>,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Inspect {
            wasm_file,
            hashes,
            json,
        } => inspect_command(&wasm_file, hashes, json),
        Commands::Compose { manifest, output } => compose_command(&manifest, output),
        Commands::Link { manifest, output } => link_command(&manifest, output),
        Commands::Build { path, output } => build_command(&path, output),
    }
}

/// A `packr build` MULTI-MEMBER manifest: member crates + explicit link edges.
/// packr assigns each member a disjoint memory region, builds it, and links —
/// the author never types a base address or a Layout.
///
/// ```toml
/// [[member]]
/// name  = "example-app"
/// crate = "./example-app"
///
/// [[member]]
/// name  = "mesh-client"
/// crate = "./mesh-client"
///
/// [[link]]                 # <member>.<interface>  <-  <member>.<interface>
/// from = "example-app.mesh:client/protocol"
/// to   = "mesh-client.mesh:client/protocol"
///
/// output = "app.composite.wasm"   # optional
/// ```
#[derive(serde::Deserialize)]
struct BuildManifest {
    #[serde(default)]
    member: Vec<BuildMember>,
    #[serde(default)]
    link: Vec<LinkEntry>,
    output: Option<String>,
}

#[derive(serde::Deserialize)]
struct BuildMember {
    /// Member alias, used in `[[link]]` edges.
    name: String,
    /// Path to the member crate (relative to the manifest).
    #[serde(rename = "crate")]
    crate_path: String,
}

/// The member set + edges resolved from a `packr build` argument (a crate dir or
/// a multi-member manifest).
struct BuildInputs {
    /// (member alias, crate path).
    members: Vec<(String, String)>,
    links: Vec<LinkEntry>,
    output: Option<String>,
    /// Directory the member crate paths are relative to.
    base_dir: PathBuf,
}

/// A `.toml` argument is a multi-member manifest; a directory is a single-actor
/// crate (one implicit member, no edges).
fn resolve_build_inputs(path: &std::path::Path) -> anyhow::Result<BuildInputs> {
    let is_manifest = path.is_file() && path.extension().map(|e| e == "toml").unwrap_or(false);
    if is_manifest {
        let text = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("read manifest {}: {e}", path.display()))?;
        let m: BuildManifest = toml::from_str(&text)
            .map_err(|e| anyhow::anyhow!("parse manifest {}: {e}", path.display()))?;
        anyhow::ensure!(!m.member.is_empty(), "manifest lists no [[member]] entries");
        Ok(BuildInputs {
            members: m
                .member
                .into_iter()
                .map(|b| (b.name, b.crate_path))
                .collect(),
            links: m.link,
            output: m.output,
            base_dir: path
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."))
                .to_path_buf(),
        })
    } else {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("actor")
            .to_string();
        Ok(BuildInputs {
            members: vec![(name, ".".to_string())],
            links: vec![],
            output: None,
            base_dir: path.to_path_buf(),
        })
    }
}

/// `packr build` — normal `packr-guest` crate(s) → one theater-loadable composite.
///
/// The fixed-base recipe, **disjoint base assignment across members**, and the
/// compose step all become implementation details of the toolchain (versioned
/// with packr) instead of hand-copied config + address math that drift out of sync
/// with the linker across releases.
///
/// `packr build <crate-dir>` builds a single actor. `packr build <manifest.toml>`
/// builds + links a member set (an actor that links a library package), assigning
/// each member a disjoint memory region automatically — the fix for the
/// multi-member same-base collision (two members at one base silently corrupt each
/// other's static data and trap at runtime).
fn build_command(path: &std::path::Path, output: Option<PathBuf>) -> anyhow::Result<()> {
    use packr::{Layout, LinkBinary, LinkEdge};

    let BuildInputs {
        members,
        links: link_entries,
        output: manifest_output,
        base_dir,
    } = resolve_build_inputs(path)?;

    // Sequential base assignment: build member i at `base`, read its `__data_end`,
    // place member i+1 immediately above (aligned). Tight-packed, disjoint, no
    // author-visible addresses. Members start just above the 256 KiB shadow stack
    // (which they share — a single coherent stack; only static data must be
    // disjoint).
    const FIRST_BASE: u32 = 0x4_0000;
    const ALIGN: u32 = 0x1_0000;
    let mut base = FIRST_BASE;
    let mut binaries = vec![LinkBinary {
        alias: "alloc".into(),
        wasm: packr::DEFAULT_ALLOCATOR_WASM.to_vec(),
        allocator: true,
    }];
    let mut first_wasm_path: Option<PathBuf> = None;
    for (name, crate_rel) in &members {
        let crate_dir = base_dir.join(crate_rel);
        let (wasm, wasm_path) = build_member(&crate_dir, base)?;
        let data_end =
            packr::read_data_end(&wasm).map_err(|e| anyhow::anyhow!("member `{name}`: {e}"))?;
        anyhow::ensure!(
            data_end > base,
            "member `{name}` has an empty/invalid data region"
        );
        if first_wasm_path.is_none() {
            first_wasm_path = Some(wasm_path);
        }
        binaries.push(LinkBinary {
            alias: name.clone(),
            wasm,
            allocator: false,
        });
        base = data_end.next_multiple_of(ALIGN);
    }

    // Allocator + regenerated metadata + heap sit ABOVE every member's region.
    let metadata_base = base;
    let alloc_base = metadata_base + ALIGN;
    let heap_base = alloc_base + ALIGN;
    let heap_end = heap_base + 0x100_0000; // 16 MiB heap
    let layout = Layout {
        memory_pages: heap_end.div_ceil(65536) + 1,
        alloc_base,
        heap_base,
        heap_end,
        metadata_base,
    };

    // Edges: `<member>.<interface>` on each side (split on the first dot).
    let split = |s: &str, side: &str| -> anyhow::Result<(String, String)> {
        s.split_once('.')
            .map(|(a, i)| (a.to_string(), i.to_string()))
            .ok_or_else(|| anyhow::anyhow!("{side} `{s}` must be `<member>.<interface>`"))
    };
    let mut edges = Vec::new();
    for l in &link_entries {
        let (from_alias, from_interface) = split(&l.from, "link.from")?;
        let (to_alias, to_interface) = split(&l.to, "link.to")?;
        edges.push(LinkEdge {
            from_alias,
            from_interface,
            to_alias,
            to_interface,
        });
    }

    let composite = packr::link(binaries, &edges, layout)?;

    let out_path = output
        .or_else(|| manifest_output.map(PathBuf::from))
        .or_else(|| {
            first_wasm_path
                .as_ref()
                .map(|p| p.with_extension("composite.wasm"))
        })
        .unwrap_or_else(|| PathBuf::from("composite.wasm"));
    std::fs::write(&out_path, &composite)
        .map_err(|e| anyhow::anyhow!("writing {}: {e}", out_path.display()))?;
    println!(
        "built self-contained composite ({} member{}) → {} ({} bytes)",
        members.len(),
        if members.len() == 1 { "" } else { "s" },
        out_path.display(),
        composite.len()
    );
    Ok(())
}

/// cargo-build one member crate for `wasm32-unknown-unknown` at a fixed `base`,
/// returning its wasm bytes + the artifact path.
fn build_member(crate_dir: &std::path::Path, base: u32) -> anyhow::Result<(Vec<u8>, PathBuf)> {
    let rustflags = [
        "-C link-arg=--import-memory",
        "-C link-arg=--stack-first",
        "-C link-arg=-zstack-size=262144",
        &format!("-C link-arg=--global-base={base}"),
        "-C link-arg=--no-entry",
        // Load-bearing: keeps `__pack_types` its own CGRF-prefixed segment so
        // `read_surface` finds it regardless of `.rodata` size.
        "-C link-arg=--no-merge-data-segments",
    ]
    .join(" ");
    eprintln!(
        "packr build: cargo build {} @ base {base} (0x{base:x})…",
        crate_dir.display()
    );
    let out = std::process::Command::new("cargo")
        .current_dir(crate_dir)
        .args([
            "build",
            "--target",
            "wasm32-unknown-unknown",
            "--release",
            "--message-format=json-render-diagnostics",
        ])
        .env("RUSTFLAGS", &rustflags)
        .stderr(std::process::Stdio::inherit())
        .output()
        .map_err(|e| anyhow::anyhow!("failed to run `cargo` (is it on PATH?): {e}"))?;
    anyhow::ensure!(
        out.status.success(),
        "cargo build failed for {}",
        crate_dir.display()
    );
    let wasm_path = find_cdylib_wasm(&out.stdout).ok_or_else(|| {
        anyhow::anyhow!(
            "no cdylib `.wasm` from {} — needs `[lib] crate-type = [\"cdylib\"]` and the \
             `wasm32-unknown-unknown` target (`rustup target add wasm32-unknown-unknown`)",
            crate_dir.display()
        )
    })?;
    let wasm = std::fs::read(&wasm_path)
        .map_err(|e| anyhow::anyhow!("reading built wasm {}: {e}", wasm_path.display()))?;
    Ok((wasm, wasm_path))
}

/// Find the cdylib `.wasm` cargo emitted, from its `--message-format=json`
/// artifact stream (robust to `[lib]` name overrides, workspaces, target dirs).
fn find_cdylib_wasm(cargo_stdout: &[u8]) -> Option<PathBuf> {
    let text = String::from_utf8_lossy(cargo_stdout);
    let mut found = None;
    for line in text.lines() {
        let v: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if v.get("reason").and_then(|r| r.as_str()) != Some("compiler-artifact") {
            continue;
        }
        if let Some(files) = v.get("filenames").and_then(|f| f.as_array()) {
            for f in files.iter().filter_map(|f| f.as_str()) {
                if f.ends_with(".wasm") {
                    found = Some(PathBuf::from(f)); // last wins = the final cdylib
                }
            }
        }
    }
    found
}

/// A `pack link` manifest — explicit, hash-checked interface links.
///
/// ```toml
/// output = "user-actor-test.wasm"
///
/// [[binary]]
/// alias = "alloc"
/// allocator = true          # no `wasm` → packr's bundled default allocator
///
/// [[binary]]
/// alias = "mathreal"
/// wasm  = "math_real.wasm"
///
/// [[binary]]
/// alias = "adder"
/// wasm  = "adder.wasm"
///
/// [[link]]              # <from-alias>.<interface>  <-  <to-alias>.<interface>
/// from = "adder.math"
/// to   = "mathreal.math"
/// ```
#[derive(serde::Deserialize)]
struct LinkManifest {
    output: Option<String>,
    #[serde(default)]
    binary: Vec<LinkBinaryEntry>,
    #[serde(default)]
    link: Vec<LinkEntry>,
    layout: Option<ManifestLayout>,
}

#[derive(serde::Deserialize)]
struct LinkBinaryEntry {
    alias: String,
    /// Path to the package wasm. Optional ONLY for the allocator: an
    /// `allocator = true` entry with no `wasm` uses packr's bundled, crate-
    /// version-locked default allocator (`packr::DEFAULT_ALLOCATOR_WASM`).
    wasm: Option<String>,
    #[serde(default)]
    allocator: bool,
}

#[derive(serde::Deserialize)]
struct LinkEntry {
    from: String,
    to: String,
}

fn link_command(manifest_path: &PathBuf, output_override: Option<PathBuf>) -> anyhow::Result<()> {
    use packr::{Layout, LinkBinary, LinkEdge};

    let text = std::fs::read_to_string(manifest_path)
        .map_err(|e| anyhow::anyhow!("failed to read manifest {}: {e}", manifest_path.display()))?;
    let manifest: LinkManifest = toml::from_str(&text).map_err(|e| {
        anyhow::anyhow!("failed to parse manifest {}: {e}", manifest_path.display())
    })?;
    anyhow::ensure!(
        !manifest.binary.is_empty(),
        "manifest lists no [[binary]] entries"
    );

    let base_dir = manifest_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));

    let mut binaries = Vec::new();
    for b in &manifest.binary {
        let wasm = match &b.wasm {
            Some(p) => {
                let wpath = base_dir.join(p);
                std::fs::read(&wpath).map_err(|e| {
                    anyhow::anyhow!("failed to read binary {}: {e}", wpath.display())
                })?
            }
            // The allocator may omit `wasm` to use packr's bundled default
            // (version-locked to the crate — no vendored blob to skew).
            None if b.allocator => packr::DEFAULT_ALLOCATOR_WASM.to_vec(),
            None => anyhow::bail!(
                "binary `{}` has no `wasm` path (only an `allocator = true` entry may omit it)",
                b.alias
            ),
        };
        binaries.push(LinkBinary {
            alias: b.alias.clone(),
            wasm,
            allocator: b.allocator,
        });
    }

    // `<alias>.<interface>` — split on the first dot (aliases are simple idents;
    // interface paths like `theater:simple/runtime` may follow).
    let split = |s: &str, side: &str| -> anyhow::Result<(String, String)> {
        s.split_once('.')
            .map(|(a, i)| (a.to_string(), i.to_string()))
            .ok_or_else(|| anyhow::anyhow!("{side} `{s}` must be `<alias>.<interface>`"))
    };
    let mut links = Vec::new();
    for l in &manifest.link {
        let (from_alias, from_interface) = split(&l.from, "link.from")?;
        let (to_alias, to_interface) = split(&l.to, "link.to")?;
        links.push(LinkEdge {
            from_alias,
            from_interface,
            to_alias,
            to_interface,
        });
    }

    let mut layout = Layout::default();
    if let Some(cfg) = &manifest.layout {
        if let Some(v) = cfg.memory_pages {
            layout.memory_pages = v;
        }
        if let Some(v) = cfg.alloc_base {
            layout.alloc_base = v;
        }
        if let Some(v) = cfg.heap_base {
            layout.heap_base = v;
        }
        if let Some(v) = cfg.heap_end {
            layout.heap_end = v;
        }
    }

    // Validate every link (hash-checked), fuse, and regenerate the composite's
    // `__pack_types` surface.
    let linked = packr::link(binaries, &links, layout)?;

    let out = output_override
        .or_else(|| manifest.output.as_ref().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("linked.wasm"));
    std::fs::write(&out, &linked)
        .map_err(|e| anyhow::anyhow!("failed to write {}: {e}", out.display()))?;

    println!(
        "linked {} binaries with {} verified link(s) -> {} ({} bytes)",
        manifest.binary.len(),
        manifest.link.len(),
        out.display(),
        linked.len()
    );
    Ok(())
}

/// A `pack compose` manifest.
///
/// ```toml
/// output = "composed.wasm"
///
/// [[package]]            # providers before consumers
/// name = "pack:alloc"    # the wasm-merge module name = the import module-name
/// wasm = "pack_alloc.wasm"
///
/// [[package]]
/// name = "math"          # adder does (import "math" "double")
/// wasm = "doubler.wasm"
///
/// [[package]]
/// name = "adder"
/// wasm = "adder.wasm"
///
/// [layout]               # optional; hex ok
/// memory_pages = 128
/// alloc_base = 0xE0000
/// heap_base  = 0xF0000
/// heap_end   = 0x800000
/// ```
#[derive(serde::Deserialize)]
struct Manifest {
    output: Option<String>,
    #[serde(default)]
    package: Vec<ManifestPackage>,
    layout: Option<ManifestLayout>,
}

#[derive(serde::Deserialize)]
struct ManifestPackage {
    name: String,
    wasm: String,
}

#[derive(serde::Deserialize)]
struct ManifestLayout {
    memory_pages: Option<u32>,
    alloc_base: Option<u32>,
    heap_base: Option<u32>,
    heap_end: Option<u32>,
}

fn compose_command(
    manifest_path: &PathBuf,
    output_override: Option<PathBuf>,
) -> anyhow::Result<()> {
    use packr::{compose, ComposeSpec, Layout, PackageSpec};

    let text = std::fs::read_to_string(manifest_path)
        .map_err(|e| anyhow::anyhow!("failed to read manifest {}: {e}", manifest_path.display()))?;
    let manifest: Manifest = toml::from_str(&text).map_err(|e| {
        anyhow::anyhow!("failed to parse manifest {}: {e}", manifest_path.display())
    })?;
    anyhow::ensure!(
        !manifest.package.is_empty(),
        "manifest lists no [[package]] entries"
    );

    // Paths in the manifest are relative to the manifest's directory.
    let base_dir = manifest_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));

    let mut packages = Vec::new();
    for p in &manifest.package {
        let wpath = base_dir.join(&p.wasm);
        let bytes = std::fs::read(&wpath)
            .map_err(|e| anyhow::anyhow!("failed to read package {}: {e}", wpath.display()))?;
        packages.push(PackageSpec::new(p.name.clone(), bytes));
    }

    let mut layout = Layout::default();
    if let Some(l) = &manifest.layout {
        if let Some(v) = l.memory_pages {
            layout.memory_pages = v;
        }
        if let Some(v) = l.alloc_base {
            layout.alloc_base = v;
        }
        if let Some(v) = l.heap_base {
            layout.heap_base = v;
        }
        if let Some(v) = l.heap_end {
            layout.heap_end = v;
        }
    }

    let composed = compose(&ComposeSpec { packages, layout })?;

    let out = output_override
        .or_else(|| manifest.output.as_ref().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("composed.wasm"));
    std::fs::write(&out, &composed)
        .map_err(|e| anyhow::anyhow!("failed to write {}: {e}", out.display()))?;

    println!(
        "composed {} package(s) -> {} ({} bytes, zero imports)",
        manifest.package.len(),
        out.display(),
        composed.len()
    );
    Ok(())
}

fn inspect_command(wasm_file: &PathBuf, show_hashes: bool, json: bool) -> anyhow::Result<()> {
    // Read the WASM file
    let wasm_bytes = std::fs::read(wasm_file)
        .map_err(|e| anyhow::anyhow!("Failed to read {}: {}", wasm_file.display(), e))?;

    // Parse the WASM binary to extract data segments (no instantiation needed)
    let parsed = ParsedModule::parse("module", &wasm_bytes)
        .map_err(|e| anyhow::anyhow!("Failed to parse WASM: {}", e))?;

    // Find the data segment containing CGRF metadata
    let metadata_bytes = find_cgrf_metadata(&parsed)
        .ok_or_else(|| anyhow::anyhow!("No Pack metadata found in WASM file"))?;

    // Decode the metadata
    let metadata = decode_metadata_with_hashes(&metadata_bytes)
        .map_err(|e| anyhow::anyhow!("Failed to decode metadata: {}", e))?;

    if json {
        if show_hashes {
            print_json_with_hashes(
                &metadata.arena,
                &metadata.import_hashes,
                &metadata.export_hashes,
            )?;
        } else {
            print_json(&metadata.arena)?;
        }
    } else if show_hashes {
        print_metadata_with_hashes(
            &metadata.arena,
            &metadata.import_hashes,
            &metadata.export_hashes,
        );
    } else {
        print_metadata(&metadata.arena);
    }

    Ok(())
}

/// Find CGRF metadata in the WASM data segments
fn find_cgrf_metadata(parsed: &ParsedModule) -> Option<Vec<u8>> {
    for segment in &parsed.data {
        // Check if this segment starts with CGRF magic
        if segment.data.len() >= 4 && segment.data[0..4] == CGRF_MAGIC {
            return Some(segment.data.clone());
        }
    }
    None
}

fn print_metadata(arena: &Arena) {
    // Print imports
    let imports = arena.imports();
    if !imports.is_empty() {
        println!("imports:");
        print_functions(&imports, "  ");
    }

    // Print exports
    let exports = arena.exports();
    if !exports.is_empty() {
        println!("exports:");
        print_functions(&exports, "  ");
    }
}

fn print_metadata_with_hashes(
    arena: &Arena,
    import_hashes: &[packr::InterfaceHash],
    export_hashes: &[packr::InterfaceHash],
) {
    // Print imports with hashes
    let imports = arena.imports();
    if !imports.is_empty() {
        println!("imports:");
        print_functions(&imports, "  ");

        if !import_hashes.is_empty() {
            println!("  interface-hashes:");
            for hash in import_hashes {
                println!("    {}: {}", hash.name, hash.hash);
            }
        }
    }

    // Print exports with hashes
    let exports = arena.exports();
    if !exports.is_empty() {
        println!("exports:");
        print_functions(&exports, "  ");

        if !export_hashes.is_empty() {
            println!("  interface-hashes:");
            for hash in export_hashes {
                println!("    {}: {}", hash.name, hash.hash);
            }
        }
    }
}

fn print_functions(functions: &[Function], indent: &str) {
    // Group functions by interface
    let mut by_interface: std::collections::BTreeMap<&str, Vec<&Function>> =
        std::collections::BTreeMap::new();

    for func in functions {
        by_interface.entry(&func.interface).or_default().push(func);
    }

    for (interface, funcs) in by_interface {
        for func in funcs {
            let params = format_params(&func.params);
            let results = format_results(&func.results);
            println!(
                "{}{}.{}: ({}) -> {}",
                indent, interface, func.name, params, results
            );
        }
    }
}

fn format_params(params: &[Param]) -> String {
    params
        .iter()
        .map(|p| format!("{}: {}", p.name, format_type(&p.ty)))
        .collect::<Vec<_>>()
        .join(", ")
}

fn format_results(results: &[Type]) -> String {
    if results.is_empty() {
        "unit".to_string()
    } else if results.len() == 1 {
        format_type(&results[0])
    } else {
        format!(
            "({})",
            results
                .iter()
                .map(format_type)
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

fn format_type(ty: &Type) -> String {
    match ty {
        Type::Unit => "unit".to_string(),
        Type::Bool => "bool".to_string(),
        Type::U8 => "u8".to_string(),
        Type::U16 => "u16".to_string(),
        Type::U32 => "u32".to_string(),
        Type::U64 => "u64".to_string(),
        Type::S8 => "s8".to_string(),
        Type::S16 => "s16".to_string(),
        Type::S32 => "s32".to_string(),
        Type::S64 => "s64".to_string(),
        Type::F32 => "f32".to_string(),
        Type::F64 => "f64".to_string(),
        Type::Char => "char".to_string(),
        Type::String => "string".to_string(),
        Type::List(inner) => format!("list<{}>", format_type(inner)),
        Type::Option(inner) => format!("option<{}>", format_type(inner)),
        Type::Result { ok, err } => {
            format!("result<{}, {}>", format_type(ok), format_type(err))
        }
        Type::Tuple(types) => {
            if types.is_empty() {
                "unit".to_string()
            } else {
                format!(
                    "tuple<{}>",
                    types.iter().map(format_type).collect::<Vec<_>>().join(", ")
                )
            }
        }
        Type::Ref(path) => path.to_string(),
        Type::Value => "value".to_string(),
    }
}

fn print_json(arena: &Arena) -> anyhow::Result<()> {
    let output = serde_json::json!({
        "imports": arena.imports().iter().map(func_to_json).collect::<Vec<_>>(),
        "exports": arena.exports().iter().map(func_to_json).collect::<Vec<_>>(),
    });
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

fn print_json_with_hashes(
    arena: &Arena,
    import_hashes: &[packr::InterfaceHash],
    export_hashes: &[packr::InterfaceHash],
) -> anyhow::Result<()> {
    let output = serde_json::json!({
        "imports": arena.imports().iter().map(func_to_json).collect::<Vec<_>>(),
        "exports": arena.exports().iter().map(func_to_json).collect::<Vec<_>>(),
        "import_hashes": import_hashes.iter().map(|h| serde_json::json!({
            "interface": h.name,
            "hash": h.hash.to_string(),
        })).collect::<Vec<_>>(),
        "export_hashes": export_hashes.iter().map(|h| serde_json::json!({
            "interface": h.name,
            "hash": h.hash.to_string(),
        })).collect::<Vec<_>>(),
    });
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

fn func_to_json(func: &Function) -> serde_json::Value {
    serde_json::json!({
        "interface": func.interface,
        "name": func.name,
        "params": func.params.iter().map(|p| serde_json::json!({
            "name": p.name,
            "type": format_type(&p.ty),
        })).collect::<Vec<_>>(),
        "results": func.results.iter().map(format_type).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::find_cdylib_wasm;

    #[test]
    fn finds_the_final_cdylib_wasm_in_cargo_json() {
        // A realistic slice of `cargo build --message-format=json` output: a dep
        // artifact (rlib), then the actor's cdylib (.wasm), then build-finished.
        let stream = concat!(
            r#"{"reason":"compiler-artifact","target":{"kind":["lib"],"name":"serde"},"filenames":["/t/libserde.rlib"]}"#,
            "\n",
            r#"{"reason":"compiler-artifact","target":{"kind":["cdylib"],"name":"mesh"},"filenames":["/t/wasm32-unknown-unknown/release/mesh.wasm"]}"#,
            "\n",
            r#"{"reason":"build-finished","success":true}"#,
            "\n",
        );
        let found = find_cdylib_wasm(stream.as_bytes()).expect("should find the cdylib wasm");
        assert_eq!(
            found,
            std::path::PathBuf::from("/t/wasm32-unknown-unknown/release/mesh.wasm")
        );
    }

    #[test]
    fn returns_none_when_no_wasm_artifact() {
        let stream = concat!(
            r#"{"reason":"compiler-artifact","target":{"kind":["lib"],"name":"serde"},"filenames":["/t/libserde.rlib"]}"#,
            "\n",
            r#"{"reason":"build-finished","success":true}"#,
        );
        assert!(find_cdylib_wasm(stream.as_bytes()).is_none());
    }
}
