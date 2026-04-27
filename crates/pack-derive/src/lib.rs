//! Derive macros for pack-abi Value conversion.
//!
//! This crate provides `#[derive(GraphValue)]` which generates implementations
//! of `From<T> for Value` and `TryFrom<Value> for T`.
//!
//! # Example
//!
//! ```ignore
//! use pack_abi::{GraphValue, Value};
//!
//! #[derive(GraphValue)]
//! struct Point {
//!     x: i64,
//!     y: i64,
//! }
//!
//! let point = Point { x: 10, y: 20 };
//! let value: Value = point.into();
//! let back: Point = value.try_into().unwrap();
//! ```
//!
//! # Crate Path
//!
//! By default, the macro expects `pack_abi` to be in scope. For `no_std` guests
//! using `pack_guest`, specify the crate path:
//!
//! ```ignore
//! use pack_guest::GraphValue;
//!
//! #[derive(GraphValue)]
//! #[graph(crate = "pack_guest::composite_abi")]
//! struct MyState {
//!     count: i32,
//! }
//! ```

use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{parse_macro_input, Attribute, Data, DeriveInput, Fields, Meta};

/// Extract the crate path from `#[graph(crate = "...")]` attribute.
/// Defaults to `pack_abi` if not specified.
fn get_crate_path(attrs: &[Attribute]) -> proc_macro2::TokenStream {
    for attr in attrs {
        if attr.path().is_ident("graph") {
            if let Meta::List(list) = &attr.meta {
                let tokens = list.tokens.to_string();
                // Parse crate = "..."
                if let Some(rest) = tokens.strip_prefix("crate") {
                    let rest = rest.trim();
                    if let Some(rest) = rest.strip_prefix('=') {
                        let rest = rest.trim();
                        if rest.starts_with('"') && rest.ends_with('"') {
                            let path_str = &rest[1..rest.len() - 1];
                            // Convert string path to token stream
                            let path: syn::Path =
                                syn::parse_str(path_str).expect("Invalid crate path");
                            return quote! { #path };
                        }
                    }
                }
            }
        }
    }
    // Default to pack_abi
    quote! { pack_abi }
}

/// Derive macro for converting between Rust types and `Value`.
///
/// # Structs
///
/// Structs are converted to `Value::Record` with field names as keys.
///
/// ```ignore
/// #[derive(GraphValue)]
/// struct Person {
///     name: String,
///     age: i64,
/// }
/// ```
///
/// # Enums
///
/// Enums are converted to `Value::Variant` with the variant index as tag.
///
/// ```ignore
/// #[derive(GraphValue)]
/// enum Shape {
///     Circle(f64),           // tag 0, payload = radius
///     Rectangle(f64, f64),   // tag 1, payload = tuple(width, height)
///     Point,                 // tag 2, no payload
/// }
/// ```
///
/// # Attributes
///
/// - `#[graph(crate = "path")]` - Specify the crate path (default: `pack_abi`)
/// - `#[graph(rename = "name")]` - Use a different name for field/variant
/// - `#[graph(tag = N)]` - Use explicit tag number for variant
#[proc_macro_derive(GraphValue, attributes(graph))]
pub fn derive_graph_value(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let crate_path = get_crate_path(&input.attrs);

    let expanded = match &input.data {
        Data::Struct(data) => derive_struct(&input, data, &crate_path),
        Data::Enum(data) => derive_enum(&input, data, &crate_path),
        Data::Union(_) => {
            return syn::Error::new_spanned(&input, "GraphValue cannot be derived for unions")
                .to_compile_error()
                .into();
        }
    };

    expanded.into()
}

fn derive_struct(
    input: &DeriveInput,
    data: &syn::DataStruct,
    krate: &proc_macro2::TokenStream,
) -> proc_macro2::TokenStream {
    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    match &data.fields {
        Fields::Named(fields) => {
            // Generate TryFrom<Value> for T
            let field_from_value: Vec<_> = fields.named.iter().map(|f| {
                let field_name = f.ident.as_ref().unwrap();
                let field_name_str = get_rename(&f.attrs).unwrap_or_else(|| field_name.to_string());
                let field_type = &f.ty;
                quote! {
                    #field_name: {
                        let field_value = fields.iter()
                            .find(|(name, _)| name == #field_name_str)
                            .map(|(_, v)| v.clone())
                            .ok_or_else(|| #krate::ConversionError::MissingField(
                                #krate::__private::String::from(#field_name_str)
                            ))?;
                        <#field_type as #krate::__private::TryFrom<#krate::Value>>::try_from(field_value)
                            .map_err(|e| #krate::ConversionError::FieldError(
                                #krate::__private::String::from(#field_name_str),
                                #krate::__private::Box::new(e)
                            ))?
                    }
                }
            }).collect();

            let field_count = fields.named.len();

            // Generate field accessors for From impl
            let field_accessors: Vec<_> = fields
                .named
                .iter()
                .map(|f| {
                    let field_name = f.ident.as_ref().unwrap();
                    let field_name_str =
                        get_rename(&f.attrs).unwrap_or_else(|| field_name.to_string());
                    quote! {
                        (
                            #krate::__private::String::from(#field_name_str),
                            #krate::Value::from(value.#field_name)
                        )
                    }
                })
                .collect();

            let type_name_str = name.to_string();

            quote! {
                impl #impl_generics #krate::__private::From<#name #ty_generics> for #krate::Value #where_clause {
                    fn from(value: #name #ty_generics) -> #krate::Value {
                        #krate::Value::Record {
                            type_name: #krate::__private::String::from(#type_name_str),
                            fields: #krate::__private::vec![
                                #(#field_accessors),*
                            ],
                        }
                    }
                }

                impl #impl_generics #krate::__private::TryFrom<#krate::Value> for #name #ty_generics #where_clause {
                    type Error = #krate::ConversionError;

                    fn try_from(value: #krate::Value) -> #krate::__private::Result<Self, Self::Error> {
                        match value {
                            #krate::Value::Record { fields, .. } => {
                                if fields.len() != #field_count {
                                    return #krate::__private::Err(#krate::ConversionError::WrongFieldCount {
                                        expected: #field_count,
                                        got: fields.len(),
                                    });
                                }
                                #krate::__private::Ok(Self {
                                    #(#field_from_value),*
                                })
                            }
                            other => #krate::__private::Err(#krate::ConversionError::ExpectedRecord(
                                #krate::__private::format!("{:?}", other)
                            )),
                        }
                    }
                }
            }
        }
        Fields::Unnamed(fields) => {
            // Tuple struct -> Value::Tuple
            let field_indices: Vec<_> = (0..fields.unnamed.len()).map(syn::Index::from).collect();

            let field_from_value: Vec<_> = fields.unnamed.iter().enumerate().map(|(i, f)| {
                let field_type = &f.ty;
                quote! {
                    <#field_type as #krate::__private::TryFrom<#krate::Value>>::try_from(
                        fields.get(#i).cloned().ok_or_else(|| #krate::ConversionError::MissingIndex(#i))?
                    ).map_err(|e| #krate::ConversionError::IndexError(#i, #krate::__private::Box::new(e)))?
                }
            }).collect();

            let field_count = fields.unnamed.len();

            quote! {
                impl #impl_generics #krate::__private::From<#name #ty_generics> for #krate::Value #where_clause {
                    fn from(value: #name #ty_generics) -> #krate::Value {
                        #krate::Value::Tuple(#krate::__private::vec![
                            #(#krate::Value::from(value.#field_indices)),*
                        ])
                    }
                }

                impl #impl_generics #krate::__private::TryFrom<#krate::Value> for #name #ty_generics #where_clause {
                    type Error = #krate::ConversionError;

                    fn try_from(value: #krate::Value) -> #krate::__private::Result<Self, Self::Error> {
                        match value {
                            #krate::Value::Tuple(fields) => {
                                if fields.len() != #field_count {
                                    return #krate::__private::Err(#krate::ConversionError::WrongFieldCount {
                                        expected: #field_count,
                                        got: fields.len(),
                                    });
                                }
                                #krate::__private::Ok(Self(
                                    #(#field_from_value),*
                                ))
                            }
                            other => #krate::__private::Err(#krate::ConversionError::ExpectedTuple(
                                #krate::__private::format!("{:?}", other)
                            )),
                        }
                    }
                }
            }
        }
        Fields::Unit => {
            // Unit struct -> Value::Tuple([])
            quote! {
                impl #impl_generics #krate::__private::From<#name #ty_generics> for #krate::Value #where_clause {
                    fn from(_: #name #ty_generics) -> #krate::Value {
                        #krate::Value::Tuple(#krate::__private::vec![])
                    }
                }

                impl #impl_generics #krate::__private::TryFrom<#krate::Value> for #name #ty_generics #where_clause {
                    type Error = #krate::ConversionError;

                    fn try_from(value: #krate::Value) -> #krate::__private::Result<Self, Self::Error> {
                        match value {
                            #krate::Value::Tuple(fields) if fields.is_empty() => {
                                #krate::__private::Ok(Self)
                            }
                            #krate::Value::Tuple(fields) => {
                                #krate::__private::Err(#krate::ConversionError::WrongFieldCount {
                                    expected: 0,
                                    got: fields.len(),
                                })
                            }
                            other => #krate::__private::Err(#krate::ConversionError::ExpectedTuple(
                                #krate::__private::format!("{:?}", other)
                            )),
                        }
                    }
                }
            }
        }
    }
}

