//! Proc macros for Composite guest components.
//!
//! Provides the `#[export]` and `#[import]` attribute macros for easily
//! exporting and importing functions with the correct WASM calling convention.
//!
//! Also provides the `wit!()` macro for generating types from WIT+ definitions.

use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, ItemFn, ReturnType, FnArg, Pat, LitStr, Token, Ident};
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;

mod wit_parser;
mod codegen;

/// Arguments for the #[export] attribute.
struct ExportArgs {
    /// Custom export name (e.g., "theater:simple/actor.init")
    name: Option<String>,
    /// WIT function name to validate/match against (e.g., "init")
    wit: Option<String>,
}

impl Parse for ExportArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut args = ExportArgs { name: None, wit: None };

        if input.is_empty() {
            return Ok(args);
        }

        while !input.is_empty() {
            let ident: syn::Ident = input.parse()?;
            input.parse::<Token![=]>()?;
            let lit: LitStr = input.parse()?;

            match ident.to_string().as_str() {
                "name" => args.name = Some(lit.value()),
                "wit" => args.wit = Some(lit.value()),
                other => {
                    return Err(syn::Error::new(
                        ident.span(),
                        format!("unexpected attribute `{}`, expected `name` or `wit`", other),
                    ));
                }
            }

            // Consume optional comma
            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
        }

        Ok(args)
    }
}

