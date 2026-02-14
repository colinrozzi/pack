//! Pack CLI - tools for working with Pack packages
//!
//! Commands:
//!   pack inspect <wasm>  - Display metadata from a WASM package

use clap::{Parser, Subcommand};
use pack::{decode_metadata_with_hashes, Arena, Function, Param, ParsedModule, Type};
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
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Inspect {
            wasm_file,
            hashes,
            json,
        } => inspect_command(&wasm_file, hashes, json),
    }
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
    } else {
        if show_hashes {
            print_metadata_with_hashes(
                &metadata.arena,
                &metadata.import_hashes,
                &metadata.export_hashes,
            );
        } else {
            print_metadata(&metadata.arena);
        }
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
    import_hashes: &[pack::InterfaceHash],
    export_hashes: &[pack::InterfaceHash],
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
        by_interface
            .entry(&func.interface)
            .or_default()
            .push(func);
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
            results.iter().map(format_type).collect::<Vec<_>>().join(", ")
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
    import_hashes: &[pack::InterfaceHash],
    export_hashes: &[pack::InterfaceHash],
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
