//! Proc macros for Composite guest components.
//!
//! Provides the `#[export]` attribute macro for easily exporting functions
//! with the correct WASM calling convention.

use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, ItemFn, ReturnType, FnArg, Pat, LitStr, Token};
use syn::parse::{Parse, ParseStream};

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
