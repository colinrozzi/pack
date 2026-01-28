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

use proc_macro::TokenStream;
use quote::{quote, format_ident};
use syn::{
    parse_macro_input, Data, DeriveInput, Fields,
    Attribute, Meta,
};

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
/// - `#[graph(rename = "name")]` - Use a different name for field/variant
/// - `#[graph(tag = N)]` - Use explicit tag number for variant
#[proc_macro_derive(GraphValue, attributes(graph))]
pub fn derive_graph_value(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);

    let expanded = match &input.data {
        Data::Struct(data) => derive_struct(&input, data),
        Data::Enum(data) => derive_enum(&input, data),
        Data::Union(_) => {
            return syn::Error::new_spanned(&input, "GraphValue cannot be derived for unions")
                .to_compile_error()
                .into();
        }
    };

    expanded.into()
}

fn derive_struct(input: &DeriveInput, data: &syn::DataStruct) -> proc_macro2::TokenStream {
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
                            .ok_or_else(|| composite_abi::ConversionError::MissingField(
                                composite_abi::__private::String::from(#field_name_str)
                            ))?;
                        <#field_type as composite_abi::__private::TryFrom<composite_abi::Value>>::try_from(field_value)
                            .map_err(|e| composite_abi::ConversionError::FieldError(
                                composite_abi::__private::String::from(#field_name_str),
                                composite_abi::__private::Box::new(e)
                            ))?
                    }
                }
            }).collect();

            let field_count = fields.named.len();

            // Generate field accessors for From impl
            let field_accessors: Vec<_> = fields.named.iter().map(|f| {
                let field_name = f.ident.as_ref().unwrap();
                let field_name_str = get_rename(&f.attrs).unwrap_or_else(|| field_name.to_string());
                quote! {
                    (
                        composite_abi::__private::String::from(#field_name_str),
                        composite_abi::Value::from(value.#field_name)
                    )
                }
            }).collect();

            quote! {
                impl #impl_generics composite_abi::__private::From<#name #ty_generics> for composite_abi::Value #where_clause {
                    fn from(value: #name #ty_generics) -> composite_abi::Value {
                        composite_abi::Value::Record(composite_abi::__private::vec![
                            #(#field_accessors),*
                        ])
                    }
                }

                impl #impl_generics composite_abi::__private::TryFrom<composite_abi::Value> for #name #ty_generics #where_clause {
                    type Error = composite_abi::ConversionError;

                    fn try_from(value: composite_abi::Value) -> composite_abi::__private::Result<Self, Self::Error> {
                        match value {
                            composite_abi::Value::Record(fields) => {
                                if fields.len() != #field_count {
                                    return composite_abi::__private::Err(composite_abi::ConversionError::WrongFieldCount {
                                        expected: #field_count,
                                        got: fields.len(),
                                    });
                                }
                                composite_abi::__private::Ok(Self {
                                    #(#field_from_value),*
                                })
                            }
                            other => composite_abi::__private::Err(composite_abi::ConversionError::ExpectedRecord(
                                composite_abi::__private::format!("{:?}", other)
                            )),
                        }
                    }
                }
            }
        }
        Fields::Unnamed(fields) => {
            // Tuple struct -> Value::Tuple
            let field_indices: Vec<_> = (0..fields.unnamed.len())
                .map(syn::Index::from)
                .collect();

            let field_from_value: Vec<_> = fields.unnamed.iter().enumerate().map(|(i, f)| {
                let field_type = &f.ty;
                quote! {
                    <#field_type as composite_abi::__private::TryFrom<composite_abi::Value>>::try_from(
                        fields.get(#i).cloned().ok_or_else(|| composite_abi::ConversionError::MissingIndex(#i))?
                    ).map_err(|e| composite_abi::ConversionError::IndexError(#i, composite_abi::__private::Box::new(e)))?
                }
            }).collect();

            let field_count = fields.unnamed.len();

            quote! {
                impl #impl_generics composite_abi::__private::From<#name #ty_generics> for composite_abi::Value #where_clause {
                    fn from(value: #name #ty_generics) -> composite_abi::Value {
                        composite_abi::Value::Tuple(composite_abi::__private::vec![
                            #(composite_abi::Value::from(value.#field_indices)),*
                        ])
                    }
                }

                impl #impl_generics composite_abi::__private::TryFrom<composite_abi::Value> for #name #ty_generics #where_clause {
                    type Error = composite_abi::ConversionError;

                    fn try_from(value: composite_abi::Value) -> composite_abi::__private::Result<Self, Self::Error> {
                        match value {
                            composite_abi::Value::Tuple(fields) => {
                                if fields.len() != #field_count {
                                    return composite_abi::__private::Err(composite_abi::ConversionError::WrongFieldCount {
                                        expected: #field_count,
                                        got: fields.len(),
                                    });
                                }
                                composite_abi::__private::Ok(Self(
                                    #(#field_from_value),*
                                ))
                            }
                            other => composite_abi::__private::Err(composite_abi::ConversionError::ExpectedTuple(
                                composite_abi::__private::format!("{:?}", other)
                            )),
                        }
                    }
                }
            }
        }
        Fields::Unit => {
            // Unit struct -> Value::Tuple([])
            quote! {
                impl #impl_generics composite_abi::__private::From<#name #ty_generics> for composite_abi::Value #where_clause {
                    fn from(_: #name #ty_generics) -> composite_abi::Value {
                        composite_abi::Value::Tuple(composite_abi::__private::vec![])
                    }
                }

                impl #impl_generics composite_abi::__private::TryFrom<composite_abi::Value> for #name #ty_generics #where_clause {
                    type Error = composite_abi::ConversionError;

                    fn try_from(value: composite_abi::Value) -> composite_abi::__private::Result<Self, Self::Error> {
                        match value {
                            composite_abi::Value::Tuple(fields) if fields.is_empty() => {
                                composite_abi::__private::Ok(Self)
                            }
                            composite_abi::Value::Tuple(fields) => {
                                composite_abi::__private::Err(composite_abi::ConversionError::WrongFieldCount {
                                    expected: 0,
                                    got: fields.len(),
                                })
                            }
                            other => composite_abi::__private::Err(composite_abi::ConversionError::ExpectedTuple(
                                composite_abi::__private::format!("{:?}", other)
                            )),
                        }
                    }
                }
            }
        }
    }
}

