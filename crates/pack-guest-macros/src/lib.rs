//! Proc macros for Pack guest packages.
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
mod metadata;

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
/// # Modes
///
/// **Value mode** (single `Value` parameter): The raw `Value` is passed directly
/// to your function. You handle all encoding/decoding manually.
///
/// **Typed mode** (with `wit` attribute and typed parameters): The macro automatically
/// extracts typed parameters from the input and wraps the result. Parameters must
/// implement `TryFrom<Value>` and return type must implement `Into<Value>`.
///
/// # Example
///
/// ```ignore
/// use pack_guest::export;
/// use pack_guest::Value;
///
/// // Value mode - raw Value handling
/// #[export]
/// fn echo(input: Value) -> Value {
///     input
/// }
///
/// // Typed mode with WIT validation
/// // The macro extracts the state param and wraps the Result
/// #[export(wit = "theater:simple/actor.init")]
/// fn init(state: Option<Vec<u8>>) -> Result<(Option<Vec<u8>>,), String> {
///     Ok((state,))
/// }
///
/// // Multiple typed parameters
/// #[export(wit = "my:package/geo.translate")]
/// fn translate(p: Point, dx: i32, dy: i32) -> Point {
///     Point { x: p.x + dx, y: p.y + dy }
/// }
/// ```
///
/// # Generated Code
///
/// The macro generates a `#[no_mangle] pub extern "C"` function with the
/// specified name (or the function name if not specified) that:
/// 1. Reads input bytes from `(in_ptr, in_len)`
/// 2. Decodes using Graph ABI
/// 3. Extracts parameters from the input tuple (typed mode) or passes Value directly
/// 4. Calls your function
/// 5. Converts the result via `Into<Value>`
/// 6. Encodes using Graph ABI
/// 7. Writes to `(out_ptr, out_cap)`
/// 8. Returns the output length, or -1 on error
#[proc_macro_attribute]
pub fn export(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attr as ExportArgs);
    let input_fn = parse_macro_input!(item as ItemFn);

    let fn_name_str = input_fn.sig.ident.to_string();

    // Try to derive export name from WIT:
    // 1. If wit attribute is explicitly provided, use it
    // 2. Otherwise, try to find the function in the world automatically
    let derived_export_name = if let Some(ref wit_path) = args.wit {
        // Explicit wit path provided
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
        // Try auto-discovery: look up function by name in world exports
        try_auto_discover_export(&fn_name_str)
    };

    // Determine the export name: explicit name > derived from wit > function name
    let export_name = args.name.clone()
        .or(derived_export_name);

    let fn_name = &input_fn.sig.ident;
    let fn_body = &input_fn.block;
    let fn_vis = &input_fn.vis;

    // Extract parameter info
    let params: Vec<_> = input_fn.sig.inputs.iter().collect();

    // Get all parameter names and types
    let mut param_names = Vec::new();
    let mut param_types = Vec::new();

    for param in &params {
        match param {
            FnArg::Typed(pat_type) => {
                let name = match &*pat_type.pat {
                    Pat::Ident(ident) => ident.ident.clone(),
                    _ => {
                        return syn::Error::new_spanned(
                            &pat_type.pat,
                            "parameter must be a simple identifier"
                        ).to_compile_error().into();
                    }
                };
                param_names.push(name);
                param_types.push((*pat_type.ty).clone());
            }
            FnArg::Receiver(_) => {
                return syn::Error::new_spanned(
                    param,
                    "exported functions cannot have self parameter"
                ).to_compile_error().into();
            }
        }
    }

    // Detect if this is "Value mode" (single Value parameter) or "Typed mode"
    let is_value_mode = param_names.len() == 1 && {
        // Check if the type is `Value` (simple path check)
        let ty = &param_types[0];
        if let syn::Type::Path(type_path) = ty {
            type_path.path.segments.last()
                .map(|seg| seg.ident == "Value")
                .unwrap_or(false)
        } else {
            false
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

    // Generate the function parameters for the inner function declaration
    let inner_fn_params = param_names.iter().zip(param_types.iter()).map(|(name, ty)| {
        quote! { #name: #ty }
    });

    // Generate the parameter extraction and function call based on mode
    let call_body = if is_value_mode {
        // Value mode - pass the Value directly (backward compatible)
        let param_name = &param_names[0];
        let param_type = &param_types[0];
        quote! {
            // Convert input Value to user's type (should be identity for Value)
            let #param_name: #param_type = match value.try_into() {
                Ok(v) => v,
                Err(_) => return Err("failed to convert input"),
            };

            // Call user's function
            let output = #inner_fn_name(#param_name);

            // Convert output to Value
            Ok(output.into())
        }
    } else if param_names.is_empty() {
        // No parameters - just call the function
        quote! {
            // Call user's function (no parameters)
            let output = #inner_fn_name();

            // Convert output to Value
            Ok(output.into())
        }
    } else if param_names.len() == 1 {
        // Single typed parameter - extract from value directly
        let param_name = &param_names[0];
        let param_type = &param_types[0];
        quote! {
            // Extract single typed parameter
            let #param_name: #param_type = match value.try_into() {
                Ok(v) => v,
                Err(_) => return Err("failed to convert parameter"),
            };

            // Call user's function
            let output = #inner_fn_name(#param_name);

            // Convert output to Value
            Ok(output.into())
        }
    } else {
        // Multiple typed parameters - extract from tuple
        let num_params = param_names.len();
        let indices: Vec<_> = (0..num_params).collect();

        let extractions = param_names.iter().zip(param_types.iter()).zip(indices.iter()).map(|((name, ty), idx)| {
            quote! {
                let #name: #ty = match items.get(#idx).cloned() {
                    Some(v) => match v.try_into() {
                        Ok(converted) => converted,
                        Err(_) => return Err("failed to convert parameter"),
                    },
                    None => return Err("missing parameter in tuple"),
                };
            }
        });

        let call_args = param_names.iter();

        quote! {
            // Extract multiple typed parameters from input tuple
            let items = match value {
                pack_guest::Value::Tuple(items) => items,
                _ => return Err("expected tuple of parameters"),
            };

            #(#extractions)*

            // Call user's function with extracted parameters
            let output = #inner_fn_name(#(#call_args),*);

            // Convert output to Value
            Ok(output.into())
        }
    };

    // Generate the wrapper with the determined export name
    let expanded = match export_name {
        Some(custom_name) => {
            // Custom or derived name - use #[export_name] attribute
            quote! {
                // The user's original function (renamed)
                #fn_vis fn #inner_fn_name(#(#inner_fn_params),*) -> #return_type
                #fn_body

                // The exported wrapper with WASM calling convention
                // ABI: guest allocates output, writes ptr/len to provided slots
                // Returns 0 = success, -1 = error (error message in ptr/len)
                #[export_name = #custom_name]
                pub extern "C" fn #wrapper_fn_name(
                    in_ptr: i32,
                    in_len: i32,
                    out_ptr_ptr: i32,
                    out_len_ptr: i32,
                ) -> i32 {
                    // Use the guest runtime to handle the boilerplate
                    pack_guest::__export_impl(
                        in_ptr, in_len, out_ptr_ptr, out_len_ptr,
                        |value| {
                            #call_body
                        }
                    )
                }
            }
        }
        None => {
            // No custom name - use #[no_mangle] with the original function name
            quote! {
                // The user's original function (renamed)
                #fn_vis fn #inner_fn_name(#(#inner_fn_params),*) -> #return_type
                #fn_body

                // The exported wrapper with WASM calling convention
                // ABI: guest allocates output, writes ptr/len to provided slots
                // Returns 0 = success, -1 = error (error message in ptr/len)
                #[no_mangle]
                pub extern "C" fn #fn_name(
                    in_ptr: i32,
                    in_len: i32,
                    out_ptr_ptr: i32,
                    out_len_ptr: i32,
                ) -> i32 {
                    // Use the guest runtime to handle the boilerplate
                    pack_guest::__export_impl(
                        in_ptr, in_len, out_ptr_ptr, out_len_ptr,
                        |value| {
                            #call_body
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

/// Try to auto-discover the export name for a function by looking it up in the world.
///
/// This is a "best effort" lookup - it returns None if:
/// - No WIT files are found
/// - No world is defined
/// - The function is not found in exports
///
/// This allows the macro to work both with and without a WIT world definition.
fn try_auto_discover_export(fn_name: &str) -> Option<String> {
    // Try to read WIT files, but don't error if not found
    let wit_content = match read_wit_files() {
        Ok(c) => c,
        Err(_) => return None,
    };

    // Try to parse the WIT, but don't error on failure
    let registry = match wit_parser::parse_wit(&wit_content) {
        Ok(r) => r,
        Err(_) => return None,
    };

    // Search for the function in world exports
    for world in &registry.worlds {
        for export in &world.exports {
            match export {
                wit_parser::WorldItem::Function(f) if f.name == fn_name => {
                    // Found as a bare export - use just the function name
                    return Some(fn_name.to_string());
                }
                wit_parser::WorldItem::InlineInterface { name: iface_name, functions } => {
                    if functions.iter().any(|f| f.name == fn_name) {
                        // Found in inline interface - use interface.function format
                        return Some(format!("{}.{}", iface_name, fn_name));
                    }
                }
                wit_parser::WorldItem::InterfacePath { namespace, package, interface } => {
                    // Check if this interface has the function
                    let iface_path = match (namespace, package) {
                        (Some(ns), Some(pkg)) => format!("{}:{}/{}", ns, pkg, interface),
                        (None, Some(pkg)) => format!("{}/{}", pkg, interface),
                        _ => interface.clone(),
                    };

                    if let Some(iface) = registry.interfaces.get(&iface_path) {
                        if iface.functions.iter().any(|f| f.name == fn_name) {
                            // Found in referenced interface
                            return Some(format!("{}.{}", iface_path, fn_name));
                        }
                    }
                }
                _ => {}
            }
        }
    }

    // Not found - return None (no error, just use default behavior)
    None
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
        // Skip any outer attributes (including doc comments)
        let _ = input.call(syn::Attribute::parse_outer)?;

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
/// use pack_guest::import;
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
        quote! { pack_guest::Value::Tuple(pack_guest::__alloc::vec![]) }
    } else if param_names.len() == 1 {
        let name = &param_names[0];
        quote! { pack_guest::Value::from(#name) }
    } else {
        let conversions = param_names.iter().map(|name| {
            quote! { pack_guest::Value::from(#name) }
        });
        quote! {
            pack_guest::Value::Tuple(pack_guest::__alloc::vec![#(#conversions),*])
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
            let result = pack_guest::__import_impl(
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
/// use pack_guest::wit;
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

/// Parse the WIT+ world and generate types, imports, and export metadata.
///
/// This macro reads WIT+ files from the `wit/` directory in your crate and generates:
/// - Rust types for all type definitions (records, variants, enums, flags)
/// - Import modules with fully typed functions
/// - Export metadata for `#[export]` validation
///
/// # Usage
///
/// Create a `wit/` directory in your crate root with `.wit` or `.wit+` files:
///
/// ```wit
/// // wit/world.wit+
/// interface runtime {
///     log: func(msg: string)
///     get-time: func() -> u64
/// }
///
/// world my-actor {
///     import runtime
///     export init: func(state: option<list<u8>>) -> option<list<u8>>
/// }
/// ```
///
/// Then in your Rust code:
///
/// ```ignore
/// #![no_std]
/// extern crate alloc;
///
/// use pack_guest::export;
///
/// // Generate types, imports, and export metadata
/// pack_guest::world!();
///
/// #[export]
/// fn init(state: Option<Vec<u8>>) -> Option<Vec<u8>> {
///     // Use generated import - fully typed!
///     runtime::log("Starting!");
///     state
/// }
/// ```
///
/// # What Gets Generated
///
/// 1. **Types**: All records, variants, enums, and flags become Rust types
/// 2. **Import modules**: Each imported interface becomes a module with typed functions
/// 3. **Export metadata**: Information for `#[export]` to validate signatures
#[proc_macro]
pub fn world(input: TokenStream) -> TokenStream {
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
        // Use inline content
        input_str
    };

    // Parse the full WIT registry
    let registry = match wit_parser::parse_wit(&wit_content) {
        Ok(r) => r,
        Err(e) => {
            return syn::Error::new(
                proc_macro2::Span::call_site(),
                format!("Failed to parse WIT: {}", e)
            ).to_compile_error().into();
        }
    };

    // Get the first world (or error if none)
    let world = match registry.worlds.first() {
        Some(w) => w,
        None => {
            return syn::Error::new(
                proc_macro2::Span::call_site(),
                "No world definition found in WIT files"
            ).to_compile_error().into();
        }
    };

    // Generate types from the world
    let types = codegen::generate_world_types(world);

    // Generate types from top-level definitions in the registry
    let registry_types: Vec<_> = registry.types.iter()
        .map(|t| codegen::generate_type_def(t))
        .collect();

    // Generate import modules
    let imports = codegen::generate_imports(&registry, world);

    // Generate export metadata
    let export_metadata = codegen::generate_export_metadata(&registry, world);

    quote::quote! {
        #(#registry_types)*
        #types
        #imports
        #export_metadata
    }.into()
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

/// Arguments for the #[import_from] attribute - just a package name.
struct ImportFromArgs {
    /// Package name to import from
    package: String,
    /// Optional function name override
    name: Option<String>,
}

impl Parse for ImportFromArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        // First argument is the package name (required)
        let package: LitStr = input.parse()?;
        let mut name = None;

        // Optional: , name = "custom_name"
        if input.peek(Token![,]) {
            input.parse::<Token![,]>()?;
            let ident: Ident = input.parse()?;
            if ident != "name" {
                return Err(syn::Error::new(
                    ident.span(),
                    format!("unexpected attribute `{}`, expected `name`", ident),
                ));
            }
            input.parse::<Token![=]>()?;
            let lit: LitStr = input.parse()?;
            name = Some(lit.value());
        }

        Ok(ImportFromArgs {
            package: package.value(),
            name,
        })
    }
}

/// Import a function from another package in a composition.
///
/// This macro generates a wrapper function for calling functions exported by
/// other packages when using `CompositionBuilder` to wire packages together.
///
/// Unlike `#[import]` which imports from the host runtime, `#[import_from]`
/// imports from another composed package.
///
/// # Example
///
/// ```ignore
/// use pack_guest::{import_from, export, Value};
///
/// // Import the "double" function from the "math" package
/// #[import_from("math")]
/// fn double(n: i64) -> i64;
///
/// // Use it in an export
/// #[export]
/// fn process(input: Value) -> Value {
///     let n: i64 = input.try_into().unwrap();
///     let doubled = double(n);
///     Value::from(doubled + 1)
/// }
/// ```
///
/// # With Custom Function Name
///
/// ```ignore
/// // Import "transform" from "math" but call it "double" locally
/// #[import_from("math", name = "transform")]
/// fn double(n: i64) -> i64;
/// ```
///
/// # How It Works
///
/// When you use `CompositionBuilder::wire()`:
/// ```ignore
/// CompositionBuilder::new()
///     .add_package("adder", adder_wasm)
///     .add_package("math", math_wasm)
///     .wire("adder", "math", "double", "math", "double")
///     .build()?;
/// ```
///
/// The composition wires `adder`'s import of `math::double` to `math`'s export.
/// The `#[import_from("math")]` macro generates the import with module name "math"
/// that the composition system can satisfy.
#[proc_macro_attribute]
pub fn import_from(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attr as ImportFromArgs);
    let sig = parse_macro_input!(item as ImportFnSignature);

    let package = &args.package;
    let import_name = args.name.unwrap_or_else(|| sig.fn_name.to_string());

    let fn_name = &sig.fn_name;
    let fn_vis = &sig.vis;
    let output = &sig.output;

    // Generate a unique name for the raw import
    let raw_fn_name = Ident::new(
        &format!("__raw_pkg_import_{}", fn_name),
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
        quote! { pack_guest::Value::Tuple(pack_guest::__alloc::vec![]) }
    } else if param_names.len() == 1 {
        let name = &param_names[0];
        quote! { pack_guest::Value::from(#name) }
    } else {
        let conversions = param_names.iter().map(|name| {
            quote! { pack_guest::Value::from(#name) }
        });
        quote! {
            pack_guest::Value::Tuple(pack_guest::__alloc::vec![#(#conversions),*])
        }
    };

    // Build the return value handling
    let return_handling = if has_return {
        quote! {
            match result.try_into() {
                Ok(v) => v,
                Err(_) => panic!("failed to convert import result from package '{}'", #package),
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
        #[link(wasm_import_module = #package)]
        extern "C" {
            #[link_name = #import_name]
            fn #raw_fn_name(in_ptr: i32, in_len: i32, out_ptr: i32, out_cap: i32) -> i32;
        }

        #fn_vis fn #fn_name(#(#fn_params),*) -> #return_type {
            let input = #input_construction;
            let result = pack_guest::__import_impl(
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

/// Embed type metadata describing this package's imports and exports.
///
/// This macro generates a static byte array containing CGRF-encoded metadata
/// and a `__pack_types` export function that returns a pointer to it.
///
/// # Syntax
///
/// ```ignore
/// pack_guest::pack_types! {
///     exports {
///         echo: func(input: value) -> value,
///         transform: func(input: value) -> value,
///     }
/// }
/// ```
///
/// With imports:
///
/// ```ignore
/// pack_guest::pack_types! {
///     imports {
///         math {
///             double: func(n: s64) -> s64,
///         }
///     }
///     exports {
///         process: func(input: value) -> value,
///     }
/// }
/// ```
#[proc_macro]
pub fn pack_types(input: TokenStream) -> TokenStream {
    let input_str = input.to_string();

    match parse_and_encode_metadata(&input_str) {
        Ok(bytes) => {
            let byte_literals: Vec<proc_macro2::TokenStream> = bytes
                .iter()
                .map(|b| {
                    let lit = proc_macro2::Literal::u8_suffixed(*b);
                    quote! { #lit }
                })
                .collect();
            let len = bytes.len();

            let expanded = quote! {
                #[doc(hidden)]
                static __PACK_TYPES_DATA: [u8; #len] = [#(#byte_literals),*];

                #[no_mangle]
                pub extern "C" fn __pack_types(out_ptr_ptr: i32, out_len_ptr: i32) -> i32 {
                    unsafe {
                        core::ptr::write(out_ptr_ptr as *mut i32, __PACK_TYPES_DATA.as_ptr() as i32);
                        core::ptr::write(out_len_ptr as *mut i32, __PACK_TYPES_DATA.len() as i32);
                    }
                    0
                }
            };

            expanded.into()
        }
        Err(e) => syn::Error::new(proc_macro2::Span::call_site(), e)
            .to_compile_error()
            .into(),
    }
}

fn parse_and_encode_metadata(input: &str) -> Result<Vec<u8>, String> {
    let tokens = wit_parser::tokenize(input).map_err(|e| format!("tokenize error: {}", e))?;
    let mut parser = wit_parser::make_parser(tokens);

    let mut imports = Vec::new();
    let mut exports = Vec::new();

    while !parser.is_eof() {
        if parser.accept_ident("imports") {
            parser.expect_symbol('{').map_err(|e| e.to_string())?;
            parse_import_sigs(&mut parser, &mut imports)?;
            parser.expect_symbol('}').map_err(|e| e.to_string())?;
        } else if parser.accept_ident("exports") {
            parser.expect_symbol('{').map_err(|e| e.to_string())?;
            parse_func_sigs_into(&mut parser, "", &mut exports)?;
            parser.expect_symbol('}').map_err(|e| e.to_string())?;
        } else {
            return Err("expected 'imports' or 'exports'".into());
        }
    }

    Ok(metadata::encode_metadata(&imports, &exports))
}

/// Parse an interface path like "theater:simple/runtime" or just "math".
/// Collects identifiers and the symbols `:` and `/` until it hits a `{`.
fn parse_interface_path(parser: &mut wit_parser::Parser) -> Result<String, String> {
    let mut path = parser.expect_ident().map_err(|e| e.to_string())?;

    // Continue collecting path components: namespace:package/interface
    loop {
        if parser.accept_symbol(':') {
            path.push(':');
            path.push_str(&parser.expect_ident().map_err(|e| e.to_string())?);
        } else if parser.accept_symbol('/') {
            path.push('/');
            path.push_str(&parser.expect_ident().map_err(|e| e.to_string())?);
        } else {
            break;
        }
    }

    Ok(path)
}

fn parse_import_sigs(
    parser: &mut wit_parser::Parser,
    sigs: &mut Vec<metadata::FuncSig>,
) -> Result<(), String> {
    while !parser.peek_is_symbol('}') && !parser.is_eof() {
        let iface_name = parse_interface_path(parser)?;
        parser.expect_symbol('{').map_err(|e| e.to_string())?;
        parse_func_sigs_into(parser, &iface_name, sigs)?;
        parser.expect_symbol('}').map_err(|e| e.to_string())?;
        parser.accept_symbol(',');
    }
    Ok(())
}

/// Parse a full function path like "theater:simple/actor.init" and return (interface, name).
/// If there's no dot, returns (default_interface, full_path).
///
/// Handles the tricky case where "name: func" needs to NOT consume the colon,
/// but "namespace:package/interface.name" SHOULD consume the colon as part of the path.
fn parse_function_path(parser: &mut wit_parser::Parser, default_interface: &str) -> Result<(String, String), String> {
    let mut path = parser.expect_ident().map_err(|e| e.to_string())?;

    // Continue collecting path components: namespace:package/interface.funcname
    // But be careful: "name: func" should NOT consume the colon!
    // We peek ahead to see if the colon is followed by an identifier that's not "func"
    loop {
        if parser.peek_is_symbol(':') {
            // Peek at what comes after the colon
            // If it's "func", this colon is the separator, not part of the path
            if parser.peek_n_is_ident(1, "func") {
                break;
            }
            // It's part of the path
            parser.accept_symbol(':');
            path.push(':');
            path.push_str(&parser.expect_ident().map_err(|e| e.to_string())?);
        } else if parser.accept_symbol('/') {
            path.push('/');
            path.push_str(&parser.expect_ident().map_err(|e| e.to_string())?);
        } else if parser.accept_symbol('.') {
            // The dot separates interface from function name
            let func_name = parser.expect_ident().map_err(|e| e.to_string())?;
            return Ok((path, func_name));
        } else {
            break;
        }
    }

    // No dot found, use the whole thing as the function name
    Ok((default_interface.to_string(), path))
}

fn parse_func_sigs_into(
    parser: &mut wit_parser::Parser,
    interface: &str,
    sigs: &mut Vec<metadata::FuncSig>,
) -> Result<(), String> {
    while !parser.peek_is_symbol('}') && !parser.is_eof() {
        let (iface, name) = parse_function_path(parser, interface)?;
        parser.expect_symbol(':').map_err(|e| e.to_string())?;
        parser.accept_ident("func");

        let func =
            wit_parser::parse_func_signature(parser, name).map_err(|e| e.to_string())?;

        let params: Vec<(String, metadata::TypeDesc)> = func
            .params
            .iter()
            .map(|(n, t)| (n.clone(), metadata::wit_type_to_type_desc(t, &[])))
            .collect();

        let results: Vec<metadata::TypeDesc> = func
            .results
            .iter()
            .map(|t| metadata::wit_type_to_type_desc(t, &[]))
            .collect();

        sigs.push(metadata::FuncSig {
            interface: iface,
            name: func.name,
            params,
            results,
        });

        parser.accept_symbol(',');
        parser.accept_symbol(';');
    }
    Ok(())
}
