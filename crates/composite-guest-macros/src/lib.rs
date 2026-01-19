//! Proc macros for Composite guest components.
//!
//! Provides the `#[export]` and `#[import]` attribute macros for easily
//! exporting and importing functions with the correct WASM calling convention.

use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, ItemFn, ReturnType, FnArg, Pat, LitStr, Token, Ident};
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;

/// Arguments for the #[export] attribute.
struct ExportArgs {
    name: Option<String>,
}

impl Parse for ExportArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        if input.is_empty() {
            return Ok(ExportArgs { name: None });
        }

        let ident: syn::Ident = input.parse()?;
        if ident != "name" {
            return Err(syn::Error::new(ident.span(), "expected `name`"));
        }

        input.parse::<Token![=]>()?;
        let lit: LitStr = input.parse()?;

        Ok(ExportArgs {
            name: Some(lit.value()),
        })
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

    // Generate the wrapper with optional custom export name
    let expanded = match &args.name {
        Some(custom_name) => {
            // Custom name provided - use #[export_name] attribute
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

/// Arguments for the #[import] attribute.
struct ImportArgs {
    module: String,
    name: Option<String>,
}

impl Parse for ImportArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut module = None;
        let mut name = None;

        while !input.is_empty() {
            let ident: Ident = input.parse()?;
            input.parse::<Token![=]>()?;
            let lit: LitStr = input.parse()?;

            match ident.to_string().as_str() {
                "module" => module = Some(lit.value()),
                "name" => name = Some(lit.value()),
                other => {
                    return Err(syn::Error::new(
                        ident.span(),
                        format!("unexpected attribute `{}`, expected `module` or `name`", other),
                    ));
                }
            }

            // Consume optional comma
            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
        }

        let module = module.ok_or_else(|| {
            syn::Error::new(input.span(), "missing required `module` attribute")
        })?;

        Ok(ImportArgs { module, name })
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
/// // Import a log function from the host
/// #[import(module = "theater:simple/runtime")]
/// fn log(msg: String);
///
/// // Import with a custom function name
/// #[import(module = "theater:simple/runtime", name = "log")]
/// fn my_log(msg: String);
///
/// // Import a function that returns a value
/// #[import(module = "my:module/interface")]
/// fn get_value(key: String) -> String;
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

    let module = &args.module;
    let import_name = args.name.as_ref().unwrap_or(&sig.fn_name.to_string()).clone();
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