fn derive_enum(input: &DeriveInput, data: &syn::DataEnum) -> proc_macro2::TokenStream {
    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    // Generate match arms for From<T> for Value
    let to_value_arms: Vec<_> = data.variants.iter().enumerate().map(|(default_tag, variant)| {
        let variant_name = &variant.ident;
        let tag = get_tag(&variant.attrs).unwrap_or(default_tag);

        match &variant.fields {
            Fields::Named(fields) => {
                let field_names: Vec<_> = fields.named.iter()
                    .map(|f| f.ident.as_ref().unwrap())
                    .collect();
                let field_to_value: Vec<_> = fields.named.iter().map(|f| {
                    let field_name = f.ident.as_ref().unwrap();
                    let field_name_str = get_rename(&f.attrs).unwrap_or_else(|| field_name.to_string());
                    quote! {
                        (
                            composite_abi::__private::String::from(#field_name_str),
                            composite_abi::Value::from(#field_name)
                        )
                    }
                }).collect();

                quote! {
                    #name::#variant_name { #(#field_names),* } => {
                        composite_abi::Value::Variant {
                            tag: #tag,
                            payload: composite_abi::__private::Some(composite_abi::__private::Box::new(
                                composite_abi::Value::Record(composite_abi::__private::vec![#(#field_to_value),*])
                            )),
                        }
                    }
                }
            }
            Fields::Unnamed(fields) => {
                let field_names: Vec<_> = (0..fields.unnamed.len())
                    .map(|i| format_ident!("f{}", i))
                    .collect();

                if fields.unnamed.len() == 1 {
                    // Single field - payload is just that value
                    let f0 = &field_names[0];
                    quote! {
                        #name::#variant_name(#(#field_names),*) => {
                            composite_abi::Value::Variant {
                                tag: #tag,
                                payload: composite_abi::__private::Some(composite_abi::__private::Box::new(
                                    composite_abi::Value::from(#f0)
                                )),
                            }
                        }
                    }
                } else {
                    // Multiple fields - payload is a tuple
                    quote! {
                        #name::#variant_name(#(#field_names),*) => {
                            composite_abi::Value::Variant {
                                tag: #tag,
                                payload: composite_abi::__private::Some(composite_abi::__private::Box::new(
                                    composite_abi::Value::Tuple(composite_abi::__private::vec![
                                        #(composite_abi::Value::from(#field_names)),*
                                    ])
                                )),
                            }
                        }
                    }
                }
            }
            Fields::Unit => {
                quote! {
                    #name::#variant_name => {
                        composite_abi::Value::Variant {
                            tag: #tag,
                            payload: composite_abi::__private::None,
                        }
                    }
                }
            }
        }
    }).collect();

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
                                .ok_or_else(|| composite_abi::ConversionError::MissingField(
                                    composite_abi::__private::String::from(#field_name_str)
                                ))?;
                            <#field_type as composite_abi::__private::TryFrom<composite_abi::Value>>::try_from(field_value)
                                .map_err(|e| composite_abi::ConversionError::FieldError(
                                    composite_abi::__private::String::from(#field_name_str),
                                    composite_abi::__private::Box::new(e)
                                ))?
                        }
                    }
                }).collect();

                quote! {
                    #tag => {
                        let payload = payload.ok_or_else(|| composite_abi::ConversionError::MissingPayload)?;
                        match *payload {
                            composite_abi::Value::Record(record_fields) => {
                                composite_abi::__private::Ok(#name::#variant_name {
                                    #(#field_from_value),*
                                })
                            }
                            other => composite_abi::__private::Err(composite_abi::ConversionError::ExpectedRecord(
                                composite_abi::__private::format!("{:?}", other)
                            )),
                        }
                    }
                }
            }
            Fields::Unnamed(fields) => {
                if fields.unnamed.len() == 1 {
                    let field_type = &fields.unnamed[0].ty;
                    quote! {
                        #tag => {
                            let payload = payload.ok_or_else(|| composite_abi::ConversionError::MissingPayload)?;
                            let value = <#field_type as composite_abi::__private::TryFrom<composite_abi::Value>>::try_from(*payload)
                                .map_err(|e| composite_abi::ConversionError::PayloadError(composite_abi::__private::Box::new(e)))?;
                            composite_abi::__private::Ok(#name::#variant_name(value))
                        }
                    }
                } else {
                    let field_conversions: Vec<_> = fields.unnamed.iter().enumerate().map(|(i, f)| {
                        let field_type = &f.ty;
                        quote! {
                            <#field_type as composite_abi::__private::TryFrom<composite_abi::Value>>::try_from(
                                tuple_fields.get(#i).cloned().ok_or_else(|| composite_abi::ConversionError::MissingIndex(#i))?
                            ).map_err(|e| composite_abi::ConversionError::IndexError(#i, composite_abi::__private::Box::new(e)))?
                        }
                    }).collect();

                    let field_count = fields.unnamed.len();

                    quote! {
                        #tag => {
                            let payload = payload.ok_or_else(|| composite_abi::ConversionError::MissingPayload)?;
                            match *payload {
                                composite_abi::Value::Tuple(tuple_fields) => {
                                    if tuple_fields.len() != #field_count {
                                        return composite_abi::__private::Err(composite_abi::ConversionError::WrongFieldCount {
                                            expected: #field_count,
                                            got: tuple_fields.len(),
                                        });
                                    }
                                    composite_abi::__private::Ok(#name::#variant_name(
                                        #(#field_conversions),*
                                    ))
                                }
                                other => composite_abi::__private::Err(composite_abi::ConversionError::ExpectedTuple(
                                    composite_abi::__private::format!("{:?}", other)
                                )),
                            }
                        }
                    }
                }
            }
            Fields::Unit => {
                quote! {
                    #tag => {
                        if payload.is_some() {
                            return composite_abi::__private::Err(composite_abi::ConversionError::UnexpectedPayload);
                        }
                        composite_abi::__private::Ok(#name::#variant_name)
                    }
                }
            }
        }
    }).collect();

    let variant_count = data.variants.len();

    quote! {
        impl #impl_generics composite_abi::__private::From<#name #ty_generics> for composite_abi::Value #where_clause {
            fn from(value: #name #ty_generics) -> composite_abi::Value {
                match value {
                    #(#to_value_arms),*
                }
            }
        }

        impl #impl_generics composite_abi::__private::TryFrom<composite_abi::Value> for #name #ty_generics #where_clause {
            type Error = composite_abi::ConversionError;

            fn try_from(value: composite_abi::Value) -> composite_abi::__private::Result<Self, Self::Error> {
                match value {
                    composite_abi::Value::Variant { tag, payload } => {
                        match tag {
                            #(#from_value_arms),*
                            other => composite_abi::__private::Err(composite_abi::ConversionError::UnknownTag {
                                tag: other,
                                max: #variant_count,
                            }),
                        }
                    }
                    other => composite_abi::__private::Err(composite_abi::ConversionError::ExpectedVariant(
                        composite_abi::__private::format!("{:?}", other)
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
                            return Some(rest[1..rest.len()-1].to_string());
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
