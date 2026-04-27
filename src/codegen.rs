//! Code generation from Pact interfaces
//!
//! Generates Rust types and traits from Pact interface definitions.

use crate::parser::{PactExport, PactImport, PactInterface};
use crate::types::{Case, Field, Type, TypeDef};

/// Generate Rust code from a Pact interface.
pub fn generate_rust(interface: &PactInterface) -> String {
    let mut output = String::new();

    // Header comment
    output.push_str(&format!("// Generated from {}.pact\n", interface.name));
    output.push_str("// DO NOT EDIT - changes will be overwritten\n\n");

    // Version comment if available
    if let Some(version) = interface.version() {
        output.push_str(&format!("// Interface version: {}\n\n", version));
    }

    // Generate type definitions
    for typedef in &interface.types {
        output.push_str(&generate_typedef(typedef));
        output.push('\n');
    }

    // Generate import traits (what the actor can call)
    for import in &interface.imports {
        if let PactImport::Interface(name) = import {
            output.push_str(&generate_import_trait(name));
            output.push('\n');
        }
    }

    // Generate export trait (what the actor must implement)
    if !interface.exports.is_empty() {
        output.push_str(&generate_export_trait(interface));
    }

    // Generate nested interfaces
    for child in &interface.children {
        output.push_str(&format!("pub mod {} {{\n", to_snake_case(&child.name)));
        let child_code = generate_rust(child);
        for line in child_code.lines() {
            output.push_str("    ");
            output.push_str(line);
            output.push('\n');
        }
        output.push_str("}\n\n");
    }

    output
}

fn generate_typedef(typedef: &TypeDef) -> String {
    match typedef {
        TypeDef::Record { name, fields } => generate_record(name, fields),
        TypeDef::Variant { name, cases } => generate_variant(name, cases),
        TypeDef::Enum { name, cases } => generate_enum(name, cases),
        TypeDef::Flags { name, flags } => generate_flags(name, flags),
        TypeDef::Alias { name, ty } => generate_alias(name, ty),
    }
}

fn generate_record(name: &str, fields: &[Field]) -> String {
    let mut out = String::new();
    let rust_name = to_pascal_case(name);

    out.push_str("#[derive(Debug, Clone, PartialEq)]\n");
    out.push_str(&format!("pub struct {} {{\n", rust_name));

    for field in fields {
        out.push_str(&format!(
            "    pub {}: {},\n",
            to_snake_case(&field.name),
            type_to_rust(&field.ty)
        ));
    }

    out.push_str("}\n");
    out
}

fn generate_variant(name: &str, cases: &[Case]) -> String {
    let mut out = String::new();
    let rust_name = to_pascal_case(name);

    out.push_str("#[derive(Debug, Clone, PartialEq)]\n");
    out.push_str(&format!("pub enum {} {{\n", rust_name));

    for case in cases {
        let case_name = to_pascal_case(&case.name);
        if case.payload == Type::Unit {
            out.push_str(&format!("    {},\n", case_name));
        } else {
            out.push_str(&format!(
                "    {}({}),\n",
                case_name,
                type_to_rust(&case.payload)
            ));
        }
    }

    out.push_str("}\n");
    out
}

fn generate_enum(name: &str, cases: &[String]) -> String {
    let mut out = String::new();
    let rust_name = to_pascal_case(name);

    out.push_str("#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]\n");
    out.push_str(&format!("pub enum {} {{\n", rust_name));

    for case in cases {
        out.push_str(&format!("    {},\n", to_pascal_case(case)));
    }

    out.push_str("}\n");
    out
}

fn generate_flags(name: &str, flags: &[String]) -> String {
    let mut out = String::new();
    let rust_name = to_pascal_case(name);

    // Use bitflags-style struct
    out.push_str("#[derive(Debug, Clone, Copy, PartialEq, Eq)]\n");
    out.push_str(&format!("pub struct {}(u32);\n\n", rust_name));

    out.push_str(&format!("impl {} {{\n", rust_name));
    for (i, flag) in flags.iter().enumerate() {
        out.push_str(&format!(
            "    pub const {}: Self = Self(1 << {});\n",
            to_screaming_snake_case(flag),
            i
        ));
    }
    out.push('\n');
    out.push_str("    pub fn contains(self, other: Self) -> bool {\n");
    out.push_str("        (self.0 & other.0) == other.0\n");
    out.push_str("    }\n");
    out.push('\n');
    out.push_str("    pub fn insert(&mut self, other: Self) {\n");
    out.push_str("        self.0 |= other.0;\n");
    out.push_str("    }\n");
    out.push_str("}\n");

    out
}

fn generate_alias(name: &str, target: &Type) -> String {
    format!(
        "pub type {} = {};\n",
        to_pascal_case(name),
        type_to_rust(target)
    )
}

fn generate_import_trait(name: &str) -> String {
    let trait_name = to_pascal_case(name);
    format!(
        "/// Import: {} - functions this actor can call\n\
         pub trait {} {{\n\
         }}\n",
        name, trait_name
    )
}

