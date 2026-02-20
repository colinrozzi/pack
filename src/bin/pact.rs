//! Pact CLI - tools for working with Pact interface definitions
//!
//! Commands:
//!   pact check <file.pact>    - Parse and validate a pact file
//!   pact check-dir <dir>      - Parse all pact files and validate cross-file references
//!   pact codegen <file>       - Generate Rust code from a pact file or directory

use clap::{Parser, Subcommand};
use pack::{
    parse_pact_file, parse_pact_dir_with_registry, PactInterface, TypeRegistry,
    codegen, TypeDef, Type,
};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "pact")]
#[command(about = "Tools for working with Pact interface definitions", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Parse and validate a single pact file
    Check {
        /// Path to the .pact file
        file: PathBuf,
    },

    /// Parse all pact files in a directory and validate cross-file references
    CheckDir {
        /// Path to the directory
        dir: PathBuf,
    },

    /// Generate Rust code from pact file(s)
    Codegen {
        /// Path to a .pact file or directory
        path: PathBuf,

        /// Output file (defaults to stdout)
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Check { file } => check_command(&file),
        Commands::CheckDir { dir } => check_dir_command(&dir),
        Commands::Codegen { path, output } => codegen_command(&path, output.as_deref()),
    }
}

fn check_command(file: &PathBuf) -> anyhow::Result<()> {
    let interface = parse_pact_file(file)
        .map_err(|e| anyhow::anyhow!("{}", e))?;

    print_interface_summary(&interface, 0);
    println!("\n✓ {} parsed successfully", file.display());
    Ok(())
}

fn check_dir_command(dir: &PathBuf) -> anyhow::Result<()> {
    let (root, registry) = parse_pact_dir_with_registry(dir)
        .map_err(|e| anyhow::anyhow!("{}", e))?;

    println!("Parsed {} interfaces:", root.children.len());
    for child in &root.children {
        print_interface_summary(child, 1);
    }

    // Validate all use declarations
    println!("\nValidating cross-file references...");
    let mut errors = Vec::new();

    for child in &root.children {
        validate_uses(child, &registry, &mut errors);
    }

    if errors.is_empty() {
        println!("✓ All references valid");
    } else {
        println!("\nErrors:");
        for error in &errors {
            println!("  ✗ {}", error);
        }
        return Err(anyhow::anyhow!("{} validation error(s)", errors.len()));
    }

    Ok(())
}

fn validate_uses(interface: &PactInterface, registry: &TypeRegistry, errors: &mut Vec<String>) {
    for use_decl in &interface.uses {
        match registry.resolve_use(use_decl) {
            Ok(types) => {
                if use_decl.items.is_empty() {
                    println!("  ✓ {}: use {} (all {} types)",
                        interface.name, use_decl.interface, types.len());
                } else {
                    println!("  ✓ {}: use {}.{{{}}}",
                        interface.name, use_decl.interface,
                        use_decl.items.join(", "));
                }
            }
            Err(e) => {
                errors.push(format!("{}: {}", interface.name, e));
            }
        }
    }

    // Recurse into children
    for child in &interface.children {
        validate_uses(child, registry, errors);
    }
}

fn codegen_command(path: &PathBuf, output: Option<&std::path::Path>) -> anyhow::Result<()> {
    let interface = if path.is_dir() {
        let (root, _registry) = parse_pact_dir_with_registry(path)
            .map_err(|e| anyhow::anyhow!("{}", e))?;
        root
    } else {
        parse_pact_file(path)
            .map_err(|e| anyhow::anyhow!("{}", e))?
    };

    let code = codegen::generate_rust(&interface);

    match output {
        Some(out_path) => {
            std::fs::write(out_path, &code)?;
            println!("Generated code written to {}", out_path.display());
        }
        None => {
            println!("{}", code);
        }
    }

    Ok(())
}

fn print_interface_summary(interface: &PactInterface, indent: usize) {
    let prefix = "  ".repeat(indent);

    println!("{}interface {} {{", prefix, interface.name);

    // Print metadata
    for meta in &interface.metadata {
        println!("{}  @{}: {:?}", prefix, meta.name, meta.value);
    }

    // Print uses
    for use_decl in &interface.uses {
        if use_decl.items.is_empty() {
            println!("{}  use {}", prefix, use_decl.interface);
        } else {
            println!("{}  use {}.{{{}}}", prefix, use_decl.interface, use_decl.items.join(", "));
        }
    }

    // Print type params
    for tp in &interface.type_params {
        if let Some(ref constraint) = tp.constraint {
            println!("{}  type {}: {}", prefix, tp.name, constraint);
        } else {
            println!("{}  type {}", prefix, tp.name);
        }
    }

    // Print types
    for typedef in &interface.types {
        println!("{}  {}", prefix, format_typedef(typedef));
    }

    // Print exports count
    if !interface.exports.is_empty() {
        println!("{}  exports: {} item(s)", prefix, interface.exports.len());
    }

    // Print imports count
    if !interface.imports.is_empty() {
        println!("{}  imports: {} item(s)", prefix, interface.imports.len());
    }

    // Print children
    for child in &interface.children {
        print_interface_summary(child, indent + 1);
    }

    println!("{}}}", prefix);
}

fn format_typedef(typedef: &TypeDef) -> String {
    match typedef {
        TypeDef::Alias { name, ty } => format!("type {} = {}", name, format_type(ty)),
        TypeDef::Record { name, fields } => {
            format!("record {} {{ {} fields }}", name, fields.len())
        }
        TypeDef::Variant { name, cases } => {
            format!("variant {} {{ {} cases }}", name, cases.len())
        }
        TypeDef::Enum { name, cases } => {
            format!("enum {} {{ {} cases }}", name, cases.len())
        }
        TypeDef::Flags { name, flags } => {
            format!("flags {} {{ {} flags }}", name, flags.len())
        }
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
        Type::Result { ok, err } => format!("result<{}, {}>", format_type(ok), format_type(err)),
        Type::Tuple(types) => {
            format!("tuple<{}>", types.iter().map(|t| format_type(t)).collect::<Vec<_>>().join(", "))
        }
        Type::Ref(path) => path.to_string(),
        Type::Value => "value".to_string(),
    }
}