/// Export a function with the Composite calling convention.
///
/// This macro transforms a Rust function into a WASM export with the
/// signature `(in_ptr: i32, in_len: i32, out_ptr: i32, out_cap: i32) -> i32`.
///
/// The input parameter type must implement `TryFrom<Value>` and the return
/// type must implement `Into<Value>`.
///
/// # Example
///
/// ```ignore
/// use composite_guest::export;
/// use composite_abi::Value;
///
/// #[export]
/// fn echo(input: Value) -> Value {
///     input
/// }
///
/// #[export]
/// fn double(n: i64) -> i64 {
///     n * 2
/// }
///
/// // With a custom export name (can include any characters)
/// #[export(name = "theater:simple/actor.init")]
/// fn init(input: Value) -> Value {
///     // Exported as "theater:simple/actor.init" instead of "init"
///     input
/// }
///
/// // With WIT validation - validates that "eval" exists in your wit/ world
/// #[export(name = "my:package/interface.eval", wit = "eval")]
/// fn eval(expr: Sexpr) -> Sexpr {
///     // Types validated against WIT definition
///     expr
/// }
/// ```
///
/// # Generated Code
///
/// The macro generates a `#[no_mangle] pub extern "C"` function with the
/// specified name (or the function name if not specified) that:
/// 1. Reads input bytes from `(in_ptr, in_len)`
/// 2. Decodes using Graph ABI
/// 3. Converts to the parameter type via `TryFrom<Value>`
/// 4. Calls your function
/// 5. Converts the result via `Into<Value>`
/// 6. Encodes using Graph ABI
/// 7. Writes to `(out_ptr, out_cap)`
/// 8. Returns the output length, or -1 on error
#[proc_macro_attribute]
pub fn export(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attr as ExportArgs);
    let input_fn = parse_macro_input!(item as ItemFn);

    // If wit attribute is provided, validate against WIT and optionally derive export name
    let derived_export_name = if let Some(ref wit_path) = args.wit {
        match validate_export_against_wit(wit_path) {
            Ok(result) => result.derived_name,
            Err(e) => {
                return syn::Error::new(
                    proc_macro2::Span::call_site(),
                    e,
                ).to_compile_error().into();
            }
        }
    } else {
        None
    };

    // Determine the export name: explicit name > derived from wit > function name
    let export_name = args.name.clone()
        .or(derived_export_name);

    let fn_name = &input_fn.sig.ident;
    let fn_body = &input_fn.block;
    let fn_vis = &input_fn.vis;

    // Extract parameter info
    let params: Vec<_> = input_fn.sig.inputs.iter().collect();

    if params.len() != 1 {
        return syn::Error::new_spanned(
            &input_fn.sig,
            "exported functions must have exactly one parameter"
        ).to_compile_error().into();
    }

    // Get the parameter name and type
    let (param_name, param_type) = match &params[0] {
        FnArg::Typed(pat_type) => {
            let name = match &*pat_type.pat {
                Pat::Ident(ident) => &ident.ident,
                _ => {
                    return syn::Error::new_spanned(
                        &pat_type.pat,
                        "parameter must be a simple identifier"
                    ).to_compile_error().into();
                }
            };
            (name, &pat_type.ty)
        }
        FnArg::Receiver(_) => {
            return syn::Error::new_spanned(
                &params[0],
                "exported functions cannot have self parameter"
            ).to_compile_error().into();
        }
    };

    // Get the return type
    let return_type = match &input_fn.sig.output {
        ReturnType::Default => {
            return syn::Error::new_spanned(
                &input_fn.sig,
                "exported functions must have a return type"
            ).to_compile_error().into();
        }
        ReturnType::Type(_, ty) => ty,
    };

    // Generate the inner function name (prefixed with underscore)
    let inner_fn_name = syn::Ident::new(
        &format!("__{}_inner", fn_name),
        fn_name.span()
    );

    // Generate the wrapper function name (always a valid Rust identifier)
    let wrapper_fn_name = syn::Ident::new(
        &format!("__{}_export", fn_name),
        fn_name.span()
    );

    // Generate the wrapper with the determined export name
    let expanded = match export_name {
        Some(custom_name) => {
            // Custom or derived name - use #[export_name] attribute
            quote! {
                // The user's original function (renamed)
                #fn_vis fn #inner_fn_name(#param_name: #param_type) -> #return_type
                #fn_body

                // The exported wrapper with WASM calling convention
                #[export_name = #custom_name]
                pub extern "C" fn #wrapper_fn_name(
                    in_ptr: i32,
                    in_len: i32,
                    out_ptr: i32,
                    out_cap: i32,
                ) -> i32 {
                    // Use the guest runtime to handle the boilerplate
                    composite_guest::__export_impl(
                        in_ptr, in_len, out_ptr, out_cap,
                        |value| {
                            // Convert input Value to user's type
                            let input: #param_type = match value.try_into() {
                                Ok(v) => v,
                                Err(_) => return Err("failed to convert input"),
                            };

                            // Call user's function
                            let output = #inner_fn_name(input);

                            // Convert output to Value
                            Ok(output.into())
                        }
                    )
                }
            }
        }
        None => {
            // No custom name - use #[no_mangle] with the original function name
            quote! {
                // The user's original function (renamed)
                #fn_vis fn #inner_fn_name(#param_name: #param_type) -> #return_type
                #fn_body

                // The exported wrapper with WASM calling convention
                #[no_mangle]
                pub extern "C" fn #fn_name(
                    in_ptr: i32,
                    in_len: i32,
                    out_ptr: i32,
                    out_cap: i32,
                ) -> i32 {
                    // Use the guest runtime to handle the boilerplate
                    composite_guest::__export_impl(
                        in_ptr, in_len, out_ptr, out_cap,
                        |value| {
                            // Convert input Value to user's type
                            let input: #param_type = match value.try_into() {
                                Ok(v) => v,
                                Err(_) => return Err("failed to convert input"),
                            };

                            // Call user's function
                            let output = #inner_fn_name(input);

                            // Convert output to Value
                            Ok(output.into())
                        }
                    )
                }
            }
        }
    };

    expanded.into()
}

/// Result of validating an export against WIT
struct WitValidationResult {
    /// The derived export name (from the WIT path)
    pub derived_name: Option<String>,
    /// The WIT function signature (params and results)
    pub function: Option<wit_parser::Function>,
}