fn generate_export_trait(interface: &PactInterface) -> String {
    let mut out = String::new();
    let trait_name = to_pascal_case(&interface.name);

    // Build generic parameters from type_params
    let generics = if interface.type_params.is_empty() {
        String::new()
    } else {
        let params: Vec<String> = interface
            .type_params
            .iter()
            .map(|tp| tp.name.clone())
            .collect();
        format!("<{}>", params.join(", "))
    };

    out.push_str(&format!("/// Exports for {} interface\n", interface.name));
    out.push_str(&format!("pub trait {}{} {{\n", trait_name, generics));

    for export in &interface.exports {
        if let PactExport::Function(func) = export {
            // Build function signature
            let fn_name = to_snake_case(&func.name);
            let params: Vec<String> = func
                .params
                .iter()
                .map(|p| format!("{}: {}", to_snake_case(&p.name), type_to_rust(&p.ty)))
                .collect();

            let params_str = if params.is_empty() {
                "&self".to_string()
            } else {
                format!("&self, {}", params.join(", "))
            };

            let return_type = if func.results.is_empty() {
                String::new()
            } else if func.results.len() == 1 {
                format!(" -> {}", type_to_rust(&func.results[0]))
            } else {
                let types: Vec<String> = func.results.iter().map(type_to_rust).collect();
                format!(" -> ({})", types.join(", "))
            };

            out.push_str(&format!(
                "    fn {}({}){};\n",
                fn_name, params_str, return_type
            ));
        }
    }

    out.push_str("}\n");
    out
}

fn type_to_rust(ty: &Type) -> String {
    match ty {
        Type::Bool => "bool".to_string(),
        Type::U8 => "u8".to_string(),
        Type::U16 => "u16".to_string(),
        Type::U32 => "u32".to_string(),
        Type::U64 => "u64".to_string(),
        Type::S8 => "i8".to_string(),
        Type::S16 => "i16".to_string(),
        Type::S32 => "i32".to_string(),
        Type::S64 => "i64".to_string(),
        Type::F32 => "f32".to_string(),
        Type::F64 => "f64".to_string(),
        Type::Char => "char".to_string(),
        Type::String => "String".to_string(),
        Type::Unit => "()".to_string(),
        Type::List(inner) => format!("Vec<{}>", type_to_rust(inner)),
        Type::Option(inner) => format!("Option<{}>", type_to_rust(inner)),
        Type::Result { ok, err } => {
            format!("Result<{}, {}>", type_to_rust(ok), type_to_rust(err))
        }
        Type::Tuple(items) => {
            let types: Vec<String> = items.iter().map(type_to_rust).collect();
            format!("({})", types.join(", "))
        }
        Type::Ref(path) => {
            // Named type reference
            to_pascal_case(&path.segments.join("::"))
        }
        Type::Value => "serde_json::Value".to_string(),
    }
}

// ============================================================================
// Name conversion utilities
// ============================================================================

fn to_pascal_case(s: &str) -> String {
    s.split(['-', '_', '(', ')', '<', '>', ','])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => first.to_uppercase().chain(chars).collect(),
            }
        })
        .collect()
}

fn to_snake_case(s: &str) -> String {
    let mut result = String::new();
    let mut prev_lower = false;

    for c in s.chars() {
        if c == '-' || c == '(' || c == ')' || c == '<' || c == '>' || c == ',' {
            if !result.is_empty() && !result.ends_with('_') {
                result.push('_');
            }
            prev_lower = false;
        } else if c.is_uppercase() {
            if prev_lower {
                result.push('_');
            }
            result.push(c.to_lowercase().next().unwrap());
            prev_lower = false;
        } else {
            result.push(c);
            prev_lower = c.is_lowercase();
        }
    }

    // Trim trailing underscore
    if result.ends_with('_') {
        result.pop();
    }

    result
}

fn to_screaming_snake_case(s: &str) -> String {
    to_snake_case(s).to_uppercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_pact;

    #[test]
    fn test_generate_calculator() {
        let src = r#"
            interface calculator {
                @version: string = "1.0.0"

                record point {
                    x: f32,
                    y: f32,
                }

                imports {
                    logger
                }

                exports {
                    add: func(a: s32, b: s32) -> s32
                    sub: func(a: s32, b: s32) -> s32
                }
            }
        "#;

        let interface = parse_pact(src).expect("parse");
        let code = generate_rust(&interface);

        println!("{}", code);

        assert!(code.contains("pub struct Point"));
        assert!(code.contains("pub x: f32"));
        assert!(code.contains("pub trait Calculator"));
        assert!(code.contains("fn add(&self, a: i32, b: i32) -> i32"));
    }

    #[test]
    fn test_generate_variant() {
        let src = r#"
            interface shapes {
                variant shape {
                    circle(f32),
                    rectangle(tuple<f32, f32>),
                    point,
                }
            }
        "#;

        let interface = parse_pact(src).expect("parse");
        let code = generate_rust(&interface);

        println!("{}", code);

        assert!(code.contains("pub enum Shape"));
        assert!(code.contains("Circle(f32)"));
        assert!(code.contains("Rectangle((f32, f32))"));
        assert!(code.contains("Point,"));
    }

    #[test]
    fn test_generate_flags() {
        let src = r#"
            interface caps {
                flags permissions {
                    read,
                    write,
                    execute,
                }
            }
        "#;

        let interface = parse_pact(src).expect("parse");
        let code = generate_rust(&interface);

        println!("{}", code);

        assert!(code.contains("pub struct Permissions"));
        assert!(code.contains("pub const READ: Self"));
        assert!(code.contains("pub const WRITE: Self"));
        assert!(code.contains("fn contains(self, other: Self)"));
    }

    #[test]
    fn test_name_conversions() {
        // Basic conversions
        assert_eq!(to_pascal_case("my-interface"), "MyInterface");
        assert_eq!(to_pascal_case("my_interface"), "MyInterface");
        assert_eq!(to_snake_case("MyInterface"), "my_interface");

        // Transform names: rpc(calculator) -> RpcCalculator
        assert_eq!(to_pascal_case("rpc(calculator)"), "RpcCalculator");
        assert_eq!(to_snake_case("rpc(calculator)"), "rpc_calculator");

        // Nested transforms: traced(rpc(calculator))
        assert_eq!(
            to_pascal_case("traced(rpc(calculator))"),
            "TracedRpcCalculator"
        );

        // Generic-style: list<string>
        assert_eq!(to_pascal_case("list<string>"), "ListString");
    }
}
