// ============================================================================
// SKETCH: How the world! macro implementation would work
// This is pseudocode showing the key pieces
// ============================================================================

// In pack-guest-macros/src/lib.rs:

/// Parse the WIT+ world and generate types + imports + export metadata
#[proc_macro]
pub fn world(input: TokenStream) -> TokenStream {
    // 1. Read WIT+ files from wit/ directory
    let wit_content = read_wit_files().expect("Failed to read WIT files");

    // 2. Parse into a WitRegistry (interfaces, worlds, types)
    let registry = wit_parser::parse_wit(&wit_content).expect("Failed to parse WIT");

    // 3. Find the world (use first one, or allow specifying)
    let world = registry.worlds.first().expect("No world defined");

    // 4. Generate everything
    let types = codegen::generate_world_types(world);
    let imports = codegen::generate_imports(&registry, world);
    let export_metadata = codegen::generate_export_metadata(&registry, world);

    quote! {
        #types
        #imports
        #export_metadata
    }.into()
}

// ============================================================================
// In pack-guest-macros/src/codegen.rs - New functions:
// ============================================================================

/// Generate import modules from world imports
pub fn generate_imports(registry: &WitRegistry, world: &World) -> TokenStream {
    let mut modules = Vec::new();

    for import in &world.imports {
        match import {
            WorldItem::InterfacePath { namespace, package, interface } => {
                // Look up the interface definition
                let path = format_interface_path(namespace, package, interface);
                if let Some(iface) = registry.interfaces.get(&path) {
                    let module = generate_import_module(&path, iface);
                    modules.push(module);
                }
            }
            WorldItem::InlineInterface { name, functions } => {
                let module = generate_inline_import_module(name, functions);
                modules.push(module);
            }
            _ => {}
        }
    }

    quote! { #(#modules)* }
}

/// Generate a single import module
fn generate_import_module(module_path: &str, iface: &Interface) -> TokenStream {
    // Convert "theater:simple/runtime" to module name "runtime"
    let module_name = iface.name.replace('-', "_");
    let module_ident = format_ident!("{}", module_name);

    let functions: Vec<_> = iface.functions.iter()
        .map(|f| generate_import_function(module_path, f))
        .collect();

    quote! {
        pub mod #module_ident {
            use super::*;
            #(#functions)*
        }
    }
}