/// Validate that a function exists in the WIT and optionally derive the export name.
///
/// The `wit_path` can be:
/// - A simple function name: "init" (searches all exports)
/// - A full path: "theater:simple/actor.init" (looks up specific interface)
fn validate_export_against_wit(wit_path: &str) -> Result<WitValidationResult, String> {
    // Read and parse WIT files
    let wit_content = read_wit_files()?;
    let registry = wit_parser::parse_wit(&wit_content)
        .map_err(|e| format!("Failed to parse WIT: {}", e))?;

    // Check if this is a full path (contains '.' or '#')
    if let Some(func_path) = wit_parser::FunctionPath::parse(wit_path) {
        // Full path specified - look up the specific function
        if let Some(func) = registry.find_function(&func_path) {
            return Ok(WitValidationResult {
                derived_name: Some(func_path.export_name()),
                function: Some(func.clone()),
            });
        }

        // Not found - provide helpful error
        let available = registry.available_exports();
        return Err(format!(
            "Function '{}' not found in WIT interfaces. Available: {:?}",
            wit_path, available
        ));
    }

    // Simple function name - search in exports and interfaces
    let func_name = wit_path;

    // First, check world exports
    for world in &registry.worlds {
        for export in &world.exports {
            match export {
                wit_parser::WorldItem::Function(f) if f.name == func_name => {
                    // Found as a bare export
                    return Ok(WitValidationResult {
                        derived_name: Some(func_name.to_string()),
                        function: Some(f.clone()),
                    });
                }
                wit_parser::WorldItem::InlineInterface { name: iface_name, functions } => {
                    if let Some(f) = functions.iter().find(|f| f.name == func_name) {
                        // Found in inline interface
                        return Ok(WitValidationResult {
                            derived_name: Some(format!("{}.{}", iface_name, func_name)),
                            function: Some(f.clone()),
                        });
                    }
                }
                wit_parser::WorldItem::InterfacePath { namespace, package, interface } => {
                    // Check if this interface path is in our registry
                    let iface_path = match (namespace, package) {
                        (Some(ns), Some(pkg)) => format!("{}:{}/{}", ns, pkg, interface),
                        (None, Some(pkg)) => format!("{}/{}", pkg, interface),
                        _ => interface.clone(),
                    };

                    if let Some(iface) = registry.interfaces.get(&iface_path) {
                        if let Some(f) = iface.functions.iter().find(|f| f.name == func_name) {
                            // Found in referenced interface
                            return Ok(WitValidationResult {
                                derived_name: Some(format!("{}.{}", iface_path, func_name)),
                                function: Some(f.clone()),
                            });
                        }
                    }
                }
                _ => {}
            }
        }
    }

    // Check top-level interfaces
    for (path, iface) in &registry.interfaces {
        if let Some(f) = iface.functions.iter().find(|f| f.name == func_name) {
            return Ok(WitValidationResult {
                derived_name: Some(format!("{}.{}", path, func_name)),
                function: Some(f.clone()),
            });
        }
    }

    // Not found
    let available = registry.available_exports();
    Err(format!(
        "Function '{}' not found in WIT exports. Available: {:?}",
        func_name, available
    ))
}

/// Result of validating an import against WIT
struct WitImportValidationResult {
    /// The derived module name (interface path)
    pub module: Option<String>,
    /// The derived import name (function name)
    pub import_name: Option<String>,
}

