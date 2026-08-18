//! Proc macros for Pack guest packages.
//!
//! Provides the `#[export]` and `#[import]` attribute macros for easily
//! exporting and importing functions with the correct WASM calling convention.
//!
//! Also provides the `pact!()` macro (formerly `pact!`, now a deprecated alias)
//! for generating types from Pact (Pact) definitions.

use proc_macro::TokenStream;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{parse_macro_input, FnArg, Ident, ItemFn, LitStr, Pat, ReturnType, Token};

mod codegen;
mod metadata;
mod pact_parser;

/// Arguments for the #[export] attribute.
struct ExportArgs {
    /// Custom export name (e.g., "theater:simple/actor.init")
    name: Option<String>,
    /// Pact function name to validate/match against (e.g., "init")
    pact: Option<String>,
    /// State type for Theater actor functions (e.g., "MyState")
    /// When set, the first parameter is treated as state that gets automatically
    /// extracted from Option<Value> and the return tuple's first element is
    /// wrapped back as new state.
    state: Option<String>,
}

impl Parse for ExportArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut args = ExportArgs {
            name: None,
            pact: None,
            state: None,
        };

        if input.is_empty() {
            return Ok(args);
        }

        while !input.is_empty() {
            let ident: syn::Ident = input.parse()?;
            input.parse::<Token![=]>()?;
            let lit: LitStr = input.parse()?;

            match ident.to_string().as_str() {
                "name" => args.name = Some(lit.value()),
                "pact" => args.pact = Some(lit.value()),
                "state" => args.state = Some(lit.value()),
                other => {
                    return Err(syn::Error::new(
                        ident.span(),
                        format!(
                            "unexpected attribute `{}`, expected `name`, `pact`, or `state`",
                            other
                        ),
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
/// **Typed mode** (with `pact` attribute and typed parameters): The macro automatically
/// extracts typed parameters from the input and wraps the result. Parameters must
/// implement `TryFrom<Value>` and return type must implement `Into<Value>`.
///
/// **State mode** (with `state` attribute): For Theater actors, the macro handles
/// state extraction and wrapping automatically. The first parameter is the state type,
/// and the return must be `Result<(StateType, Output), Error>`. The macro extracts
/// state from a `Value`, passes it to your function, and returns the new state
/// back to the runtime.
///
/// # Example
///
/// ```ignore
/// use packr_guest::export;
/// use packr_guest::Value;
///
/// // Value mode - raw Value handling
/// #[export]
/// fn echo(input: Value) -> Value {
///     input
/// }
///
/// // Typed mode with Pact validation
/// #[export(pact = "my:package/geo.translate")]
/// fn translate(p: Point, dx: i32, dy: i32) -> Point {
///     Point { x: p.x + dx, y: p.y + dy }
/// }
///
/// // State mode for Theater actors - state automatically extracted/wrapped
/// #[derive(Clone, Default, IntoValue, FromValue)]
/// struct MyState { count: i32 }
///
/// #[export(name = "theater:simple/actor.init", state = "MyState")]
/// fn init(state: MyState) -> Result<(MyState, ()), String> {
///     Ok((state, ()))
/// }
///
/// #[export(name = "theater:simple/counter.increment", state = "MyState")]
/// fn increment(state: MyState, amount: i32) -> Result<(MyState, i32), String> {
///     let mut new_state = state;
///     new_state.count += amount;
///     Ok((new_state, new_state.count))
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

    // Try to derive export name from Pact:
    // 1. If pact attribute is explicitly provided, use it
    // 2. Otherwise, try to find the function in the world automatically
    let derived_export_name = if let Some(ref pact_path) = args.pact {
        // Explicit pact path provided
        match validate_export_against_pact(pact_path) {
            Ok(result) => result.derived_name,
            Err(e) => {
                return syn::Error::new(proc_macro2::Span::call_site(), e)
                    .to_compile_error()
                    .into();
            }
        }
    } else {
        // Try auto-discovery: look up function by name in world exports
        try_auto_discover_export(&fn_name_str)
    };

    // Determine the export name: explicit name > derived from pact > function name
    let export_name = args.name.clone().or(derived_export_name);

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
                            "parameter must be a simple identifier",
                        )
                        .to_compile_error()
                        .into();
                    }
                };
                param_names.push(name);
                param_types.push((*pat_type.ty).clone());
            }
            FnArg::Receiver(_) => {
                return syn::Error::new_spanned(
                    param,
                    "exported functions cannot have self parameter",
                )
                .to_compile_error()
                .into();
            }
        }
    }

    // Detect if this is "Value mode" (single Value parameter) or "Typed mode"
    let is_value_mode = param_names.len() == 1 && {
        // Check if the type is `Value` (simple path check)
        let ty = &param_types[0];
        if let syn::Type::Path(type_path) = ty {
            type_path
                .path
                .segments
                .last()
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
                "exported functions must have a return type",
            )
            .to_compile_error()
            .into();
        }
        ReturnType::Type(_, ty) => ty,
    };

    // Generate the inner function name (prefixed with underscore)
    let inner_fn_name = syn::Ident::new(&format!("__{}_inner", fn_name), fn_name.span());

    // Generate the wrapper function name (always a valid Rust identifier)
    let wrapper_fn_name = syn::Ident::new(&format!("__{}_export", fn_name), fn_name.span());

    // Generate the function parameters for the inner function declaration
    let inner_fn_params = param_names
        .iter()
        .zip(param_types.iter())
        .map(|(name, ty)| {
            quote! { #name: #ty }
        });

    // Generate the parameter extraction and function call based on mode
    let call_body = if args.state.is_some() {
        // State mode for Theater actors
        // Input: Tuple([state_value, params...])
        // Output: Result<Tuple([new_state, output]), error>

        if param_names.is_empty() {
            return syn::Error::new_spanned(
                &input_fn.sig,
                "state mode requires at least one parameter (the state)",
            )
            .to_compile_error()
            .into();
        }

        let state_name = &param_names[0];
        let state_type = &param_types[0];

        // Remaining parameters (after state)
        let other_param_names: Vec<_> = param_names.iter().skip(1).collect();
        let other_param_types: Vec<_> = param_types.iter().skip(1).collect();

        // Generate extraction for other parameters
        let param_extractions = if other_param_names.is_empty() {
            quote! {}
        } else if other_param_names.len() == 1 {
            let name = &other_param_names[0];
            let ty = &other_param_types[0];
            quote! {
                // For single param, unwrap from tuple if needed
                let #name: #ty = match params_value {
                    packr_guest::Value::Tuple(mut items) if items.len() == 1 => {
                        match items.remove(0).try_into() {
                            Ok(v) => v,
                            Err(_) => return Err("failed to convert parameter"),
                        }
                    },
                    other => {
                        match other.try_into() {
                            Ok(v) => v,
                            Err(_) => return Err("failed to convert parameter"),
                        }
                    }
                };
            }
        } else {
            let indices: Vec<_> = (0..other_param_names.len()).collect();
            let extractions = other_param_names
                .iter()
                .zip(other_param_types.iter())
                .zip(indices.iter())
                .map(|((name, ty), idx)| {
                    quote! {
                        let #name: #ty = match param_items.get(#idx).cloned() {
                            Some(v) => match v.try_into() {
                                Ok(converted) => converted,
                                Err(_) => return Err("failed to convert parameter"),
                            },
                            None => return Err("missing parameter in tuple"),
                        };
                    }
                });
            quote! {
                let param_items = match params_value {
                    packr_guest::Value::Tuple(items) => items,
                    _ => return Err("expected tuple of parameters"),
                };
                #(#extractions)*
            }
        };

        let call_args = param_names.iter();

        quote! {
            // State mode: Input is Tuple([state, params])
            let (state_opt, params_value) = match value {
                packr_guest::Value::Tuple(mut items) if items.len() >= 1 => {
                    let state_opt = items.remove(0);
                    let params = if items.len() == 1 {
                        items.remove(0)
                    } else if items.is_empty() {
                        packr_guest::Value::Tuple(packr_guest::__alloc::vec![])
                    } else {
                        packr_guest::Value::Tuple(items)
                    };
                    (state_opt, params)
                },
                _ => return Err("expected tuple with state and params"),
            };

            // Extract state from Value
            let #state_name: #state_type = match state_opt.try_into() {
                Ok(v) => v,
                Err(_) => return Err("failed to convert state"),
            };

            // Extract other parameters
            #param_extractions

            // Call user's function
            let result = #inner_fn_name(#(#call_args),*);

            // Handle Result: convert (NewState, Output) to proper Value format
            match result {
                Ok((new_state, output)) => {
                    let state_value: packr_guest::Value = new_state.into();
                    let output_value: packr_guest::Value = output.into();

                    // Return Result<Tuple([state, output]), _>
                    Ok(packr_guest::Value::Result {
                        ok_type: packr_guest::ValueType::Tuple(packr_guest::__alloc::vec![
                            packr_guest::ValueType::Bool,
                            packr_guest::ValueType::Bool,
                        ]),
                        err_type: packr_guest::ValueType::String,
                        value: Ok(packr_guest::__alloc::boxed::Box::new(
                            packr_guest::Value::Tuple(packr_guest::__alloc::vec![state_value, output_value])
                        )),
                    })
                },
                Err(e) => {
                    // Return error
                    Ok(packr_guest::Value::Result {
                        ok_type: packr_guest::ValueType::Bool,
                        err_type: packr_guest::ValueType::String,
                        value: Err(packr_guest::__alloc::boxed::Box::new(
                            packr_guest::Value::String(e.into())
                        )),
                    })
                }
            }
        }
    } else if is_value_mode {
        // Value mode - extract from tuple like other params
        let param_name = &param_names[0];
        let param_type = &param_types[0];
        quote! {
            // Extract single Value parameter from tuple
            let #param_name: #param_type = match value {
                packr_guest::Value::Tuple(mut items) if items.len() == 1 => items.remove(0),
                other => other,
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
        // Single typed parameter. Theater always wraps inputs in a Tuple, so
        // a 1-param function receives Tuple([arg]) — unwrap before converting.
        // Fall back to a direct try_into() for callers that pass the value
        // unwrapped.
        let param_name = &param_names[0];
        let param_type = &param_types[0];
        quote! {
            let #param_name: #param_type = match value {
                packr_guest::Value::Tuple(mut items) if items.len() == 1 => {
                    match items.remove(0).try_into() {
                        Ok(v) => v,
                        Err(_) => return Err("failed to convert parameter"),
                    }
                }
                other => match other.try_into() {
                    Ok(v) => v,
                    Err(_) => return Err("failed to convert parameter"),
                },
            };

            let output = #inner_fn_name(#param_name);
            Ok(output.into())
        }
    } else {
        // Multiple typed parameters - extract from tuple
        let num_params = param_names.len();
        let indices: Vec<_> = (0..num_params).collect();

        let extractions = param_names
            .iter()
            .zip(param_types.iter())
            .zip(indices.iter())
            .map(|((name, ty), idx)| {
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
                packr_guest::Value::Tuple(items) => items,
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
                    packr_guest::__export_impl(
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
                    packr_guest::__export_impl(
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

/// Result of validating an export against Pact
#[allow(dead_code)]
struct PactValidationResult {
    /// The derived export name (from the Pact path)
    pub derived_name: Option<String>,
    /// The Pact function signature (params and results)
    pub function: Option<pact_parser::Function>,
}

/// Validate that a function exists in the Pact and optionally derive the export name.
///
/// The `pact_path` can be:
/// - A simple function name: "init" (searches all exports)
/// - A full path: "theater:simple/actor.init" (looks up specific interface)
fn validate_export_against_pact(pact_path: &str) -> Result<PactValidationResult, String> {
    // Read and parse Pact files
    let pact_content = read_pact_files()?;
    let registry = pact_parser::parse_pact(&pact_content)
        .map_err(|e| format!("Failed to parse Pact: {}", e))?;

    // Check if this is a full path (contains '.' or '#')
    if let Some(func_path) = pact_parser::FunctionPath::parse(pact_path) {
        // Full path specified - look up the specific function
        if let Some(func) = registry.find_function(&func_path) {
            return Ok(PactValidationResult {
                derived_name: Some(func_path.export_name()),
                function: Some(func.clone()),
            });
        }

        // Not found - provide helpful error
        let available = registry.available_exports();
        return Err(format!(
            "Function '{}' not found in Pact interfaces. Available: {:?}",
            pact_path, available
        ));
    }

    // Simple function name - search in exports and interfaces
    let func_name = pact_path;

    // First, check world exports
    for world in &registry.worlds {
        for export in &world.exports {
            match export {
                pact_parser::WorldItem::Function(f) if f.name == func_name => {
                    // Found as a bare export
                    return Ok(PactValidationResult {
                        derived_name: Some(func_name.to_string()),
                        function: Some(f.clone()),
                    });
                }
                pact_parser::WorldItem::InlineInterface {
                    name: iface_name,
                    functions,
                } => {
                    if let Some(f) = functions.iter().find(|f| f.name == func_name) {
                        // Found in inline interface
                        return Ok(PactValidationResult {
                            derived_name: Some(format!("{}.{}", iface_name, func_name)),
                            function: Some(f.clone()),
                        });
                    }
                }
                pact_parser::WorldItem::InterfacePath {
                    namespace,
                    package,
                    interface,
                } => {
                    // Check if this interface path is in our registry
                    let iface_path = match (namespace, package) {
                        (Some(ns), Some(pkg)) => format!("{}:{}/{}", ns, pkg, interface),
                        (None, Some(pkg)) => format!("{}/{}", pkg, interface),
                        _ => interface.clone(),
                    };

                    if let Some(iface) = registry.interfaces.get(&iface_path) {
                        if let Some(f) = iface.functions.iter().find(|f| f.name == func_name) {
                            // Found in referenced interface
                            return Ok(PactValidationResult {
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
            return Ok(PactValidationResult {
                derived_name: Some(format!("{}.{}", path, func_name)),
                function: Some(f.clone()),
            });
        }
    }

    // Not found
    let available = registry.available_exports();
    Err(format!(
        "Function '{}' not found in Pact exports. Available: {:?}",
        func_name, available
    ))
}

/// Try to auto-discover the export name for a function by looking it up in the world.
///
/// This is a "best effort" lookup - it returns None if:
/// - No Pact files are found
/// - No world is defined
/// - The function is not found in exports
///
/// This allows the macro to work both with and without a Pact world definition.
fn try_auto_discover_export(fn_name: &str) -> Option<String> {
    // Try to read Pact files, but don't error if not found
    let pact_content = match read_pact_files() {
        Ok(c) => c,
        Err(_) => return None,
    };

    // Try to parse the Pact, but don't error on failure
    let registry = match pact_parser::parse_pact(&pact_content) {
        Ok(r) => r,
        Err(_) => return None,
    };

    // Search for the function in world exports
    for world in &registry.worlds {
        for export in &world.exports {
            match export {
                pact_parser::WorldItem::Function(f) if f.name == fn_name => {
                    // Found as a bare export - use just the function name
                    return Some(fn_name.to_string());
                }
                pact_parser::WorldItem::InlineInterface {
                    name: iface_name,
                    functions,
                } => {
                    if functions.iter().any(|f| f.name == fn_name) {
                        // Found in inline interface - use interface.function format
                        return Some(format!("{}.{}", iface_name, fn_name));
                    }
                }
                pact_parser::WorldItem::InterfacePath {
                    namespace,
                    package,
                    interface,
                } => {
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

/// Result of validating an import against Pact
struct PactImportValidationResult {
    /// The derived module name (interface path)
    pub module: Option<String>,
    /// The derived import name (function name)
    pub import_name: Option<String>,
}

/// Validate that a function exists in the Pact imports and derive module/name.
///
/// The `pact_path` should be a full path like "theater:simple/runtime.log"
fn validate_import_against_pact(pact_path: &str) -> Result<PactImportValidationResult, String> {
    // Read and parse Pact files
    let pact_content = read_pact_files()?;
    let registry = pact_parser::parse_pact(&pact_content)
        .map_err(|e| format!("Failed to parse Pact: {}", e))?;

    // Parse the function path
    let func_path = pact_parser::FunctionPath::parse(pact_path).ok_or_else(|| {
        format!(
            "Invalid Pact path '{}'. Expected format: 'namespace:package/interface.function'",
            pact_path
        )
    })?;

    // Look up the function in the registry
    if registry.find_import_function(&func_path).is_some() {
        return Ok(PactImportValidationResult {
            module: Some(func_path.interface.to_string()),
            import_name: Some(func_path.function),
        });
    }

    // Also check if the function exists in any interface (even if not explicitly imported)
    if registry.find_function(&func_path).is_some() {
        return Ok(PactImportValidationResult {
            module: Some(func_path.interface.to_string()),
            import_name: Some(func_path.function),
        });
    }

    // Not found - provide helpful error
    let available = registry.available_imports();
    Err(format!(
        "Function '{}' not found in Pact interfaces. Available imports: {:?}",
        pact_path, available
    ))
}

/// Arguments for the #[import] attribute.
struct ImportArgs {
    /// Module name (e.g., "theater:simple/runtime")
    module: Option<String>,
    /// Function name override
    name: Option<String>,
    /// Pact path for validation and auto-derivation (e.g., "theater:simple/runtime.log")
    pact: Option<String>,
}

impl Parse for ImportArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut module = None;
        let mut name = None;
        let mut pact = None;

        while !input.is_empty() {
            let ident: Ident = input.parse()?;
            input.parse::<Token![=]>()?;
            let lit: LitStr = input.parse()?;

            match ident.to_string().as_str() {
                "module" => module = Some(lit.value()),
                "name" => name = Some(lit.value()),
                "pact" => pact = Some(lit.value()),
                other => {
                    return Err(syn::Error::new(
                        ident.span(),
                        format!(
                            "unexpected attribute `{}`, expected `module`, `name`, or `pact`",
                            other
                        ),
                    ));
                }
            }

            // Consume optional comma
            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
        }

        // Either module or pact must be specified
        if module.is_none() && pact.is_none() {
            return Err(syn::Error::new(
                input.span(),
                "either `module` or `pact` attribute is required",
            ));
        }

        Ok(ImportArgs { module, name, pact })
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
/// use packr_guest::import;
///
/// // Import a log function from the host (manual module specification)
/// #[import(module = "theater:simple/runtime")]
/// fn log(msg: String);
///
/// // Import with Pact path - module and name derived automatically
/// #[import(pact = "theater:simple/runtime.log")]
/// fn log(msg: String);
///
/// // Import with a custom function name
/// #[import(module = "theater:simple/runtime", name = "log")]
/// fn my_log(msg: String);
///
/// // Import a function that returns a value
/// #[import(pact = "theater:simple/runtime.get-chain")]
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

    // If pact attribute is provided, validate and derive module/name
    let (derived_module, derived_name) = if let Some(ref pact_path) = args.pact {
        match validate_import_against_pact(pact_path) {
            Ok(result) => (result.module, result.import_name),
            Err(e) => {
                return syn::Error::new(proc_macro2::Span::call_site(), e)
                    .to_compile_error()
                    .into();
            }
        }
    } else {
        (None, None)
    };

    // Determine module: explicit > derived from pact
    let module = args
        .module
        .clone()
        .or(derived_module)
        .expect("module should be set by either `module` or `pact` attribute");

    // Determine import name: explicit > derived from pact > function name
    let import_name = args
        .name
        .clone()
        .or(derived_name)
        .unwrap_or_else(|| sig.fn_name.to_string());

    let fn_name = &sig.fn_name;
    let fn_vis = &sig.vis;
    let output = &sig.output;

    // Generate a unique name for the raw import
    let raw_fn_name = Ident::new(&format!("__raw_import_{}", fn_name), fn_name.span());

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
                            "parameter must be a simple identifier",
                        )
                        .to_compile_error()
                        .into();
                    }
                };
                param_names.push(name.clone());
                param_types.push((*pat_type.ty).clone());
            }
            FnArg::Receiver(_) => {
                return syn::Error::new_spanned(
                    param,
                    "imported functions cannot have self parameter",
                )
                .to_compile_error()
                .into();
            }
        }
    }

    // Determine return type handling. When the import is `-> ()` (default),
    // emit no `-> X` and no trailing unit expression — both trigger
    // `clippy::unused_unit` in user code.
    let (return_clause, has_return) = match output {
        ReturnType::Default => (quote! {}, false),
        ReturnType::Type(_, ty) => (quote! { -> #ty }, true),
    };

    // Build the input value - tuple of all parameters
    let input_construction = if param_names.is_empty() {
        quote! { packr_guest::Value::Tuple(packr_guest::__alloc::vec![]) }
    } else if param_names.len() == 1 {
        let name = &param_names[0];
        quote! { packr_guest::Value::from(#name) }
    } else {
        let conversions = param_names.iter().map(|name| {
            quote! { packr_guest::Value::from(#name) }
        });
        quote! {
            packr_guest::Value::Tuple(packr_guest::__alloc::vec![#(#conversions),*])
        }
    };

    // Build the body. When `has_return`, decode the result; otherwise drop it.
    // Use FromValue::from_value() to support nested Option/Result types.
    let body = if has_return {
        quote! {
            let input = #input_construction;
            let result = packr_guest::__import_impl(
                |in_ptr, in_len, out_ptr, out_cap| unsafe {
                    #raw_fn_name(in_ptr, in_len, out_ptr, out_cap)
                },
                input,
            );
            match packr_guest::FromValue::from_value(result) {
                Ok(v) => v,
                Err(_) => panic!("failed to convert import result"),
            }
        }
    } else {
        quote! {
            let input = #input_construction;
            packr_guest::__import_impl(
                |in_ptr, in_len, out_ptr, out_cap| unsafe {
                    #raw_fn_name(in_ptr, in_len, out_ptr, out_cap)
                },
                input,
            );
        }
    };

    // Generate the function signature parameters
    let fn_params = param_names
        .iter()
        .zip(param_types.iter())
        .map(|(name, ty)| {
            quote! { #name: #ty }
        });

    let expanded = quote! {
        #[link(wasm_import_module = #module)]
        extern "C" {
            #[link_name = #import_name]
            fn #raw_fn_name(in_ptr: i32, in_len: i32, out_ptr: i32, out_cap: i32) -> i32;
        }

        #fn_vis fn #fn_name(#(#fn_params),*) #return_clause {
            #body
        }
    };

    expanded.into()
}

/// Generate types and bindings from a Pact (Pact) definition.
///
/// `pact!` reads a Pact definition — inline, from a shared file, or from the
/// `pact/` directory — and generates:
/// - Rust types for all type definitions (records, variants, enums, flags)
/// - `From<T> for Value` / `TryFrom<Value> for T` implementations
///
/// # Inline
///
/// ```ignore
/// packr_guest::pact! {
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
///
/// # From a shared file
///
/// Point the macro at a specific file so several crates can share ONE
/// definition instead of copying (or symlinking) it into each repo:
///
/// ```ignore
/// packr_guest::pact!(from "../shared/api.pact");   // or: pact!("../shared/api.pact")
/// ```
///
/// A relative path resolves against `CARGO_MANIFEST_DIR` (the crate root); an
/// absolute path is used as-is. The file is registered as a build dependency,
/// so editing the shared definition triggers a rebuild.
///
/// # From the `pact/` directory
///
/// With no argument, `pact!()` reads every `.pact` file under `pact/`.
///
/// # Cross-file imports
///
/// A Pact file can pull type definitions from another Pact file with a
/// path-based `use`, so a type is single-sourced instead of hand-mirrored:
///
/// ```ignore
/// // consumer.pact
/// use "../shared.pact".{msg, chat-state};
/// record snapshot { latest: chat-state, last-msg: msg }
/// world consumer { export snap: func(s: snapshot) -> snapshot }
/// ```
///
/// The path resolves relative to the importing file's directory (relative to
/// `CARGO_MANIFEST_DIR` for inline/`pact/`-dir input). The named types — plus
/// their transitive same-file dependencies — are pulled in and generated
/// locally. Every `use`d file is registered as a build dependency, so editing it
/// triggers a rebuild.
///
/// # Record annotations
///
/// A `record`/`variant` can carry `@`-prefixed codegen annotations:
///
/// ```pact
/// @forward-compatible
/// @default
/// record chat-state { members: set<tuple<list<u8>, list<u8>>>, log: list<message> }
/// ```
///
/// - `@forward-compatible` emits `#[graph(…, forward_compatible)]` — tolerant
///   decode (a missing field defaults, an extra one is ignored), so old
///   persisted bytes still decode after a field is added.
/// - `@default` adds `Default` to the generated derive (records only).
#[proc_macro]
pub fn pact(input: TokenStream) -> TokenStream {
    expand_pact(input)
}

/// Shared implementation behind [`pact!`]: resolve the Pact source (a specific
/// file, inline content, or the `pact/` directory), parse it, and generate the
/// world's types.
fn expand_pact(input: TokenStream) -> TokenStream {
    // Form 1: `pact!(from "path")` / `pact!("path")` — read a specific file so
    // multiple crates can share one definition instead of symlinking a copy
    // into each repo.
    if let Ok(file_ref) = syn::parse2::<PactFileRef>(input.clone().into()) {
        let (content, abs_path) = match read_pact_file(&file_ref.path.value()) {
            Ok(v) => v,
            Err(e) => {
                return syn::Error::new(file_ref.path.span(), e)
                    .to_compile_error()
                    .into();
            }
        };
        let mut world = match pact_parser::parse_world(&content) {
            Ok(w) => w,
            Err(e) => {
                return syn::Error::new(
                    file_ref.path.span(),
                    format!("Failed to parse Pact from {}: {}", abs_path.display(), e),
                )
                .to_compile_error()
                .into();
            }
        };
        // Resolve `use "path".{names}` imports relative to THIS file's directory.
        let base_dir = abs_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        let used_paths = match resolve_world_uses(&mut world, &base_dir) {
            Ok(v) => v,
            Err(e) => {
                return syn::Error::new(file_ref.path.span(), e)
                    .to_compile_error()
                    .into();
            }
        };
        let generated = codegen::generate_world_types(&world);
        // Register the source file + every `use`d file as build dependencies so
        // editing any of them triggers recompilation of this crate.
        let mut trackers = vec![abs_path];
        trackers.extend(used_paths);
        let trackers = build_dep_trackers(&trackers);
        return quote! {
            #trackers
            #generated
        }
        .into();
    }

    // Check if we have inline content or should read from files
    let input_str = input.to_string();

    let pact_content = if input_str.trim().is_empty() {
        // Read from pact/ directory
        match read_pact_files() {
            Ok(content) => content,
            Err(e) => {
                return syn::Error::new(
                    proc_macro2::Span::call_site(),
                    format!("Failed to read Pact files: {}", e),
                )
                .to_compile_error()
                .into();
            }
        }
    } else {
        // Use inline content - parse the token stream as a raw string
        // The input is the raw Pact content between the braces
        input_str
    };

    // Parse the Pact content
    let mut world = match pact_parser::parse_world(&pact_content) {
        Ok(w) => w,
        Err(e) => {
            return syn::Error::new(
                proc_macro2::Span::call_site(),
                format!("Failed to parse Pact: {}", e),
            )
            .to_compile_error()
            .into();
        }
    };

    // Resolve `use "path".{names}` imports relative to the crate root.
    let base_dir = match std::env::var("CARGO_MANIFEST_DIR") {
        Ok(d) => std::path::PathBuf::from(d),
        Err(_) => std::path::PathBuf::from("."),
    };
    let used_paths = match resolve_world_uses(&mut world, &base_dir) {
        Ok(v) => v,
        Err(e) => {
            return syn::Error::new(proc_macro2::Span::call_site(), e)
                .to_compile_error()
                .into();
        }
    };

    // Generate the types
    let generated = codegen::generate_world_types(&world);
    let trackers = build_dep_trackers(&used_paths);

    quote! {
        #trackers
        #generated
    }
    .into()
}

/// Resolve a world's `use "path".{names}` imports: for each, read the file
/// (relative to `base_dir`), parse it, pull the named type defs plus their
/// transitive same-file dependencies, and append them to the world's types
/// (deduped by name). Returns the absolute paths read, for build-dep tracking.
fn resolve_world_uses(
    world: &mut pact_parser::World,
    base_dir: &std::path::Path,
) -> Result<Vec<std::path::PathBuf>, String> {
    let uses = world.uses.clone();
    let mut tracked = Vec::new();
    for u in &uses {
        let path = base_dir.join(&u.path);
        let content = std::fs::read_to_string(&path)
            .map_err(|e| format!("Failed to read `use` file {}: {}", path.display(), e))?;
        let registry = pact_parser::parse_pact(&content)
            .map_err(|e| format!("Failed to parse `use` file {}: {}", path.display(), e))?;
        // Types available in the used file: top-level defs + any world's defs.
        let mut avail = registry.types.clone();
        for w in &registry.worlds {
            avail.extend(w.types.iter().cloned());
        }
        let pulled = pact_parser::resolve_used_types(&u.items, &avail)
            .map_err(|e| format!("in `use \"{}\"`: {}", u.path, e))?;
        for td in pulled {
            if !world.types.iter().any(|t| t.name() == td.name()) {
                world.types.push(td);
            }
        }
        tracked.push(path);
    }
    Ok(tracked)
}

/// Build `const _: &[u8] = include_bytes!("<abs>");` entries for each path, so
/// editing a source or `use`d file triggers a rebuild.
fn build_dep_trackers(paths: &[std::path::PathBuf]) -> proc_macro2::TokenStream {
    let entries = paths.iter().map(|p| {
        let s = p.to_string_lossy().into_owned();
        quote! { const _: &[u8] = include_bytes!(#s); }
    });
    quote! { #(#entries)* }
}

/// A `pact!` invocation that points at an external definition file:
/// `pact!(from "path/to/api.pact")`, or the shorthand `pact!("path/to/api.pact")`.
///
/// This lets several crates share ONE Pact file instead of copying (or
/// symlinking) it into each repo. A relative path is resolved against
/// `CARGO_MANIFEST_DIR` (the crate root); an absolute path is used as-is.
struct PactFileRef {
    path: LitStr,
}

impl Parse for PactFileRef {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        // Optional leading `from` keyword (a plain ident, not a real Rust kw).
        // Inline Pact starts with `interface`/`world`/`record`/… so it fails this
        // parse and the caller falls back to treating the input as inline Pact.
        if input.peek(Ident) {
            let kw: Ident = input.parse()?;
            if kw != "from" {
                return Err(syn::Error::new(
                    kw.span(),
                    "expected `from` followed by a string literal, or a string literal",
                ));
            }
        }
        let path: LitStr = input.parse()?;
        if !input.is_empty() {
            return Err(input.error("unexpected tokens after the Pact file path"));
        }
        Ok(PactFileRef { path })
    }
}

/// Read a Pact definition from a specific file. Returns the file contents and
/// the resolved absolute path (so the caller can register it as a build
/// dependency). Relative paths resolve against `CARGO_MANIFEST_DIR`.
fn read_pact_file(path_str: &str) -> Result<(String, std::path::PathBuf), String> {
    let path = std::path::Path::new(path_str);
    let full = if path.is_absolute() {
        path.to_path_buf()
    } else {
        let manifest_dir =
            std::env::var("CARGO_MANIFEST_DIR").map_err(|_| "CARGO_MANIFEST_DIR not set")?;
        std::path::Path::new(&manifest_dir).join(path)
    };
    let content = std::fs::read_to_string(&full)
        .map_err(|e| format!("Failed to read Pact file {:?}: {}", full, e))?;
    Ok((content, full))
}

/// Parse the Pact world and generate types, imports, and export metadata.
///
/// This macro reads Pact files from the `pact/` directory in your crate and generates:
/// - Rust types for all type definitions (records, variants, enums, flags)
/// - Import modules with fully typed functions
/// - Export metadata for `#[export]` validation
///
/// # Usage
///
/// Create a `pact/` directory in your crate root with `.pact` files:
///
/// ```pact
/// // pact/world.pact
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
/// use packr_guest::export;
///
/// // Generate types, imports, and export metadata
/// packr_guest::world!();
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

    let pact_content = if input_str.trim().is_empty() {
        // Read from pact/ directory
        match read_pact_files() {
            Ok(content) => content,
            Err(e) => {
                return syn::Error::new(
                    proc_macro2::Span::call_site(),
                    format!("Failed to read Pact files: {}", e),
                )
                .to_compile_error()
                .into();
            }
        }
    } else {
        // Use inline content
        input_str
    };

    // Parse the full Pact registry
    let registry = match pact_parser::parse_pact(&pact_content) {
        Ok(r) => r,
        Err(e) => {
            return syn::Error::new(
                proc_macro2::Span::call_site(),
                format!("Failed to parse Pact: {}", e),
            )
            .to_compile_error()
            .into();
        }
    };

    // Get the first world (or error if none)
    let world = match registry.worlds.first() {
        Some(w) => w,
        None => {
            return syn::Error::new(
                proc_macro2::Span::call_site(),
                "No world definition found in Pact files",
            )
            .to_compile_error()
            .into();
        }
    };

    // Generate types from the world
    let types = codegen::generate_world_types(world);

    // Generate types from top-level definitions in the registry
    let registry_types: Vec<_> = registry
        .types
        .iter()
        .map(codegen::generate_type_def)
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
    }
    .into()
}

/// Read all Pact files from the pact/ directory and pact/deps/ subdirectories
fn read_pact_files() -> Result<String, String> {
    // Get the manifest directory (crate root)
    let manifest_dir =
        std::env::var("CARGO_MANIFEST_DIR").map_err(|_| "CARGO_MANIFEST_DIR not set")?;

    let pact_dir = std::path::Path::new(&manifest_dir).join("pact");

    if !pact_dir.exists() {
        return Err(format!("pact/ directory not found at {:?}", pact_dir));
    }

    let mut content = String::new();

    // Read Pact files recursively (includes pact/deps/)
    read_pact_files_recursive(&pact_dir, &mut content)?;

    if content.is_empty() {
        return Err("No .pact files found in pact/ directory".to_string());
    }

    Ok(content)
}

/// Recursively read Pact files from a directory
fn read_pact_files_recursive(dir: &std::path::Path, content: &mut String) -> Result<(), String> {
    let entries =
        std::fs::read_dir(dir).map_err(|e| format!("Failed to read directory {:?}: {}", dir, e))?;

    for entry in entries {
        let entry = entry.map_err(|e| format!("Failed to read directory entry: {}", e))?;
        let path = entry.path();

        if path.is_dir() {
            // Recurse into subdirectories (including deps/)
            read_pact_files_recursive(&path, content)?;
        } else if let Some(ext) = path.extension() {
            if ext == "pact" {
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
/// use packr_guest::{import_from, export, Value};
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
    let raw_fn_name = Ident::new(&format!("__raw_pkg_import_{}", fn_name), fn_name.span());

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
                            "parameter must be a simple identifier",
                        )
                        .to_compile_error()
                        .into();
                    }
                };
                param_names.push(name.clone());
                param_types.push((*pat_type.ty).clone());
            }
            FnArg::Receiver(_) => {
                return syn::Error::new_spanned(
                    param,
                    "imported functions cannot have self parameter",
                )
                .to_compile_error()
                .into();
            }
        }
    }

    // Determine return type handling. When the import is `-> ()` (default),
    // emit no `-> X` and no trailing unit expression — both trigger
    // `clippy::unused_unit` in user code.
    let (return_clause, has_return) = match output {
        ReturnType::Default => (quote! {}, false),
        ReturnType::Type(_, ty) => (quote! { -> #ty }, true),
    };

    // Build the input value - tuple of all parameters
    let input_construction = if param_names.is_empty() {
        quote! { packr_guest::Value::Tuple(packr_guest::__alloc::vec![]) }
    } else if param_names.len() == 1 {
        let name = &param_names[0];
        quote! { packr_guest::Value::from(#name) }
    } else {
        let conversions = param_names.iter().map(|name| {
            quote! { packr_guest::Value::from(#name) }
        });
        quote! {
            packr_guest::Value::Tuple(packr_guest::__alloc::vec![#(#conversions),*])
        }
    };

    // Build the body. When `has_return`, decode the result; otherwise drop it.
    // Use FromValue::from_value() (like `#[import]` and `#[export]` do) so nested
    // `Option`/`Result` return types decode — the composite ABI implements
    // `FromValue` for those but not `TryFrom<Value>`, so `try_into()` forced every
    // consumer of a `Result`-returning package function to write a newtype shim.
    let body = if has_return {
        quote! {
            let input = #input_construction;
            let result = packr_guest::__import_impl(
                |in_ptr, in_len, out_ptr, out_cap| unsafe {
                    #raw_fn_name(in_ptr, in_len, out_ptr, out_cap)
                },
                input,
            );
            match packr_guest::FromValue::from_value(result) {
                Ok(v) => v,
                Err(_) => panic!("failed to convert import result from package '{}'", #package),
            }
        }
    } else {
        quote! {
            let input = #input_construction;
            packr_guest::__import_impl(
                |in_ptr, in_len, out_ptr, out_cap| unsafe {
                    #raw_fn_name(in_ptr, in_len, out_ptr, out_cap)
                },
                input,
            );
        }
    };

    // Generate the function signature parameters
    let fn_params = param_names
        .iter()
        .zip(param_types.iter())
        .map(|(name, ty)| {
            quote! { #name: #ty }
        });

    let expanded = quote! {
        #[link(wasm_import_module = #package)]
        extern "C" {
            #[link_name = #import_name]
            fn #raw_fn_name(in_ptr: i32, in_len: i32, out_ptr: i32, out_cap: i32) -> i32;
        }

        #fn_vis fn #fn_name(#(#fn_params),*) #return_clause {
            #body
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
/// ## Inline syntax:
///
/// ```ignore
/// packr_guest::pack_types! {
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
/// packr_guest::pack_types! {
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
///
/// ## File-based syntax:
///
/// ```ignore
/// packr_guest::pack_types!(file = "actor.types");
/// ```
///
/// The file path is relative to the crate's `CARGO_MANIFEST_DIR`.
#[proc_macro]
pub fn pack_types(input: TokenStream) -> TokenStream {
    let input_str = input.to_string();

    // Check if this is a file reference: file = "path"
    let content = if input_str.trim().starts_with("file") {
        match parse_file_reference(&input_str) {
            Ok(c) => c,
            Err(e) => {
                return syn::Error::new(proc_macro2::Span::call_site(), e)
                    .to_compile_error()
                    .into();
            }
        }
    } else {
        input_str
    };

    match parse_and_encode_metadata(&content) {
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

/// Parse a file reference like `file = "path/to/file.types"` and read the file content.
fn parse_file_reference(input: &str) -> Result<String, String> {
    // Parse: file = "path"
    let input = input.trim();

    // Strip "file" prefix
    let rest = input
        .strip_prefix("file")
        .ok_or("expected 'file = \"path\"'")?
        .trim();

    // Strip "="
    let rest = rest
        .strip_prefix('=')
        .ok_or("expected '=' after 'file'")?
        .trim();

    // Strip quotes and get path
    let path = rest.trim_matches('"');
    if path.is_empty() {
        return Err("file path cannot be empty".to_string());
    }

    // Get the manifest directory (crate root)
    let manifest_dir =
        std::env::var("CARGO_MANIFEST_DIR").map_err(|_| "CARGO_MANIFEST_DIR not set")?;

    let full_path = std::path::Path::new(&manifest_dir).join(path);

    std::fs::read_to_string(&full_path)
        .map_err(|e| format!("failed to read '{}': {}", full_path.display(), e))
}

fn parse_and_encode_metadata(input: &str) -> Result<Vec<u8>, String> {
    let tokens = pact_parser::tokenize(input).map_err(|e| format!("tokenize error: {}", e))?;
    let mut parser = pact_parser::make_parser(tokens);

    let mut imports = Vec::new();
    let mut exports = Vec::new();
    let mut types = Vec::new();
    let mut type_params: Vec<metadata::TypeParam> = Vec::new();

    while !parser.is_eof() {
        // Interface-level generic parameter: `type s: constraint`. Distinguished
        // from a `type s = ...` alias by the `:` in third position (a generic
        // alias `type s<a> = ...` has `<` there instead). We define this syntax
        // to mirror the host pact convention.
        if parser.peek_n_is_ident(0, "type") && parser.peek_n_is_symbol(2, ':') {
            parser.accept_ident("type");
            let name = parser.expect_ident().map_err(|e| e.to_string())?;
            parser.expect_symbol(':').map_err(|e| e.to_string())?;
            let constraint = parser.expect_ident().map_err(|e| e.to_string())?;
            type_params.push(metadata::TypeParam {
                name,
                constraint: Some(constraint),
            });
            parser.accept_symbol(',');
            parser.accept_symbol(';');
            continue;
        }

        // Try to parse a type definition (record, variant, enum, flags, type alias)
        if let Some(td) = pact_parser::try_parse_typedef_public(&mut parser)
            .map_err(|e| format!("type definition error: {}", e))?
        {
            types.push(td);
            parser.accept_symbol(',');
            parser.accept_symbol(';');
            continue;
        }

        if parser.accept_ident("imports") {
            let param_names: Vec<String> = type_params.iter().map(|tp| tp.name.clone()).collect();
            parser.expect_symbol('{').map_err(|e| e.to_string())?;
            parse_import_sigs(&mut parser, &mut imports, &types, &param_names)?;
            parser.expect_symbol('}').map_err(|e| e.to_string())?;
        } else if parser.accept_ident("exports") {
            let param_names: Vec<String> = type_params.iter().map(|tp| tp.name.clone()).collect();
            parser.expect_symbol('{').map_err(|e| e.to_string())?;
            parse_func_sigs_into(&mut parser, "", &mut exports, &types, &param_names)?;
            parser.expect_symbol('}').map_err(|e| e.to_string())?;
        } else {
            return Err("expected type definition, 'imports', or 'exports'".into());
        }
    }

    Ok(metadata::encode_metadata(&imports, &exports, &type_params))
}

/// Parse an interface path like "theater:simple/runtime" or just "math".
/// Collects identifiers and the symbols `:` and `/` until it hits a `{`.
fn parse_interface_path(parser: &mut pact_parser::Parser) -> Result<String, String> {
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
    parser: &mut pact_parser::Parser,
    sigs: &mut Vec<metadata::FuncSig>,
    types: &[pact_parser::TypeDef],
    params: &[String],
) -> Result<(), String> {
    while !parser.peek_is_symbol('}') && !parser.is_eof() {
        let iface_name = parse_interface_path(parser)?;
        parser.expect_symbol('{').map_err(|e| e.to_string())?;
        parse_func_sigs_into(parser, &iface_name, sigs, types, params)?;
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
fn parse_function_path(
    parser: &mut pact_parser::Parser,
    default_interface: &str,
) -> Result<(String, String), String> {
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
    parser: &mut pact_parser::Parser,
    interface: &str,
    sigs: &mut Vec<metadata::FuncSig>,
    types: &[pact_parser::TypeDef],
    params: &[String],
) -> Result<(), String> {
    // Typedefs declared inside this block (e.g. `record foo { ... }`) shadow
    // and extend the outer `types` slice for ref resolution within the block.
    let mut local_types: Vec<pact_parser::TypeDef> = types.to_vec();

    while !parser.peek_is_symbol('}') && !parser.is_eof() {
        // Try a typedef first — records/variants/etc declared inside an
        // interface block are scoped to that block and resolve refs structurally.
        if let Some(td) = pact_parser::try_parse_typedef_public(parser)
            .map_err(|e| format!("type definition error: {}", e))?
        {
            local_types.push(td);
            parser.accept_symbol(',');
            parser.accept_symbol(';');
            continue;
        }

        let (iface, name) = parse_function_path(parser, interface)?;

        // Interface group: `iface { func: ..., ... }`. Symmetric with imports, so
        // exports can declare whole interfaces (and a provider can be recognized as
        // *providing* the same interface an actor *requires*). `parse_function_path`
        // has already collected the full interface path; a following `{` means this
        // was a group name, not a function.
        if parser.peek_is_symbol('{') {
            parser.expect_symbol('{').map_err(|e| e.to_string())?;
            parse_func_sigs_into(parser, &name, sigs, &local_types, params)?;
            parser.expect_symbol('}').map_err(|e| e.to_string())?;
            parser.accept_symbol(',');
            parser.accept_symbol(';');
            continue;
        }

        parser.expect_symbol(':').map_err(|e| e.to_string())?;
        parser.accept_ident("func");

        let func = pact_parser::parse_func_signature(parser, name).map_err(|e| e.to_string())?;

        let sig_params: Vec<(String, metadata::TypeDesc)> = func
            .params
            .iter()
            .map(|(n, t)| {
                (
                    n.clone(),
                    metadata::pact_type_to_type_desc_scoped(t, &local_types, params),
                )
            })
            .collect();

        let results: Vec<metadata::TypeDesc> = func
            .results
            .iter()
            .map(|t| metadata::pact_type_to_type_desc_scoped(t, &local_types, params))
            .collect();

        sigs.push(metadata::FuncSig {
            interface: iface,
            name: func.name,
            params: sig_params,
            results,
        });

        parser.accept_symbol(',');
        parser.accept_symbol(';');
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use packr_abi::{
        hash_function, hash_interface, hash_record, hash_result, Binding, HASH_STRING,
    };

    /// Parser accepts `record name { ... }` inside `imports { iface { ... } }`,
    /// scoping the type to that interface block, and refs in subsequent function
    /// signatures resolve to its structural hash. This is the original
    /// agentry-actor blocker from the user report.
    #[test]
    fn parses_record_inside_imports_interface_block() {
        let src = r#"
            imports {
                theater:simple/podman {
                    record container-spec {
                        image: string,
                        name: string,
                    }

                    run: func(spec: container-spec) -> result<string, string>
                }
            }
        "#;

        // Should parse without error (this is the bug from agentry).
        let bytes = parse_and_encode_metadata(src).expect("parse");
        assert!(!bytes.is_empty());

        // Decoded metadata should carry an import-hash for the interface that
        // matches the structural hash computed by hand.
        let record_hash = hash_record(&[("image", HASH_STRING), ("name", HASH_STRING)]);
        let func_hash = hash_function(&[record_hash], &[hash_result(&HASH_STRING, &HASH_STRING)]);
        let expected_iface_hash = hash_interface(
            "theater:simple/podman",
            &[],
            &[Binding {
                name: "run",
                hash: func_hash,
            }],
        );

        // Re-derive via the public path used by the metadata encoder.
        let mut sigs = Vec::new();
        let tokens = pact_parser::tokenize(src).expect("tokenize");
        let mut parser = pact_parser::make_parser(tokens);
        parser.accept_ident("imports");
        parser.expect_symbol('{').expect("imports {");
        parse_import_sigs(&mut parser, &mut sigs, &[], &[]).expect("import sigs");

        let iface_hashes = metadata::compute_interface_hashes(&sigs);
        assert_eq!(iface_hashes.len(), 1);
        assert_eq!(iface_hashes[0].name, "theater:simple/podman");
        assert_eq!(iface_hashes[0].hash, expected_iface_hash);
    }

    /// Exports accept the same interface-grouping grammar as imports, and a
    /// grouped export produces the *same* interface hash as the matching grouped
    /// import — the symmetry that makes interface-to-interface linking work.
    #[test]
    fn parses_grouped_exports_symmetric_with_imports() {
        let export_src = "exports { math { double: func(n: s64) -> s64, } }";
        let import_src = "imports { math { double: func(n: s64) -> s64, } }";

        // Grouped exports parse (previously: `expected ':', got '{'`).
        assert!(!parse_and_encode_metadata(export_src)
            .expect("grouped exports should parse")
            .is_empty());

        let sigs = |src: &str, kw: &str, grouped: bool| {
            let tokens = pact_parser::tokenize(src).expect("tokenize");
            let mut p = pact_parser::make_parser(tokens);
            p.accept_ident(kw);
            p.expect_symbol('{').unwrap();
            let mut out = Vec::new();
            if grouped {
                parse_import_sigs(&mut p, &mut out, &[], &[]).unwrap();
            } else {
                parse_func_sigs_into(&mut p, "", &mut out, &[], &[]).unwrap();
            }
            out
        };

        let export_sigs = sigs(export_src, "exports", false);
        let import_sigs = sigs(import_src, "imports", true);

        // The grouped export is recorded under the `math` interface.
        assert_eq!(export_sigs.len(), 1);
        assert_eq!(export_sigs[0].interface, "math");
        assert_eq!(export_sigs[0].name, "double");

        // Export and import interface hashes agree.
        let ex = metadata::compute_interface_hashes(&export_sigs);
        let im = metadata::compute_interface_hashes(&import_sigs);
        assert_eq!(ex.len(), 1);
        assert_eq!(ex[0].name, "math");
        assert_eq!(
            ex[0].hash, im[0].hash,
            "grouped export/import hashes must match"
        );
    }

    /// Records declared inside one interface block do not leak into a sibling
    /// interface block's resolution scope.
    #[test]
    fn record_scope_does_not_leak_between_sibling_interfaces() {
        let src = r#"
            imports {
                ns:pkg/a {
                    record shape {
                        a: string,
                    }
                    do-a: func(s: shape) -> string
                }
                ns:pkg/b {
                    // No `shape` typedef here — `shape` should remain unresolved.
                    do-b: func(s: shape) -> string
                }
            }
        "#;

        let mut sigs = Vec::new();
        let tokens = pact_parser::tokenize(src).expect("tokenize");
        let mut parser = pact_parser::make_parser(tokens);
        parser.accept_ident("imports");
        parser.expect_symbol('{').expect("imports {");
        parse_import_sigs(&mut parser, &mut sigs, &[], &[]).expect("import sigs");

        let iface_hashes = metadata::compute_interface_hashes(&sigs);
        let h_a = iface_hashes.iter().find(|h| h.name == "ns:pkg/a").unwrap();
        let h_b = iface_hashes.iter().find(|h| h.name == "ns:pkg/b").unwrap();

        // `a` resolves `shape` structurally; `b` falls back to opaque (HASH_SELF_REF
        // via TypeDesc::Value). The hashes must differ.
        assert_ne!(h_a.hash, h_b.hash);
    }

    /// Top-level typedefs remain visible inside imports/exports interface blocks.
    #[test]
    fn top_level_typedef_visible_inside_interface_block() {
        let src = r#"
            record point {
                x: s32,
                y: s32,
            }

            imports {
                ns:pkg/geo {
                    move: func(p: point) -> point
                }
            }
        "#;
        let bytes = parse_and_encode_metadata(src).expect("parse");
        assert!(!bytes.is_empty());
    }

    /// `result<_, E>` ok-arm hashes as Bool (and `_` in err-arm as String) by
    /// convention — see the comment in `pack/src/parser/pact.rs::parse_result`.
    /// The actor side must match this exactly so hashes converge.
    #[test]
    fn result_underscore_uses_bool_string_convention() {
        let src = r#"
            imports {
                ns:pkg/api {
                    do-nothing: func() -> result<_, string>
                }
            }
        "#;
        let mut sigs = Vec::new();
        let tokens = pact_parser::tokenize(src).expect("tokenize");
        let mut parser = pact_parser::make_parser(tokens);
        parser.accept_ident("imports");
        parser.expect_symbol('{').expect("imports {");
        parse_import_sigs(&mut parser, &mut sigs, &[], &[]).expect("import sigs");

        use packr_abi::HASH_BOOL;
        let func_hash = hash_function(&[], &[hash_result(&HASH_BOOL, &HASH_STRING)]);
        let expected = hash_interface(
            "ns:pkg/api",
            &[],
            &[Binding {
                name: "do-nothing",
                hash: func_hash,
            }],
        );
        let iface_hashes = metadata::compute_interface_hashes(&sigs);
        assert_eq!(iface_hashes[0].hash, expected);
    }

    // ========================================================================
    // Interface-level generics (M4-guest): embed type_params in __pack_types
    // ========================================================================

    /// The decoded `type-params` field, as a list of (name, constraint) pairs,
    /// mirroring exactly what the host `decode_type_param_list` reads.
    fn decoded_type_params(bytes: &[u8]) -> Vec<(String, String)> {
        let val = packr_abi::decode(bytes).expect("decode metadata");
        let packr_abi::Value::Record { fields, .. } = val else {
            panic!("expected record");
        };
        let mut out = Vec::new();
        for (name, v) in fields {
            if name != "type-params" {
                continue;
            }
            let packr_abi::Value::List { items, .. } = v else {
                panic!("type-params must be a list");
            };
            for item in items {
                if let packr_abi::Value::Record { fields, .. } = item {
                    let mut n = String::new();
                    let mut c = String::new();
                    for (fname, fval) in fields {
                        if let packr_abi::Value::String(s) = fval {
                            match fname.as_str() {
                                "name" => n = s,
                                "constraint" => c = s,
                                _ => {}
                            }
                        }
                    }
                    out.push((n, c));
                }
            }
        }
        out
    }

    #[test]
    fn embeds_interface_type_params() {
        // `type s: serializable` at the top level declares an interface-level
        // generic; a function signature then uses it.
        let src = "type s: serializable\nexports { get: func() -> s }";
        let bytes = parse_and_encode_metadata(src).expect("parse generic interface");
        assert_eq!(
            decoded_type_params(&bytes),
            vec![("s".to_string(), "serializable".to_string())]
        );
    }

    #[test]
    fn non_generic_omits_type_params_field() {
        // No interface parameter => no `type-params` field at all, so a
        // non-generic package is byte-identical to before this change.
        let src = "exports { ping: func() -> bool }";
        let bytes = parse_and_encode_metadata(src).expect("parse plain interface");
        assert!(decoded_type_params(&bytes).is_empty());
        assert!(
            !String::from_utf8_lossy(&bytes).contains("type-params"),
            "non-generic metadata must not carry a type-params field"
        );
    }

    #[test]
    fn interface_param_is_not_confused_with_alias() {
        // `type s = u32` is an ALIAS (a type def), not an interface parameter.
        let src = "type s = u32\nexports { get: func() -> s }";
        let bytes = parse_and_encode_metadata(src).expect("parse alias");
        assert!(
            decoded_type_params(&bytes).is_empty(),
            "a `type x = ...` alias must not be read as an interface parameter"
        );
    }
}