fn derive_enum(
    input: &DeriveInput,
    data: &syn::DataEnum,
    krate: &proc_macro2::TokenStream,
) -> proc_macro2::TokenStream {
    let name = &input.ident;
    let type_name_str = name.to_string();
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    // Generate match arms for From<T> for Value
    let to_value_arms: Vec<_> = data
        .variants
        .iter()
        .enumerate()
        .map(|(default_tag, variant)| {
            let variant_name = &variant.ident;
            let case_name_str = variant_name.to_string();
            let tag = get_tag(&variant.attrs).unwrap_or(default_tag);

            match &variant.fields {
                Fields::Named(fields) => {
                    let field_names: Vec<_> = fields
                        .named
                        .iter()
                        .map(|f| f.ident.as_ref().unwrap())
                        .collect();
                    // For named fields, we wrap in a Record as the single payload element
                    let field_to_value: Vec<_> = fields
                        .named
                        .iter()
                        .map(|f| {
                            let field_name = f.ident.as_ref().unwrap();
                            let field_name_str =
                                get_rename(&f.attrs).unwrap_or_else(|| field_name.to_string());
                            quote! {
                                (
                                    #krate::__private::String::from(#field_name_str),
                                    #krate::Value::from(#field_name)
                                )
                            }
                        })
                        .collect();

                    quote! {
                        #name::#variant_name { #(#field_names),* } => {
                            #krate::Value::Variant {
                                type_name: #krate::__private::String::from(#type_name_str),
                                case_name: #krate::__private::String::from(#case_name_str),
                                tag: #tag,
                                payload: #krate::__private::vec![
                                    #krate::Value::Record {
                                        type_name: #krate::__private::String::from(#case_name_str),
                                        fields: #krate::__private::vec![#(#field_to_value),*],
                                    }
                                ],
                            }
                        }
                    }
                }
                Fields::Unnamed(fields) => {
                    let field_names: Vec<_> = (0..fields.unnamed.len())
                        .map(|i| format_ident!("f{}", i))
                        .collect();

                    // Payload is a vec of all the field values
                    quote! {
                        #name::#variant_name(#(#field_names),*) => {
                            #krate::Value::Variant {
                                type_name: #krate::__private::String::from(#type_name_str),
                                case_name: #krate::__private::String::from(#case_name_str),
                                tag: #tag,
                                payload: #krate::__private::vec![
                                    #(#krate::Value::from(#field_names)),*
                                ],
                            }
                        }
                    }
                }
                Fields::Unit => {
                    quote! {
                        #name::#variant_name => {
                            #krate::Value::Variant {
                                type_name: #krate::__private::String::from(#type_name_str),
                                case_name: #krate::__private::String::from(#case_name_str),
                                tag: #tag,
                                payload: #krate::__private::vec![],
                            }
                        }
                    }
                }
            }
        })
        .collect();

    // Generate match arms for TryFrom<Value> for T
    let from_value_arms: Vec<_> = data.variants.iter().enumerate().map(|(default_tag, variant)| {
        let variant_name = &variant.ident;
        let tag = get_tag(&variant.attrs).unwrap_or(default_tag);

        match &variant.fields {
            Fields::Named(fields) => {
                let field_from_value: Vec<_> = fields.named.iter().map(|f| {
                    let field_name = f.ident.as_ref().unwrap();
                    let field_name_str = get_rename(&f.attrs).unwrap_or_else(|| field_name.to_string());
                    let field_type = &f.ty;
                    quote! {
                        #field_name: {
                            let field_value = record_fields.iter()
                                .find(|(name, _)| name == #field_name_str)
                                .map(|(_, v)| v.clone())
                                .ok_or_else(|| #krate::ConversionError::MissingField(
                                    #krate::__private::String::from(#field_name_str)
                                ))?;
                            <#field_type as #krate::__private::TryFrom<#krate::Value>>::try_from(field_value)
                                .map_err(|e| #krate::ConversionError::FieldError(
                                    #krate::__private::String::from(#field_name_str),
                                    #krate::__private::Box::new(e)
                                ))?
                        }
                    }
                }).collect();

                quote! {
                    #tag => {
                        // For named fields, payload should contain a single Record
                        if payload.len() != 1 {
                            return #krate::__private::Err(#krate::ConversionError::WrongFieldCount {
                                expected: 1,
                                got: payload.len(),
                            });
                        }
                        match &payload[0] {
                            #krate::Value::Record { fields: record_fields, .. } => {
                                #krate::__private::Ok(#name::#variant_name {
                                    #(#field_from_value),*
                                })
                            }
                            other => #krate::__private::Err(#krate::ConversionError::ExpectedRecord(
                                #krate::__private::format!("{:?}", other)
                            )),
                        }
                    }
                }
            }
            Fields::Unnamed(fields) => {
                let field_count = fields.unnamed.len();
                let field_conversions: Vec<_> = fields.unnamed.iter().enumerate().map(|(i, f)| {
                    let field_type = &f.ty;
                    quote! {
                        <#field_type as #krate::__private::TryFrom<#krate::Value>>::try_from(
                            payload.get(#i).cloned().ok_or_else(|| #krate::ConversionError::MissingIndex(#i))?
                        ).map_err(|e| #krate::ConversionError::IndexError(#i, #krate::__private::Box::new(e)))?
                    }
                }).collect();

                quote! {
                    #tag => {
                        if payload.len() != #field_count {
                            return #krate::__private::Err(#krate::ConversionError::WrongFieldCount {
                                expected: #field_count,
                                got: payload.len(),
                            });
                        }
                        #krate::__private::Ok(#name::#variant_name(
                            #(#field_conversions),*
                        ))
                    }
                }
            }
            Fields::Unit => {
                quote! {
                    #tag => {
                        if !payload.is_empty() {
                            return #krate::__private::Err(#krate::ConversionError::UnexpectedPayload);
                        }
                        #krate::__private::Ok(#name::#variant_name)
                    }
                }
            }
        }
    }).collect();

    let variant_count = data.variants.len();

    quote! {
        impl #impl_generics #krate::__private::From<#name #ty_generics> for #krate::Value #where_clause {
            fn from(value: #name #ty_generics) -> #krate::Value {
                match value {
                    #(#to_value_arms),*
                }
            }
        }

        impl #impl_generics #krate::__private::TryFrom<#krate::Value> for #name #ty_generics #where_clause {
            type Error = #krate::ConversionError;

            fn try_from(value: #krate::Value) -> #krate::__private::Result<Self, Self::Error> {
                match value {
                    #krate::Value::Variant { tag, payload, .. } => {
                        match tag {
                            #(#from_value_arms),*
                            other => #krate::__private::Err(#krate::ConversionError::UnknownTag {
                                tag: other,
                                max: #variant_count,
                            }),
                        }
                    }
                    other => #krate::__private::Err(#krate::ConversionError::ExpectedVariant(
                        #krate::__private::format!("{:?}", other)
                    )),
                }
            }
        }
    }
}