/// Generate a single typed import function
fn generate_import_function(module_path: &str, func: &Function) -> TokenStream {
    let fn_name = format_ident!("{}", func.name.replace('-', "_"));
    let raw_fn_name = format_ident!("__raw_{}", func.name.replace('-', "_"));
    let link_name = &func.name;

    // Generate parameter list
    let params: Vec<_> = func.params.iter().map(|(name, ty)| {
        let param_name = format_ident!("{}", name.replace('-', "_"));
        let param_type = generate_type_ref(ty, None);
        // For string params in imports, use &str instead of String
        let param_type = if matches!(ty, Type::String) {
            quote! { &str }
        } else {
            param_type
        };
        quote! { #param_name: #param_type }
    }).collect();

    // Generate return type
    let return_type = if func.results.is_empty() {
        quote! { () }
    } else if func.results.len() == 1 {
        generate_type_ref(&func.results[0], None)
    } else {
        let tys: Vec<_> = func.results.iter()
            .map(|t| generate_type_ref(t, None))
            .collect();
        quote! { (#(#tys),*) }
    };

    // Generate input value construction
    let input_construction = if func.params.is_empty() {
        quote! { pack_guest::Value::Tuple(::alloc::vec![]) }
    } else if func.params.len() == 1 {
        let (name, ty) = &func.params[0];
        let param_name = format_ident!("{}", name.replace('-', "_"));
        generate_to_value_for_import(ty, quote! { #param_name })
    } else {
        let conversions: Vec<_> = func.params.iter().map(|(name, ty)| {
            let param_name = format_ident!("{}", name.replace('-', "_"));
            generate_to_value_for_import(ty, quote! { #param_name })
        }).collect();
        quote! { pack_guest::Value::Tuple(::alloc::vec![#(#conversions),*]) }
    };

    // Generate return extraction
    let return_extraction = if func.results.is_empty() {
        quote! { }
    } else {
        let result_type = &return_type;
        quote! {
            match result.try_into() {
                Ok(v) => v,
                Err(_) => panic!("failed to convert {} result", stringify!(#fn_name)),
            }
        }
    };

    let has_return = !func.results.is_empty();
    let body = if has_return {
        quote! {
            let input = #input_construction;
            let result = pack_guest::__import_impl(
                |a, b, c, d| unsafe { #raw_fn_name(a, b, c, d) },
                input,
            );
            #return_extraction
        }
    } else {
        quote! {
            let input = #input_construction;
            let _ = pack_guest::__import_impl(
                |a, b, c, d| unsafe { #raw_fn_name(a, b, c, d) },
                input,
            );
        }
    };

    quote! {
        pub fn #fn_name(#(#params),*) -> #return_type {
            #[link(wasm_import_module = #module_path)]
            extern "C" {
                #[link_name = #link_name]
                fn #raw_fn_name(in_ptr: i32, in_len: i32, out_ptr: i32, out_cap: i32) -> i32;
            }

            #body
        }
    }
}

/// Generate value conversion for import params (handles &str specially)
fn generate_to_value_for_import(ty: &Type, expr: TokenStream) -> TokenStream {
    match ty {
        Type::String => quote! {
            pack_guest::Value::String(::alloc::string::String::from(#expr))
        },
        // Other types use the standard conversion
        _ => generate_to_value(ty, expr, None),
    }
}

/// Generate export metadata for validation
pub fn generate_export_metadata(registry: &WitRegistry, world: &World) -> TokenStream {
    let mut exports = Vec::new();

    for export in &world.exports {
        match export {
            WorldItem::Function(f) => {
                let sig = format_function_signature(f);
                exports.push((&f.name, sig, None));
            }
            WorldItem::InterfacePath { namespace, package, interface } => {
                let path = format_interface_path(namespace, package, interface);
                if let Some(iface) = registry.interfaces.get(&path) {
                    for f in &iface.functions {
                        let sig = format_function_signature(f);
                        let export_name = format!("{}.{}", path, f.name);
                        exports.push((&f.name, sig, Some(export_name)));
                    }
                }
            }
            WorldItem::InlineInterface { name, functions } => {
                for f in functions {
                    let sig = format_function_signature(f);
                    let export_name = format!("{}.{}", name, f.name);
                    exports.push((&f.name, sig, Some(export_name)));
                }
            }
        }
    }

    let entries: Vec<_> = exports.iter().map(|(name, sig, export_name)| {
        let export_name_str = export_name.as_ref().map(|s| s.as_str()).unwrap_or(*name);
        quote! {
            (#name, #sig, #export_name_str)
        }
    }).collect();

    quote! {
        #[doc(hidden)]
        pub mod __pack_exports {
            /// (function_name, wit_signature, export_name)
            pub const EXPORTS: &[(&str, &str, &str)] = &[
                #(#entries),*
            ];

            pub fn get_export(name: &str) -> Option<(&'static str, &'static str)> {
                EXPORTS.iter()
                    .find(|(n, _, _)| *n == name)
                    .map(|(_, sig, export_name)| (*sig, *export_name))
            }
        }
    }
}

// ============================================================================
// Updated #[export] macro that validates against world
// ============================================================================

#[proc_macro_attribute]
pub fn export(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attr as ExportArgs);
    let input_fn = parse_macro_input!(item as ItemFn);

    let fn_name = input_fn.sig.ident.to_string();

    // Try to load the world metadata
    // (In reality, we'd parse the WIT files again or use a cached version)
    let wit_content = match read_wit_files() {
        Ok(c) => c,
        Err(_) => {
            // No world defined - fall back to current behavior
            return generate_export_without_validation(args, input_fn).into();
        }
    };

    let registry = match wit_parser::parse_wit(&wit_content) {
        Ok(r) => r,
        Err(_) => return generate_export_without_validation(args, input_fn).into(),
    };

    // Find the function in exports
    let world = match registry.worlds.first() {
        Some(w) => w,
        None => return generate_export_without_validation(args, input_fn).into(),
    };

    // Look up the expected signature
    let (expected_func, export_name) = match find_export_function(&registry, world, &fn_name) {
        Some(result) => result,
        None => {
            // Not found in exports - could be an error or allow it?
            return syn::Error::new(
                input_fn.sig.ident.span(),
                format!(
                    "function `{}` is not declared as an export in the WIT+ world\n\
                    Available exports: {:?}",
                    fn_name,
                    registry.available_exports()
                )
            ).to_compile_error().into();
        }
    };

    // Validate the Rust signature matches the WIT signature
    if let Err(e) = validate_signature(&input_fn.sig, expected_func) {
        return e.to_compile_error().into();
    }

    // Generate the export wrapper with the correct export name
    generate_validated_export(input_fn, expected_func, &export_name).into()
}

/// Validate that a Rust function signature matches the expected WIT signature
fn validate_signature(sig: &syn::Signature, expected: &Function) -> Result<(), syn::Error> {
    // Check parameter count
    let rust_params: Vec<_> = sig.inputs.iter().collect();
    if rust_params.len() != expected.params.len() {
        return Err(syn::Error::new(
            sig.paren_token.span.join(),
            format!(
                "parameter count mismatch: expected {} parameters, got {}\n\
                Expected: {}\n\
                Hint: parameters should be: {}",
                expected.params.len(),
                rust_params.len(),
                format_function_signature(expected),
                format_expected_params(expected),
            )
        ));
    }

    // Check each parameter type
    for (i, ((wit_name, wit_type), rust_param)) in expected.params.iter().zip(rust_params.iter()).enumerate() {
        if let FnArg::Typed(pat_type) = rust_param {
            if !type_matches(&pat_type.ty, wit_type) {
                let expected_rust = wit_type_to_rust_string(wit_type);
                return Err(syn::Error::new_spanned(
                    &pat_type.ty,
                    format!(
                        "parameter `{}` type mismatch:\n\
                        Expected (from WIT): {}\n\
                        Got: {}",
                        wit_name,
                        expected_rust,
                        quote! { #pat_type.ty }.to_string(),
                    )
                ));
            }
        }
    }

    // Check return type
    // ... similar validation ...

    Ok(())
}

/// Generate export wrapper with validated signature
fn generate_validated_export(
    input_fn: ItemFn,
    expected: &Function,
    export_name: &str,
) -> TokenStream {
    let fn_name = &input_fn.sig.ident;
    let fn_body = &input_fn.block;

    // Extract parameter info from the WIT function
    let param_names: Vec<_> = expected.params.iter()
        .map(|(name, _)| format_ident!("{}", name.replace('-', "_")))
        .collect();

    let param_extractions: Vec<_> = expected.params.iter().enumerate()
        .map(|(i, (name, ty))| {
            let param_name = format_ident!("{}", name.replace('-', "_"));
            let rust_type = generate_type_ref(ty, None);
            quote! {
                let #param_name: #rust_type = items.get(#i)
                    .cloned()
                    .ok_or("missing parameter")?
                    .try_into()
                    .map_err(|_| "type conversion failed")?;
            }
        })
        .collect();

    let inner_fn_name = format_ident!("__{}_inner", fn_name);
    let wrapper_fn_name = format_ident!("__{}_export", fn_name);

    // Recreate the inner function with proper types from WIT
    let inner_params: Vec<_> = expected.params.iter()
        .map(|(name, ty)| {
            let param_name = format_ident!("{}", name.replace('-', "_"));
            let rust_type = generate_type_ref(ty, None);
            quote! { #param_name: #rust_type }
        })
        .collect();

    let return_type = if expected.results.is_empty() {
        quote! { () }
    } else if expected.results.len() == 1 {
        generate_type_ref(&expected.results[0], None)
    } else {
        let tys: Vec<_> = expected.results.iter()
            .map(|t| generate_type_ref(t, None))
            .collect();
        quote! { (#(#tys),*) }
    };

    quote! {
        fn #inner_fn_name(#(#inner_params),*) -> #return_type
        #fn_body

        #[export_name = #export_name]
        pub extern "C" fn #wrapper_fn_name(
            in_ptr: i32, in_len: i32, out_ptr: i32, out_cap: i32
        ) -> i32 {
            pack_guest::__export_impl(in_ptr, in_len, out_ptr, out_cap, |value| {
                let items = match value {
                    pack_guest::Value::Tuple(items) => items,
                    _ => return Err("expected tuple"),
                };

                #(#param_extractions)*

                let result = #inner_fn_name(#(#param_names),*);
                Ok(result.into())
            })
        }
    }
}