/// Validate that a function exists in the WIT imports and derive module/name.
///
/// The `wit_path` should be a full path like "theater:simple/runtime.log"
fn validate_import_against_wit(wit_path: &str) -> Result<WitImportValidationResult, String> {
    // Read and parse WIT files
    let wit_content = read_wit_files()?;
    let registry = wit_parser::parse_wit(&wit_content)
        .map_err(|e| format!("Failed to parse WIT: {}", e))?;

    // Parse the function path
    let func_path = wit_parser::FunctionPath::parse(wit_path)
        .ok_or_else(|| format!(
            "Invalid WIT path '{}'. Expected format: 'namespace:package/interface.function'",
            wit_path
        ))?;

    // Look up the function in the registry
    if registry.find_import_function(&func_path).is_some() {
        return Ok(WitImportValidationResult {
            module: Some(func_path.interface.to_string()),
            import_name: Some(func_path.function),
        });
    }

    // Also check if the function exists in any interface (even if not explicitly imported)
    if registry.find_function(&func_path).is_some() {
        return Ok(WitImportValidationResult {
            module: Some(func_path.interface.to_string()),
            import_name: Some(func_path.function),
        });
    }

    // Not found - provide helpful error
    let available = registry.available_imports();
    Err(format!(
        "Function '{}' not found in WIT interfaces. Available imports: {:?}",
        wit_path, available
    ))
}

/// Arguments for the #[import] attribute.
struct ImportArgs {
    /// Module name (e.g., "theater:simple/runtime")
    module: Option<String>,
    /// Function name override
    name: Option<String>,
    /// WIT path for validation and auto-derivation (e.g., "theater:simple/runtime.log")
    wit: Option<String>,
}

impl Parse for ImportArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut module = None;
        let mut name = None;
        let mut wit = None;

        while !input.is_empty() {
            let ident: Ident = input.parse()?;
            input.parse::<Token![=]>()?;
            let lit: LitStr = input.parse()?;

            match ident.to_string().as_str() {
                "module" => module = Some(lit.value()),
                "name" => name = Some(lit.value()),
                "wit" => wit = Some(lit.value()),
                other => {
                    return Err(syn::Error::new(
                        ident.span(),
                        format!("unexpected attribute `{}`, expected `module`, `name`, or `wit`", other),
                    ));
                }
            }

            // Consume optional comma
            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
        }

        // Either module or wit must be specified
        if module.is_none() && wit.is_none() {
            return Err(syn::Error::new(
                input.span(),
                "either `module` or `wit` attribute is required",
            ));
        }

        Ok(ImportArgs { module, name, wit })
    }
}

/// A function signature for imports (fn name(args) -> ReturnType;)
struct ImportFnSignature {
    vis: syn::Visibility,
    fn_name: Ident,
    inputs: Punctuated<FnArg, Token![,]>,
    output: ReturnType,
}

impl Parse for ImportFnSignature {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let vis: syn::Visibility = input.parse()?;
        input.parse::<Token![fn]>()?;
        let fn_name: Ident = input.parse()?;

        let content;
        syn::parenthesized!(content in input);
        let inputs = content.parse_terminated(FnArg::parse, Token![,])?;

        let output: ReturnType = input.parse()?;
        input.parse::<Token![;]>()?;

        Ok(ImportFnSignature {
            vis,
            fn_name,
            inputs,
            output,
        })
    }
}

