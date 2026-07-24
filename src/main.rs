//! Pack CLI - tools for working with Pack packages
//!
//! Commands:
//!   packr inspect <wasm>       - Display a package's metadata

use clap::{Parser, Subcommand};
use packr::compose::{compose, Component, GraphLink};
use packr::{decode_metadata_with_hashes, Arena, Function, Param, Type};
use serde::Deserialize;
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

    /// Compose N components + a link graph into one multi-memory composite wasm
    Compose {
        /// Path to the compose manifest (TOML)
        manifest: PathBuf,

        /// Output path for the composite wasm
        #[arg(long, short = 'o')]
        output: PathBuf,
    },
}

/// The compose manifest: `[[component]]` entries + `[[link]]` entries.
#[derive(Debug, Deserialize)]
struct ComposeManifest {
    #[serde(default, rename = "component")]
    components: Vec<ManifestComponent>,
    #[serde(default, rename = "link")]
    links: Vec<ManifestLink>,
}

#[derive(Debug, Deserialize)]
struct ManifestComponent {
    name: String,
    /// Path to a prebuilt `.wasm`, relative to the manifest file.
    wasm: String,
    #[serde(default)]
    entry: bool,
}

#[derive(Debug, Deserialize)]
struct ManifestLink {
    /// Name of the consumer component.
    consumer: String,
    /// The consumer's import, formatted `"module.name"`.
    import: String,
    /// Name of the provider component.
    provider: String,
    /// The provider's export name.
    export: String,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Inspect {
            wasm_file,
            hashes,
            json,
        } => inspect_command(&wasm_file, hashes, json),
        Commands::Compose { manifest, output } => compose_command(&manifest, &output),
    }
}

/// `packr compose <manifest> -o <out>`: parse the manifest, read each
/// component's wasm (relative to the manifest), compose, and write the composite.
fn compose_command(manifest_path: &PathBuf, output: &PathBuf) -> anyhow::Result<()> {
    let text = std::fs::read_to_string(manifest_path).map_err(|e| {
        anyhow::anyhow!("Failed to read manifest {}: {}", manifest_path.display(), e)
    })?;
    let manifest: ComposeManifest =
        toml::from_str(&text).map_err(|e| anyhow::anyhow!("Failed to parse manifest: {}", e))?;

    // Component wasm paths are relative to the manifest's directory.
    let base = manifest_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));

    let mut components = Vec::with_capacity(manifest.components.len());
    for c in &manifest.components {
        let wasm_path = base.join(&c.wasm);
        let wasm = std::fs::read(&wasm_path).map_err(|e| {
            anyhow::anyhow!(
                "Failed to read component `{}` wasm {}: {}",
                c.name,
                wasm_path.display(),
                e
            )
        })?;
        components.push(Component {
            name: c.name.clone(),
            wasm,
            entry: c.entry,
        });
    }

    let mut links = Vec::with_capacity(manifest.links.len());
    for l in &manifest.links {
        let (import_module, import_name) = l.import.split_once('.').ok_or_else(|| {
            anyhow::anyhow!("link import `{}` must be formatted `module.name`", l.import)
        })?;
        links.push(GraphLink {
            consumer: l.consumer.clone(),
            import_module: import_module.to_string(),
            import_name: import_name.to_string(),
            provider: l.provider.clone(),
            export_name: l.export.clone(),
        });
    }

    let composite =
        compose(components, &links).map_err(|e| anyhow::anyhow!("Composition failed: {:#}", e))?;

    std::fs::write(output, &composite)
        .map_err(|e| anyhow::anyhow!("Failed to write {}: {}", output.display(), e))?;

    println!(
        "Composed {} component(s), {} link(s) -> {}",
        manifest.components.len(),
        manifest.links.len(),
        output.display()
    );
    Ok(())
}

fn inspect_command(wasm_file: &PathBuf, show_hashes: bool, json: bool) -> anyhow::Result<()> {
    // Read the WASM file
    let wasm_bytes = std::fs::read(wasm_file)
        .map_err(|e| anyhow::anyhow!("Failed to read {}: {}", wasm_file.display(), e))?;

    // Scan the WASM binary's data segments for the CGRF metadata (no
    // instantiation needed).
    let metadata_bytes = find_cgrf_metadata(&wasm_bytes)
        .map_err(|e| anyhow::anyhow!("Failed to parse WASM: {}", e))?
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

/// Find CGRF metadata in the WASM data segments.
///
/// Scans the module's data section for the first segment whose bytes begin with
/// the `CGRF` magic — packr emits the metadata as its own data segment.
fn find_cgrf_metadata(wasm: &[u8]) -> Result<Option<Vec<u8>>, wasmparser::BinaryReaderError> {
    use wasmparser::{Parser, Payload};

    for payload in Parser::new(0).parse_all(wasm) {
        if let Payload::DataSection(reader) = payload? {
            for data in reader {
                let bytes = data?.data;
                if bytes.len() >= 4 && bytes[0..4] == CGRF_MAGIC {
                    return Ok(Some(bytes.to_vec()));
                }
            }
        }
    }
    Ok(None)
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