/// Extract `#[graph(rename = "...")]` attribute
fn get_rename(attrs: &[Attribute]) -> Option<String> {
    for attr in attrs {
        if attr.path().is_ident("graph") {
            if let Meta::List(list) = &attr.meta {
                let tokens = list.tokens.to_string();
                // Parse rename = "..."
                if let Some(rest) = tokens.strip_prefix("rename") {
                    let rest = rest.trim();
                    if let Some(rest) = rest.strip_prefix('=') {
                        let rest = rest.trim();
                        if rest.starts_with('"') && rest.ends_with('"') {
                            return Some(rest[1..rest.len() - 1].to_string());
                        }
                    }
                }
            }
        }
    }
    None
}

/// Extract `#[graph(tag = N)]` attribute
fn get_tag(attrs: &[Attribute]) -> Option<usize> {
    for attr in attrs {
        if attr.path().is_ident("graph") {
            if let Meta::List(list) = &attr.meta {
                let tokens = list.tokens.to_string();
                // Parse tag = N
                if let Some(rest) = tokens.strip_prefix("tag") {
                    let rest = rest.trim();
                    if let Some(rest) = rest.strip_prefix('=') {
                        let rest = rest.trim();
                        return rest.parse().ok();
                    }
                }
            }
        }
    }
    None
}
