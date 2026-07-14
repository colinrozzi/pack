//! Pack CLI - tools for working with Pack packages
//!
//! Commands:
//!   pack inspect <wasm>  - Display metadata from a WASM package

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
    }
}

/// A `pack link` manifest — explicit, hash-checked interface links.
///
/// ```toml
/// output = "user-actor-test.wasm"
///
/// [[binary]]
/// alias = "alloc"
/// wasm  = "pack_alloc.wasm"
/// allocator = true
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
    wasm: String,
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
        let wpath = base_dir.join(&b.wasm);
        let wasm = std::fs::read(&wpath)
            .map_err(|e| anyhow::anyhow!("failed to read binary {}: {e}", wpath.display()))?;
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
