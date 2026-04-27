use pack::{generate_rust, parse_pact_dir_with_registry, PactExport, TransformRegistry};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (root, type_registry) = parse_pact_dir_with_registry("examples/transforms")?;
    let transform_registry = TransformRegistry::with_builtins();

    println!("=== Parsed Interfaces ===\n");
    for child in &root.children {
        println!("Interface: {}", child.name);
        println!(
            "  Uses: {:?}",
            child
                .uses
                .iter()
                .map(|u| {
                    if u.transform_args.is_empty() {
                        u.interface.clone()
                    } else {
                        format!("{}({:?})", u.interface, u.transform_args)
                    }
                })
                .collect::<Vec<_>>()
        );
        println!(
            "  Aliases: {:?}",
            child
                .aliases
                .iter()
                .map(|a| { format!("{} = {}({:?})", a.name, a.transform, a.args) })
                .collect::<Vec<_>>()
        );
        println!();
    }

    println!("=== Original Calculator Interface ===\n");
    let calc = type_registry.get_interface("calculator").unwrap();
    for export in &calc.exports {
        if let PactExport::Function(f) = export {
            println!("  {}: {:?} -> {:?}", f.name, f.params, f.results);
        }
    }
    println!();

    println!("=== Transformed rpc(calculator) ===\n");
    let rpc_calc =
        type_registry.get_transformed_interface("rpc", "calculator", &transform_registry)?;

    println!("Name: {}", rpc_calc.name);
    println!("Types added:");
    for ty in &rpc_calc.types {
        println!("  - {}", ty.name());
    }
    println!("\nExports (wrapped):");
    for export in &rpc_calc.exports {
        if let PactExport::Function(f) = export {
            println!("  {}: {:?} -> {:?}", f.name, f.params, f.results);
        }
    }
    println!();

    println!("=== Generated Rust for rpc(calculator) ===\n");
    let rust_code = generate_rust(&rpc_calc);
    println!("{}", rust_code);

    println!("=== Resolved Scope for 'caller' ===\n");
    let caller = type_registry.get_interface("caller").unwrap();
    let scope = type_registry.resolve_scope_with_transforms(caller, &transform_registry)?;

    println!("Types in scope:");
    for (name, _) in &scope.types {
        println!("  - {}", name);
    }
    println!("\nTransformed interfaces:");
    for iface in &scope.transformed_interfaces {
        println!("  - {}", iface.name);
    }

    Ok(())
}
