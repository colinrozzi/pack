//! Code generator for Pact types.
//!
//! Takes Pact type definitions and generates Rust types with From/TryFrom
//! implementations for Value conversion.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::pact_parser::{
    Function, Interface, PactRegistry, Type, TypeAttrs, TypeDef, VariantCase, World, WorldItem,
};

/// Convert a Pact identifier (kebab-case) to Rust identifier (PascalCase for types, snake_case for functions)
fn to_rust_type_name(pact_name: &str) -> syn::Ident {
    let pascal = pact_name
        .split('-')
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(c) => c.to_uppercase().chain(chars).collect::<String>(),
                None => String::new(),
            }
        })
        .collect::<String>();
    format_ident!("{}", pascal)
}

fn to_rust_field_name(pact_name: &str) -> syn::Ident {
    let snake = pact_name.replace('-', "_");
    format_ident!("{}", snake)
}

fn to_rust_variant_name(pact_name: &str) -> syn::Ident {
    // Same as type name - PascalCase
    to_rust_type_name(pact_name)
}

/// Generate Rust type reference from Pact type
fn generate_type_ref(ty: &Type, self_type_name: Option<&str>) -> TokenStream {
    match ty {
        Type::Bool => quote! { bool },
        Type::U8 => quote! { u8 },
        Type::U16 => quote! { u16 },
        Type::U32 => quote! { u32 },
        Type::U64 => quote! { u64 },
        Type::S8 => quote! { i8 },
        Type::S16 => quote! { i16 },
        Type::S32 => quote! { i32 },
        Type::S64 => quote! { i64 },
        Type::F32 => quote! { f32 },
        Type::F64 => quote! { f64 },
        Type::Char => quote! { char },
        Type::String => quote! { ::alloc::string::String },
        Type::List(inner) => {
            // A `list<self>` needs no Box — the Vec already provides the
            // indirection, and `Vec<Self>` round-trips (whereas `Vec<Box<Self>>`
            // could not: `Box<T>` has no decode impl).
            let inner_ty = match inner.as_ref() {
                Type::SelfRef => match self_type_name {
                    Some(name) => {
                        let rust_name = to_rust_type_name(name);
                        quote! { #rust_name }
                    }
                    None => quote! { Self },
                },
                other => generate_type_ref(other, self_type_name),
            };
            quote! { ::alloc::vec::Vec<#inner_ty> }
        }
        Type::Option(inner) => {
            let inner_ty = generate_type_ref(inner, self_type_name);
            quote! { ::core::option::Option<#inner_ty> }
        }
        Type::Result { ok, err } => {
            let ok_ty = ok
                .as_ref()
                .map(|t| generate_type_ref(t, self_type_name))
                .unwrap_or_else(|| quote! { () });
            let err_ty = err
                .as_ref()
                .map(|t| generate_type_ref(t, self_type_name))
                .unwrap_or_else(|| quote! { () });
            quote! { ::core::result::Result<#ok_ty, #err_ty> }
        }
        Type::Tuple(items) => {
            if items.is_empty() {
                quote! { () }
            } else {
                let item_tys: Vec<_> = items
                    .iter()
                    .map(|t| generate_type_ref(t, self_type_name))
                    .collect();
                quote! { (#(#item_tys),*) }
            }
        }
        Type::Map { key, value } => {
            // `map<K, V>` lowers to `BTreeMap<K, V>`, which round-trips through
            // the ABI as `list<tuple<K, V>>` (key-sorted, so it's canonical).
            let key_ty = generate_type_ref(key, self_type_name);
            let value_ty = generate_type_ref(value, self_type_name);
            quote! { ::alloc::collections::BTreeMap<#key_ty, #value_ty> }
        }
        Type::Set(elem) => {
            // `set<T>` lowers to `BTreeSet<T>`, which round-trips through the ABI
            // as `list<T>` (key-sorted, so it's canonical).
            let elem_ty = generate_type_ref(elem, self_type_name);
            quote! { ::alloc::collections::BTreeSet<#elem_ty> }
        }
        Type::Named(name) => {
            let rust_name = to_rust_type_name(name);
            quote! { #rust_name }
        }
        Type::App { name, args } => {
            // Generic type application, e.g. `pair<u32, string>` -> `Pair<u32, String>`.
            let rust_name = to_rust_type_name(name);
            let arg_tys: Vec<_> = args
                .iter()
                .map(|t| generate_type_ref(t, self_type_name))
                .collect();
            quote! { #rust_name<#(#arg_tys),*> }
        }
        Type::SelfRef => {
            // A self-reference uses `Rec<Self>` (a decodable heap indirection),
            // not `Box<Self>`: `Box` is `#[fundamental]` and cannot round-trip
            // when nested in a container, so `Box<Self>` breaks a recursive
            // struct's `option<self>` field. `Rec<Self>` works in every position.
            if let Some(name) = self_type_name {
                let rust_name = to_rust_type_name(name);
                quote! { packr_guest::Rec<#rust_name> }
            } else {
                // Shouldn't happen in valid Pact
                quote! { Self }
            }
        }
    }
}

/// Build the `<A, B>` generic clause and the trait-bound `where` clause for a
/// generic type definition with the given Pact type-parameter names. Returns
/// empty token streams for a non-generic definition, so existing (non-generic)
/// codegen is unchanged. The bounds mirror what the hand-written `From`/
/// `TryFrom` impls need of each parameter:
///   - `A: Into<Value>`                             (encode)
///   - `A: TryFrom<Value, Error = ConversionError>` (decode)
fn generic_parts(type_params: &[String]) -> (TokenStream, TokenStream) {
    if type_params.is_empty() {
        return (quote! {}, quote! {});
    }
    let idents: Vec<syn::Ident> = type_params.iter().map(|p| to_rust_type_name(p)).collect();
    let generics = quote! { <#(#idents),*> };
    let bounds = idents.iter().map(|id| {
        quote! {
            #id: ::core::convert::Into<packr_guest::Value>
                + ::core::convert::TryFrom<packr_guest::Value, Error = packr_guest::ConversionError>
        }
    });
    let where_clause = quote! { where #(#bounds),* };
    (generics, where_clause)
}

/// Generate Value conversion expression for a type (Rust value -> Value)
#[allow(clippy::only_used_in_recursion)]
fn generate_to_value(ty: &Type, expr: TokenStream, self_type_name: Option<&str>) -> TokenStream {
    match ty {
        Type::Bool => quote! { packr_guest::Value::Bool(#expr) },
        Type::U8 => quote! { packr_guest::Value::U8(#expr) },
        Type::U16 => quote! { packr_guest::Value::U16(#expr) },
        Type::U32 => quote! { packr_guest::Value::U32(#expr) },
        Type::U64 => quote! { packr_guest::Value::U64(#expr) },
        Type::S8 => quote! { packr_guest::Value::S8(#expr) },
        Type::S16 => quote! { packr_guest::Value::S16(#expr) },
        Type::S32 => quote! { packr_guest::Value::S32(#expr) },
        Type::S64 => quote! { packr_guest::Value::S64(#expr) },
        Type::F32 => quote! { packr_guest::Value::F32(#expr) },
        Type::F64 => quote! { packr_guest::Value::F64(#expr) },
        Type::Char => quote! { packr_guest::Value::Char(#expr) },
        Type::String => quote! { packr_guest::Value::String(#expr) },
        Type::List(_) | Type::Option(_) | Type::Map { .. } | Type::Set(_) => {
            // Vec<T> / Option<T> / BTreeMap<K, V> encode via their packr_abi
            // `From` impls, which build the correct struct-form Value (a map
            // becomes a list<tuple<K, V>>).
            quote! { ::core::convert::Into::<packr_guest::Value>::into(#expr) }
        }
        Type::Result { ok, err } => {
            let ok_conversion = ok
                .as_ref()
                .map(|t| generate_to_value(t, quote! { v }, self_type_name))
                .unwrap_or_else(|| quote! { packr_guest::Value::Tuple(::alloc::vec![]) });
            let err_conversion = err
                .as_ref()
                .map(|t| generate_to_value(t, quote! { e }, self_type_name))
                .unwrap_or_else(|| quote! { packr_guest::Value::Tuple(::alloc::vec![]) });
            // Build the (struct-form) Variant encoding of a result that
            // `FromValue for Result` decodes (its legacy-variant branch).
            quote! {
                match #expr {
                    Ok(v) => packr_guest::Value::Variant {
                        type_name: ::alloc::string::String::from("result"),
                        case_name: ::alloc::string::String::from("ok"),
                        tag: 0,
                        payload: ::alloc::vec![#ok_conversion],
                    },
                    Err(e) => packr_guest::Value::Variant {
                        type_name: ::alloc::string::String::from("result"),
                        case_name: ::alloc::string::String::from("err"),
                        tag: 1,
                        payload: ::alloc::vec![#err_conversion],
                    },
                }
            }
        }
        Type::Tuple(items) => {
            if items.is_empty() {
                quote! { packr_guest::Value::Tuple(::alloc::vec![]) }
            } else {
                let conversions: Vec<_> = items
                    .iter()
                    .enumerate()
                    .map(|(i, t)| {
                        let idx = syn::Index::from(i);
                        let item_expr = quote! { #expr.#idx };
                        generate_to_value(t, item_expr, self_type_name)
                    })
                    .collect();
                quote! {
                    packr_guest::Value::Tuple(::alloc::vec![#(#conversions),*])
                }
            }
        }
        Type::Named(_) | Type::App { .. } | Type::SelfRef => {
            // Named types, generic applications, and self-refs implement
            // Into<Value>. Route through `Into::into` (rather than
            // `Value::from`) so that a generic parameter's `A: Into<Value>`
            // bound is never mis-selected for a concrete/container field whose
            // own impl should apply.
            quote! { ::core::convert::Into::<packr_guest::Value>::into(#expr) }
        }
    }
}

/// Generate a complete Rust type definition with From/TryFrom impls
pub fn generate_type_def(typedef: &TypeDef) -> TokenStream {
    generate_type_def_with_attrs(typedef, None)
}

fn generate_type_def_with_attrs(typedef: &TypeDef, attrs: Option<&TypeAttrs>) -> TokenStream {
    match typedef {
        TypeDef::Alias {
            name,
            type_params,
            ty,
        } => generate_alias(name, type_params, ty),
        TypeDef::Record {
            name,
            type_params,
            fields,
        } => generate_record(name, type_params, fields, attrs),
        TypeDef::Variant {
            name,
            type_params,
            cases,
        } => generate_variant(name, type_params, cases, attrs),
        TypeDef::Enum { name, cases } => generate_enum(name, cases, attrs),
        TypeDef::Flags { name, flags } => generate_flags(name, flags),
    }
}

fn generate_alias(name: &str, type_params: &[String], ty: &Type) -> TokenStream {
    let rust_name = to_rust_type_name(name);
    let (generics, _) = generic_parts(type_params);
    let rust_ty = generate_type_ref(ty, None);

    quote! {
        pub type #rust_name #generics = #rust_ty;
    }
}

fn generate_record(
    name: &str,
    type_params: &[String],
    fields: &[(String, Type)],
    attrs: Option<&TypeAttrs>,
) -> TokenStream {
    let rust_name = to_rust_type_name(name);
    // Only the generic parameter list is needed on the type; the GraphValue
    // derive supplies the trait bounds on its own generated impls.
    let (generics, _where_clause) = generic_parts(type_params);

    let field_defs: Vec<_> = fields
        .iter()
        .map(|(fname, ftype)| {
            let rust_fname = to_rust_field_name(fname);
            let rust_ftype = generate_type_ref(ftype, Some(name));
            quote! { pub #rust_fname: #rust_ftype }
        })
        .collect();

    // Marshal via the GraphValue derive (the same path src/codegen.rs uses),
    // rather than hand-written From/TryFrom impls — the derive builds the
    // correct struct-form Value and decodes via FromValue (so option/recursive
    // fields work), and it stays in lockstep with the tested derive.
    let derive_graph = derive_and_graph(attrs, true);
    quote! {
        #derive_graph
        pub struct #rust_name #generics {
            #(#field_defs),*
        }
    }
}

fn generate_variant(
    name: &str,
    type_params: &[String],
    cases: &[VariantCase],
    attrs: Option<&TypeAttrs>,
) -> TokenStream {
    let rust_name = to_rust_type_name(name);
    let (generics, _where_clause) = generic_parts(type_params);

    let case_defs: Vec<_> = cases
        .iter()
        .map(|case| {
            let case_name = to_rust_variant_name(&case.name);
            match &case.payload {
                Some(ty) => {
                    let rust_ty = generate_type_ref(ty, Some(name));
                    quote! { #case_name(#rust_ty) }
                }
                None => quote! { #case_name },
            }
        })
        .collect();

    // As with records, marshal via the GraphValue derive (case order = tag
    // order) instead of hand-written impls.
    let derive_graph = derive_and_graph(attrs, false);
    quote! {
        #derive_graph
        pub enum #rust_name #generics {
            #(#case_defs),*
        }
    }
}

fn generate_enum(name: &str, cases: &[String], attrs: Option<&TypeAttrs>) -> TokenStream {
    let rust_name = to_rust_type_name(name);

    let case_defs: Vec<_> = cases
        .iter()
        .map(|case| to_rust_variant_name(case))
        .collect();

    // A C-like enum marshals via the derive too (matching src/codegen.rs). It
    // keeps its Copy/Eq/Hash derives; only `@forward-compatible` applies here
    // (`@default` needs a `#[default]` variant, which pact enums don't express).
    let fwd_tok = if attrs.is_some_and(|a| a.forward_compatible) {
        quote! { , forward_compatible }
    } else {
        quote! {}
    };
    quote! {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, packr_guest::GraphValue)]
        #[graph(crate = "packr_guest::composite_abi" #fwd_tok)]
        pub enum #rust_name {
            #(#case_defs),*
        }
    }
}

fn generate_flags(name: &str, flags: &[String]) -> TokenStream {
    let rust_name = to_rust_type_name(name);

    let flag_consts: Vec<_> = flags
        .iter()
        .enumerate()
        .map(|(i, flag)| {
            let const_name = format_ident!("{}", flag.to_uppercase().replace('-', "_"));
            let bit: u64 = 1 << i;
            quote! { pub const #const_name: #rust_name = #rust_name(#bit); }
        })
        .collect();

    quote! {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
        pub struct #rust_name(pub u64);

        impl #rust_name {
            pub const NONE: #rust_name = #rust_name(0);
            #(#flag_consts)*

            pub fn contains(self, other: Self) -> bool {
                (self.0 & other.0) == other.0
            }
        }

        impl ::core::ops::BitOr for #rust_name {
            type Output = Self;
            fn bitor(self, rhs: Self) -> Self {
                #rust_name(self.0 | rhs.0)
            }
        }

        impl ::core::ops::BitAnd for #rust_name {
            type Output = Self;
            fn bitand(self, rhs: Self) -> Self {
                #rust_name(self.0 & rhs.0)
            }
        }

        impl From<#rust_name> for packr_guest::Value {
            fn from(value: #rust_name) -> packr_guest::Value {
                packr_guest::Value::Flags(value.0)
            }
        }

        impl TryFrom<packr_guest::Value> for #rust_name {
            type Error = packr_guest::ConversionError;

            fn try_from(value: packr_guest::Value) -> Result<Self, Self::Error> {
                match value {
                    packr_guest::Value::Flags(bits) => Ok(#rust_name(bits)),
                    _ => Err(packr_guest::ConversionError::TypeMismatch {
                        expected: "Flags".into(),
                        got: ::alloc::format!("{:?}", value),
                    }),
                }
            }
        }
    }
}

/// Generate all types from a world definition
pub fn generate_world_types(world: &World) -> TokenStream {
    let type_defs: Vec<_> = world
        .types
        .iter()
        .map(|td| generate_type_def_with_attrs(td, world.type_attrs.get(td.name())))
        .collect();

    quote! {
        #(#type_defs)*
    }
}

/// Build the `#[derive(...)]` + `#[graph(...)]` lines for a generated type,
/// honouring `@forward-compatible` / `@default` annotations. `allow_default`
/// is false for enums/variants (Rust's `derive(Default)` needs a `#[default]`
/// variant, which pact enums don't express).
fn derive_and_graph(attrs: Option<&TypeAttrs>, allow_default: bool) -> TokenStream {
    let forward_compatible = attrs.is_some_and(|a| a.forward_compatible);
    let derive_default = allow_default && attrs.is_some_and(|a| a.derive_default);
    let default_tok = if derive_default {
        quote! { , ::core::default::Default }
    } else {
        quote! {}
    };
    let fwd_tok = if forward_compatible {
        quote! { , forward_compatible }
    } else {
        quote! {}
    };
    quote! {
        #[derive(Debug, Clone, PartialEq #default_tok, packr_guest::GraphValue)]
        #[graph(crate = "packr_guest::composite_abi" #fwd_tok)]
    }
}

/// Get export function info from a world
#[allow(dead_code)]
pub fn get_world_exports(world: &World) -> Vec<&Function> {
    let mut exports = Vec::new();
    for item in &world.exports {
        match item {
            WorldItem::Function(f) => exports.push(f),
            WorldItem::InlineInterface { functions, .. } => {
                for f in functions {
                    exports.push(f);
                }
            }
            _ => {}
        }
    }
    exports
}

/// Get import function info from a world
#[allow(dead_code)]
pub fn get_world_imports(world: &World) -> Vec<(&str, &Function)> {
    let mut imports = Vec::new();
    for item in &world.imports {
        match item {
            WorldItem::Function(f) => imports.push(("", f)),
            WorldItem::InlineInterface { name, functions } => {
                for f in functions {
                    imports.push((name.as_str(), f));
                }
            }
            _ => {}
        }
    }
    imports
}

// ============================================================================
// Import Module Generation
// ============================================================================

/// Format an interface path from its components
fn format_interface_path(
    namespace: &Option<String>,
    package: &Option<String>,
    interface: &str,
) -> String {
    match (namespace, package) {
        (Some(ns), Some(pkg)) => format!("{}:{}/{}", ns, pkg, interface),
        (None, Some(pkg)) => format!("{}/{}", pkg, interface),
        _ => interface.to_string(),
    }
}

/// Generate import modules from world imports
pub fn generate_imports(registry: &PactRegistry, world: &World) -> TokenStream {
    let mut modules = Vec::new();

    for import in &world.imports {
        match import {
            WorldItem::InterfacePath {
                namespace,
                package,
                interface,
            } => {
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
            WorldItem::Function(f) => {
                // Bare function import - generate at top level
                let func = generate_import_function("", f);
                modules.push(func);
            }
        }
    }

    quote! { #(#modules)* }
}

/// Generate a single import module from an interface
fn generate_import_module(module_path: &str, iface: &Interface) -> TokenStream {
    // Convert interface name to module name (kebab-case to snake_case)
    let module_name = iface.name.replace('-', "_");
    let module_ident = format_ident!("{}", module_name);

    let functions: Vec<_> = iface
        .functions
        .iter()
        .map(|f| generate_import_function(module_path, f))
        .collect();

    // Also generate types from the interface
    let types: Vec<_> = iface.types.iter().map(generate_type_def).collect();

    quote! {
        pub mod #module_ident {
            #![allow(unused_imports)]
            use super::*;

            #(#types)*
            #(#functions)*
        }
    }
}

/// Generate an inline import module (from world inline interface)
fn generate_inline_import_module(name: &str, functions: &[Function]) -> TokenStream {
    let module_name = name.replace('-', "_");
    let module_ident = format_ident!("{}", module_name);

    let funcs: Vec<_> = functions
        .iter()
        .map(|f| generate_import_function(name, f))
        .collect();

    quote! {
        pub mod #module_ident {
            #![allow(unused_imports)]
            use super::*;

            #(#funcs)*
        }
    }
}

/// Generate a single typed import function
fn generate_import_function(module_path: &str, func: &Function) -> TokenStream {
    let fn_name = format_ident!("{}", func.name.replace('-', "_"));
    let raw_fn_name = format_ident!("__raw_{}", func.name.replace('-', "_"));
    let link_name = &func.name;

    // Generate parameter list - use &str for string params in imports
    let params: Vec<_> = func
        .params
        .iter()
        .map(|(name, ty)| {
            let param_name = format_ident!("{}", name.replace('-', "_"));
            let param_type = if matches!(ty, Type::String) {
                quote! { &str }
            } else {
                generate_type_ref(ty, None)
            };
            quote! { #param_name: #param_type }
        })
        .collect();

    // Generate return type
    let return_type = if func.results.is_empty() {
        quote! { () }
    } else if func.results.len() == 1 {
        generate_type_ref(&func.results[0], None)
    } else {
        let tys: Vec<_> = func
            .results
            .iter()
            .map(|t| generate_type_ref(t, None))
            .collect();
        quote! { (#(#tys),*) }
    };

    // Generate input value construction
    let input_construction = if func.params.is_empty() {
        quote! { packr_guest::Value::Tuple(::alloc::vec![]) }
    } else if func.params.len() == 1 {
        let (name, ty) = &func.params[0];
        let param_name = format_ident!("{}", name.replace('-', "_"));
        generate_to_value_for_import(ty, quote! { #param_name })
    } else {
        let conversions: Vec<_> = func
            .params
            .iter()
            .map(|(name, ty)| {
                let param_name = format_ident!("{}", name.replace('-', "_"));
                generate_to_value_for_import(ty, quote! { #param_name })
            })
            .collect();
        quote! { packr_guest::Value::Tuple(::alloc::vec![#(#conversions),*]) }
    };

    // Generate return extraction
    let has_return = !func.results.is_empty();
    let body = if has_return {
        let result_ty = &return_type;
        quote! {
            let input = #input_construction;
            let result = packr_guest::__import_impl(
                |a, b, c, d| unsafe { #raw_fn_name(a, b, c, d) },
                input,
            );
            match <#result_ty>::try_from(result) {
                Ok(v) => v,
                Err(_) => panic!("failed to convert result from {}", stringify!(#fn_name)),
            }
        }
    } else {
        quote! {
            let input = #input_construction;
            let _ = packr_guest::__import_impl(
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
            packr_guest::Value::String(::alloc::string::String::from(#expr))
        },
        Type::Option(inner) if matches!(inner.as_ref(), Type::String) => quote! {
            match #expr {
                Some(s) => packr_guest::Value::Option {
                    inner_type: packr_guest::ValueType::String,
                    value: Some(::alloc::boxed::Box::new(
                        packr_guest::Value::String(::alloc::string::String::from(s))
                    )),
                },
                None => packr_guest::Value::Option {
                    inner_type: packr_guest::ValueType::String,
                    value: None,
                },
            }
        },
        // Other types use the standard conversion
        _ => generate_to_value(ty, expr, None),
    }
}

// ============================================================================
// Export Metadata Generation
// ============================================================================

/// Format a Pact function signature as a string
fn format_function_signature(func: &Function) -> String {
    let params: Vec<String> = func
        .params
        .iter()
        .map(|(name, ty)| format!("{}: {}", name, format_pact_type(ty)))
        .collect();

    let results = if func.results.is_empty() {
        String::new()
    } else if func.results.len() == 1 {
        format!(" -> {}", format_pact_type(&func.results[0]))
    } else {
        let result_strs: Vec<String> = func.results.iter().map(format_pact_type).collect();
        format!(" -> ({})", result_strs.join(", "))
    };

    format!("func({}){}", params.join(", "), results)
}

/// Format a Pact type as a string
fn format_pact_type(ty: &Type) -> String {
    match ty {
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
        Type::List(inner) => format!("list<{}>", format_pact_type(inner)),
        Type::Option(inner) => format!("option<{}>", format_pact_type(inner)),
        Type::Result { ok, err } => {
            let ok_str = ok
                .as_ref()
                .map(|t| format_pact_type(t))
                .unwrap_or_else(|| "_".to_string());
            let err_str = err
                .as_ref()
                .map(|t| format_pact_type(t))
                .unwrap_or_else(|| "_".to_string());
            format!("result<{}, {}>", ok_str, err_str)
        }
        Type::Tuple(items) => {
            let item_strs: Vec<String> = items.iter().map(format_pact_type).collect();
            format!("tuple<{}>", item_strs.join(", "))
        }
        Type::Map { key, value } => {
            format!(
                "map<{}, {}>",
                format_pact_type(key),
                format_pact_type(value)
            )
        }
        Type::Set(elem) => format!("set<{}>", format_pact_type(elem)),
        Type::Named(name) => name.clone(),
        Type::App { name, args } => {
            let arg_strs: Vec<String> = args.iter().map(format_pact_type).collect();
            format!("{}<{}>", name, arg_strs.join(", "))
        }
        Type::SelfRef => "self".to_string(),
    }
}

/// Information about an expected export
#[allow(dead_code)]
pub struct ExportInfo {
    pub name: String,
    pub export_name: String,
    pub signature: String,
    pub params: Vec<(String, Type)>,
    pub results: Vec<Type>,
}

/// Generate export metadata for validation
pub fn generate_export_metadata(registry: &PactRegistry, world: &World) -> TokenStream {
    let mut exports: Vec<ExportInfo> = Vec::new();

    for export in &world.exports {
        match export {
            WorldItem::Function(f) => {
                exports.push(ExportInfo {
                    name: f.name.clone(),
                    export_name: f.name.clone(),
                    signature: format_function_signature(f),
                    params: f.params.clone(),
                    results: f.results.clone(),
                });
            }
            WorldItem::InterfacePath {
                namespace,
                package,
                interface,
            } => {
                let path = format_interface_path(namespace, package, interface);
                if let Some(iface) = registry.interfaces.get(&path) {
                    for f in &iface.functions {
                        let export_name = format!("{}.{}", path, f.name);
                        exports.push(ExportInfo {
                            name: f.name.clone(),
                            export_name,
                            signature: format_function_signature(f),
                            params: f.params.clone(),
                            results: f.results.clone(),
                        });
                    }
                }
            }
            WorldItem::InlineInterface { name, functions } => {
                for f in functions {
                    let export_name = format!("{}.{}", name, f.name);
                    exports.push(ExportInfo {
                        name: f.name.clone(),
                        export_name,
                        signature: format_function_signature(f),
                        params: f.params.clone(),
                        results: f.results.clone(),
                    });
                }
            }
        }
    }

    let entries: Vec<_> = exports
        .iter()
        .map(|e| {
            let name = &e.name;
            let export_name = &e.export_name;
            let sig = &e.signature;
            quote! {
                (#name, #export_name, #sig)
            }
        })
        .collect();

    quote! {
        #[doc(hidden)]
        pub mod __pack_exports {
            /// (function_name, export_name, pact_signature)
            pub const EXPORTS: &[(&str, &str, &str)] = &[
                #(#entries),*
            ];

            /// Look up an export by function name
            pub fn get_export(name: &str) -> Option<(&'static str, &'static str)> {
                EXPORTS.iter()
                    .find(|(n, _, _)| *n == name)
                    .map(|(_, export_name, sig)| (*export_name, *sig))
            }
        }
    }
}

/// Collect export information for the #[export] macro to use
#[allow(dead_code)]
pub fn collect_exports(registry: &PactRegistry, world: &World) -> Vec<ExportInfo> {
    let mut exports = Vec::new();

    for export in &world.exports {
        match export {
            WorldItem::Function(f) => {
                exports.push(ExportInfo {
                    name: f.name.clone(),
                    export_name: f.name.clone(),
                    signature: format_function_signature(f),
                    params: f.params.clone(),
                    results: f.results.clone(),
                });
            }
            WorldItem::InterfacePath {
                namespace,
                package,
                interface,
            } => {
                let path = format_interface_path(namespace, package, interface);
                if let Some(iface) = registry.interfaces.get(&path) {
                    for f in &iface.functions {
                        let export_name = format!("{}.{}", path, f.name);
                        exports.push(ExportInfo {
                            name: f.name.clone(),
                            export_name,
                            signature: format_function_signature(f),
                            params: f.params.clone(),
                            results: f.results.clone(),
                        });
                    }
                }
            }
            WorldItem::InlineInterface { name, functions } => {
                for f in functions {
                    let export_name = format!("{}.{}", name, f.name);
                    exports.push(ExportInfo {
                        name: f.name.clone(),
                        export_name,
                        signature: format_function_signature(f),
                        params: f.params.clone(),
                        results: f.results.clone(),
                    });
                }
            }
        }
    }

    exports
}

#[cfg(test)]
mod generic_tests {
    use super::*;
    use crate::pact_parser::{Type, TypeDef, VariantCase};

    /// The generated code must at least be syntactically valid Rust.
    fn assert_valid_rust(ts: &TokenStream) {
        syn::parse2::<syn::File>(ts.clone())
            .unwrap_or_else(|e| panic!("generated code is not valid Rust: {e}\n{ts}"));
    }

    #[test]
    fn type_ref_application_lowers_to_generic() {
        let ty = Type::App {
            name: "pair".into(),
            args: vec![Type::U32, Type::String],
        };
        let out = generate_type_ref(&ty, None);
        assert_eq!(
            out.to_string(),
            "Pair < u32 , :: alloc :: string :: String >"
        );
    }

    #[test]
    fn record_annotations_emit_forward_compatible_and_default() {
        use crate::pact_parser::TypeAttrs;
        let td = TypeDef::Record {
            name: "state".into(),
            type_params: vec![],
            fields: vec![("count".into(), Type::U64)],
        };
        // No annotations: neither attr present.
        let plain = generate_type_def(&td).to_string();
        assert!(!plain.contains("forward_compatible"), "{plain}");
        assert!(!plain.contains("Default"), "{plain}");

        // Both annotations: forward_compatible on the graph attr + Default derived.
        let attrs = TypeAttrs {
            forward_compatible: true,
            derive_default: true,
        };
        let annotated = generate_type_def_with_attrs(&td, Some(&attrs)).to_string();
        assert!(annotated.contains("forward_compatible"), "{annotated}");
        assert!(annotated.contains("Default"), "{annotated}");
        assert_valid_rust(&generate_type_def_with_attrs(&td, Some(&attrs)));
    }

    #[test]
    fn generic_record_codegen_is_valid_and_parameterized() {
        let td = TypeDef::Record {
            name: "pair".into(),
            type_params: vec!["a".into(), "b".into()],
            fields: vec![
                ("first".into(), Type::Named("a".into())),
                ("second".into(), Type::Named("b".into())),
            ],
        };
        let out = generate_type_def(&td);
        assert_valid_rust(&out);
        let s = out.to_string();
        assert!(s.contains("struct Pair < A , B >"), "{s}");
        // Marshalling comes from the GraphValue derive (not hand-written impls),
        // pointed at the guest ABI crate.
        assert!(s.contains("GraphValue"), "must derive GraphValue: {s}");
        assert!(
            s.contains("composite_abi"),
            "must set the graph crate path: {s}"
        );
    }

    #[test]
    fn recursive_generic_variant_codegen_is_valid() {
        // variant tree<t> { leaf(t), branch(tuple<tree<t>, tree<t>>) }
        let app = || Type::App {
            name: "tree".into(),
            args: vec![Type::Named("t".into())],
        };
        let td = TypeDef::Variant {
            name: "tree".into(),
            type_params: vec!["t".into()],
            cases: vec![
                VariantCase {
                    name: "leaf".into(),
                    payload: Some(Type::Named("t".into())),
                },
                VariantCase {
                    name: "branch".into(),
                    payload: Some(Type::Tuple(vec![app(), app()])),
                },
            ],
        };
        let out = generate_type_def(&td);
        assert_valid_rust(&out);
        let s = out.to_string();
        assert!(s.contains("enum Tree < T >"), "{s}");
        // The recursive application lowers to the parameterized Rust type.
        assert!(s.contains("Tree < T >"), "{s}");
    }
}