/// Import a function from the host with the Composite calling convention.
///
/// This macro generates a wrapper function that handles Graph ABI encoding/decoding
/// for calling host-provided functions.
///
/// # Example
///
/// ```ignore
/// use composite_guest::import;
///
/// // Import a log function from the host (manual module specification)
/// #[import(module = "theater:simple/runtime")]
/// fn log(msg: String);
///
/// // Import with WIT path - module and name derived automatically
/// #[import(wit = "theater:simple/runtime.log")]
/// fn log(msg: String);
///
/// // Import with a custom function name
/// #[import(module = "theater:simple/runtime", name = "log")]
/// fn my_log(msg: String);
///
/// // Import a function that returns a value
/// #[import(wit = "theater:simple/runtime.get-chain")]
/// fn get_chain() -> Chain;
/// ```
///
/// # Generated Code
///
/// The macro generates:
/// 1. An `extern "C"` block declaring the raw WASM import
/// 2. A wrapper function with your signature that:
///    - Converts arguments to `Value` and encodes using Graph ABI
///    - Calls the raw import function
///    - Decodes the result and converts back to your return type
#[proc_macro_attribute]
pub fn import(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attr as ImportArgs);
    let sig = parse_macro_input!(item as ImportFnSignature);

    // If wit attribute is provided, validate and derive module/name
    let (derived_module, derived_name) = if let Some(ref wit_path) = args.wit {
        match validate_import_against_wit(wit_path) {
            Ok(result) => (result.module, result.import_name),
            Err(e) => {
                return syn::Error::new(
                    proc_macro2::Span::call_site(),
                    e,
                ).to_compile_error().into();
            }
        }
    } else {
        (None, None)
    };

    // Determine module: explicit > derived from wit
    let module = args.module.clone()
        .or(derived_module)
        .expect("module should be set by either `module` or `wit` attribute");

    // Determine import name: explicit > derived from wit > function name
    let import_name = args.name.clone()
        .or(derived_name)
        .unwrap_or_else(|| sig.fn_name.to_string());

    let fn_name = &sig.fn_name;
    let fn_vis = &sig.vis;
    let output = &sig.output;

    // Generate a unique name for the raw import
    let raw_fn_name = Ident::new(
        &format!("__raw_import_{}", fn_name),
        fn_name.span()
    );

    // Extract parameter names and types
    let params: Vec<_> = sig.inputs.iter().collect();
    let mut param_names = Vec::new();
    let mut param_types = Vec::new();

    for param in &params {
        match param {
            FnArg::Typed(pat_type) => {
                let name = match &*pat_type.pat {
                    Pat::Ident(ident) => &ident.ident,
                    _ => {
                        return syn::Error::new_spanned(
                            &pat_type.pat,
                            "parameter must be a simple identifier"
                        ).to_compile_error().into();
                    }
                };
                param_names.push(name.clone());
                param_types.push((*pat_type.ty).clone());
            }
            FnArg::Receiver(_) => {
                return syn::Error::new_spanned(
                    param,
                    "imported functions cannot have self parameter"
                ).to_compile_error().into();
            }
        }
    }

    // Determine return type handling
    let (return_type, has_return) = match output {
        ReturnType::Default => (quote! { () }, false),
        ReturnType::Type(_, ty) => (quote! { #ty }, true),
    };

    // Build the input value - tuple of all parameters
    let input_construction = if param_names.is_empty() {
        quote! { composite_guest::Value::Tuple(composite_guest::__alloc::vec![]) }
    } else if param_names.len() == 1 {
        let name = &param_names[0];
        quote! { composite_guest::Value::from(#name) }
    } else {
        let conversions = param_names.iter().map(|name| {
            quote! { composite_guest::Value::from(#name) }
        });
        quote! {
            composite_guest::Value::Tuple(composite_guest::__alloc::vec![#(#conversions),*])
        }
    };

    // Build the return value handling
    let return_handling = if has_return {
        quote! {
            match result.try_into() {
                Ok(v) => v,
                Err(_) => panic!("failed to convert import result"),
            }
        }
    } else {
        quote! { () }
    };

    // Generate the function signature parameters
    let fn_params = param_names.iter().zip(param_types.iter()).map(|(name, ty)| {
        quote! { #name: #ty }
    });

    let expanded = quote! {
        #[link(wasm_import_module = #module)]
        extern "C" {
            #[link_name = #import_name]
            fn #raw_fn_name(in_ptr: i32, in_len: i32, out_ptr: i32, out_cap: i32) -> i32;
        }

        #fn_vis fn #fn_name(#(#fn_params),*) -> #return_type {
            let input = #input_construction;
            let result = composite_guest::__import_impl(
                |in_ptr, in_len, out_ptr, out_cap| unsafe {
                    #raw_fn_name(in_ptr, in_len, out_ptr, out_cap)
                },
                input,
            );
            #return_handling
        }
    };

    expanded.into()
}

/// Generate types and bindings from WIT+ definitions.
///
/// This macro reads WIT+ files from the `wit/` directory in your crate and generates:
/// - Rust types for all type definitions (records, variants, enums, flags)
/// - `From<T> for Value` implementations for converting to Value
/// - `TryFrom<Value> for T` implementations for converting from Value
///
/// # Usage
///
/// Create a `wit/` directory in your crate root with `.wit` files:
///
/// ```wit
/// // wit/world.wit
/// variant sexpr {
///     sym(string),
///     num(s64),
///     cons(list<sexpr>),
///     nil,
/// }
///
/// world my-actor {
///     export eval: func(expr: sexpr) -> sexpr
/// }
/// ```
///
/// Then in your Rust code:
///
/// ```ignore
/// use composite_guest::wit;
///
/// // Generate types from wit/ directory
/// wit!();
///
/// // Now you can use the generated types
/// #[export]
/// fn eval(expr: Sexpr) -> Sexpr {
///     // ...
/// }
/// ```
///
/// # Alternative: Inline WIT
///
/// You can also provide WIT content directly:
///
/// ```ignore
/// wit! {
///     variant sexpr {
///         sym(string),
///         num(s64),
///         nil,
///     }
///
///     world my-actor {
///         export eval: func(expr: sexpr) -> sexpr
///     }
/// }
/// ```
#[proc_macro]
pub fn wit(input: TokenStream) -> TokenStream {
    // Check if we have inline content or should read from files
    let input_str = input.to_string();

    let wit_content = if input_str.trim().is_empty() {
        // Read from wit/ directory
        match read_wit_files() {
            Ok(content) => content,
            Err(e) => {
                return syn::Error::new(
                    proc_macro2::Span::call_site(),
                    format!("Failed to read WIT files: {}", e)
                ).to_compile_error().into();
            }
        }
    } else {
        // Use inline content - parse the token stream as a raw string
        // The input is the raw WIT content between the braces
        input_str
    };

    // Parse the WIT content
    let world = match wit_parser::parse_world(&wit_content) {
        Ok(w) => w,
        Err(e) => {
            return syn::Error::new(
                proc_macro2::Span::call_site(),
                format!("Failed to parse WIT: {}", e)
            ).to_compile_error().into();
        }
    };

    // Generate the types
    let generated = codegen::generate_world_types(&world);

    generated.into()
}

/// Read all WIT files from the wit/ directory and wit/deps/ subdirectories
fn read_wit_files() -> Result<String, String> {
    // Get the manifest directory (crate root)
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
        .map_err(|_| "CARGO_MANIFEST_DIR not set")?;

    let wit_dir = std::path::Path::new(&manifest_dir).join("wit");

    if !wit_dir.exists() {
        return Err(format!("wit/ directory not found at {:?}", wit_dir));
    }

    let mut content = String::new();

    // Read WIT files recursively (includes wit/deps/)
    read_wit_files_recursive(&wit_dir, &mut content)?;

    if content.is_empty() {
        return Err("No .wit or .wit+ files found in wit/ directory".to_string());
    }

    Ok(content)
}

/// Recursively read WIT files from a directory
fn read_wit_files_recursive(dir: &std::path::Path, content: &mut String) -> Result<(), String> {
    let entries = std::fs::read_dir(dir)
        .map_err(|e| format!("Failed to read directory {:?}: {}", dir, e))?;

    for entry in entries {
        let entry = entry.map_err(|e| format!("Failed to read directory entry: {}", e))?;
        let path = entry.path();

        if path.is_dir() {
            // Recurse into subdirectories (including deps/)
            read_wit_files_recursive(&path, content)?;
        } else if let Some(ext) = path.extension() {
            if ext == "wit" || ext == "wit+" {
                let file_content = std::fs::read_to_string(&path)
                    .map_err(|e| format!("Failed to read {:?}: {}", path, e))?;
                content.push_str(&file_content);
                content.push('\n');
            }
        }
    }

    Ok(())
}
