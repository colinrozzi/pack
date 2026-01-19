//! Code generator for WIT+ types.
//!
//! Takes WIT+ type definitions and generates Rust types with From/TryFrom
//! implementations for Value conversion.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::wit_parser::{Type, TypeDef, VariantCase, World, Function, WorldItem};

/// Convert a WIT identifier (kebab-case) to Rust identifier (PascalCase for types, snake_case for functions)
fn to_rust_type_name(wit_name: &str) -> syn::Ident {
    let pascal = wit_name
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

fn to_rust_field_name(wit_name: &str) -> syn::Ident {
    let snake = wit_name.replace('-', "_");
    format_ident!("{}", snake)
}

fn to_rust_variant_name(wit_name: &str) -> syn::Ident {
    // Same as type name - PascalCase
    to_rust_type_name(wit_name)
}

/// Generate Rust type reference from WIT+ type
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
            let inner_ty = generate_type_ref(inner, self_type_name);
            quote! { ::alloc::vec::Vec<#inner_ty> }
        }
        Type::Option(inner) => {
            let inner_ty = generate_type_ref(inner, self_type_name);
            quote! { ::core::option::Option<#inner_ty> }
        }
        Type::Result { ok, err } => {
            let ok_ty = ok.as_ref()
                .map(|t| generate_type_ref(t, self_type_name))
                .unwrap_or_else(|| quote! { () });
            let err_ty = err.as_ref()
                .map(|t| generate_type_ref(t, self_type_name))
                .unwrap_or_else(|| quote! { () });
            quote! { ::core::result::Result<#ok_ty, #err_ty> }
        }
        Type::Tuple(items) => {
            if items.is_empty() {
                quote! { () }
            } else {
                let item_tys: Vec<_> = items.iter()
                    .map(|t| generate_type_ref(t, self_type_name))
                    .collect();
                quote! { (#(#item_tys),*) }
            }
        }
        Type::Named(name) => {
            let rust_name = to_rust_type_name(name);
            quote! { #rust_name }
        }
        Type::SelfRef => {
            if let Some(name) = self_type_name {
                let rust_name = to_rust_type_name(name);
                quote! { ::alloc::boxed::Box<#rust_name> }
            } else {
                // Shouldn't happen in valid WIT+
                quote! { Self }
            }
        }
    }
}

/// Generate Value conversion expression for a type (Rust value -> Value)
fn generate_to_value(ty: &Type, expr: TokenStream, self_type_name: Option<&str>) -> TokenStream {
    match ty {
        Type::Bool => quote! { composite_guest::Value::Bool(#expr) },
        Type::U8 => quote! { composite_guest::Value::U8(#expr) },
        Type::U16 => quote! { composite_guest::Value::U16(#expr) },
        Type::U32 => quote! { composite_guest::Value::U32(#expr) },
        Type::U64 => quote! { composite_guest::Value::U64(#expr) },
        Type::S8 => quote! { composite_guest::Value::S8(#expr) },
        Type::S16 => quote! { composite_guest::Value::S16(#expr) },
        Type::S32 => quote! { composite_guest::Value::S32(#expr) },
        Type::S64 => quote! { composite_guest::Value::S64(#expr) },
        Type::F32 => quote! { composite_guest::Value::F32(#expr) },
        Type::F64 => quote! { composite_guest::Value::F64(#expr) },
        Type::Char => quote! { composite_guest::Value::Char(#expr) },
        Type::String => quote! { composite_guest::Value::String(#expr) },
        Type::List(inner) => {
            let inner_conversion = generate_to_value(inner, quote! { item }, self_type_name);
            quote! {
                composite_guest::Value::List(
                    #expr.into_iter().map(|item| #inner_conversion).collect()
                )
            }
        }
        Type::Option(inner) => {
            let inner_conversion = generate_to_value(inner, quote! { v }, self_type_name);
            quote! {
                composite_guest::Value::Option(
                    #expr.map(|v| ::alloc::boxed::Box::new(#inner_conversion))
                )
            }
        }
        Type::Result { ok, err } => {
            let ok_conversion = ok.as_ref()
                .map(|t| generate_to_value(t, quote! { v }, self_type_name))
                .unwrap_or_else(|| quote! { composite_guest::Value::Tuple(::alloc::vec![]) });
            let err_conversion = err.as_ref()
                .map(|t| generate_to_value(t, quote! { e }, self_type_name))
                .unwrap_or_else(|| quote! { composite_guest::Value::Tuple(::alloc::vec![]) });
            quote! {
                match #expr {
                    Ok(v) => composite_guest::Value::Variant {
                        tag: 0,
                        payload: Some(::alloc::boxed::Box::new(#ok_conversion)),
                    },
                    Err(e) => composite_guest::Value::Variant {
                        tag: 1,
                        payload: Some(::alloc::boxed::Box::new(#err_conversion)),
                    },
                }
            }
        }
        Type::Tuple(items) => {
            if items.is_empty() {
                quote! { composite_guest::Value::Tuple(::alloc::vec![]) }
            } else {
                let conversions: Vec<_> = items.iter().enumerate()
                    .map(|(i, t)| {
                        let idx = syn::Index::from(i);
                        let item_expr = quote! { #expr.#idx };
                        generate_to_value(t, item_expr, self_type_name)
                    })
                    .collect();
                quote! {
                    composite_guest::Value::Tuple(::alloc::vec![#(#conversions),*])
                }
            }
        }
        Type::Named(_) | Type::SelfRef => {
            // Named types and self-refs implement Into<Value>
            quote! { composite_guest::Value::from(#expr) }
        }
    }
}

/// Generate Value extraction expression for a type (Value -> Rust value)
fn generate_from_value(ty: &Type, expr: TokenStream, self_type_name: Option<&str>) -> TokenStream {
    match ty {
        Type::Bool => quote! {
            match #expr {
                composite_guest::Value::Bool(v) => v,
                _ => return Err(composite_guest::ConversionError::TypeMismatch {
                    expected: "Bool".into(),
                    got: ::alloc::format!("{:?}", #expr),
                }),
            }
        },
        Type::U8 => quote! {
            match #expr {
                composite_guest::Value::U8(v) => v,
                _ => return Err(composite_guest::ConversionError::TypeMismatch {
                    expected: "U8".into(),
                    got: ::alloc::format!("{:?}", #expr),
                }),
            }
        },
        Type::U16 => quote! {
            match #expr {
                composite_guest::Value::U16(v) => v,
                _ => return Err(composite_guest::ConversionError::TypeMismatch {
                    expected: "U16".into(),
                    got: ::alloc::format!("{:?}", #expr),
                }),
            }
        },
        Type::U32 => quote! {
            match #expr {
                composite_guest::Value::U32(v) => v,
                _ => return Err(composite_guest::ConversionError::TypeMismatch {
                    expected: "U32".into(),
                    got: ::alloc::format!("{:?}", #expr),
                }),
            }
        },
        Type::U64 => quote! {
            match #expr {
                composite_guest::Value::U64(v) => v,
                _ => return Err(composite_guest::ConversionError::TypeMismatch {
                    expected: "U64".into(),
                    got: ::alloc::format!("{:?}", #expr),
                }),
            }
        },
        Type::S8 => quote! {
            match #expr {
                composite_guest::Value::S8(v) => v,
                _ => return Err(composite_guest::ConversionError::TypeMismatch {
                    expected: "S8".into(),
                    got: ::alloc::format!("{:?}", #expr),
                }),
            }
        },
        Type::S16 => quote! {
            match #expr {
                composite_guest::Value::S16(v) => v,
                _ => return Err(composite_guest::ConversionError::TypeMismatch {
                    expected: "S16".into(),
                    got: ::alloc::format!("{:?}", #expr),
                }),
            }
        },
        Type::S32 => quote! {
            match #expr {
                composite_guest::Value::S32(v) => v,
                _ => return Err(composite_guest::ConversionError::TypeMismatch {
                    expected: "S32".into(),
                    got: ::alloc::format!("{:?}", #expr),
                }),
            }
        },
        Type::S64 => quote! {
            match #expr {
                composite_guest::Value::S64(v) => v,
                _ => return Err(composite_guest::ConversionError::TypeMismatch {
                    expected: "S64".into(),
                    got: ::alloc::format!("{:?}", #expr),
                }),
            }
        },
        Type::F32 => quote! {
            match #expr {
                composite_guest::Value::F32(v) => v,
                _ => return Err(composite_guest::ConversionError::TypeMismatch {
                    expected: "F32".into(),
                    got: ::alloc::format!("{:?}", #expr),
                }),
            }
        },
        Type::F64 => quote! {
            match #expr {
                composite_guest::Value::F64(v) => v,
                _ => return Err(composite_guest::ConversionError::TypeMismatch {
                    expected: "F64".into(),
                    got: ::alloc::format!("{:?}", #expr),
                }),
            }
        },
        Type::Char => quote! {
            match #expr {
                composite_guest::Value::Char(v) => v,
                _ => return Err(composite_guest::ConversionError::TypeMismatch {
                    expected: "Char".into(),
                    got: ::alloc::format!("{:?}", #expr),
                }),
            }
        },
        Type::String => quote! {
            match #expr {
                composite_guest::Value::String(v) => v,
                _ => return Err(composite_guest::ConversionError::TypeMismatch {
                    expected: "String".into(),
                    got: ::alloc::format!("{:?}", #expr),
                }),
            }
        },
        Type::List(inner) => {
            // For list<self>, we need to handle Box wrapping specially
            let item_conversion = if matches!(inner.as_ref(), Type::SelfRef) {
                if let Some(name) = self_type_name {
                    let rust_name = to_rust_type_name(name);
                    quote! { ::alloc::boxed::Box::new(<#rust_name>::try_from(item)?) }
                } else {
                    quote! { ::alloc::boxed::Box::new(Self::try_from(item)?) }
                }
            } else {
                let inner_ty = generate_type_ref(inner, self_type_name);
                quote! { <#inner_ty>::try_from(item)? }
            };
            quote! {
                match #expr {
                    composite_guest::Value::List(items) => {
                        let mut result = ::alloc::vec::Vec::with_capacity(items.len());
                        for item in items {
                            result.push(#item_conversion);
                        }
                        result
                    }
                    _ => return Err(composite_guest::ConversionError::ExpectedList(
                        ::alloc::format!("{:?}", #expr)
                    )),
                }
            }
        }
        Type::Option(inner) => {
            // For option<self>, we need to handle Box wrapping specially
            let some_conversion = if matches!(inner.as_ref(), Type::SelfRef) {
                if let Some(name) = self_type_name {
                    let rust_name = to_rust_type_name(name);
                    quote! { Some(::alloc::boxed::Box::new(<#rust_name>::try_from(*boxed)?)) }
                } else {
                    quote! { Some(::alloc::boxed::Box::new(Self::try_from(*boxed)?)) }
                }
            } else {
                let inner_ty = generate_type_ref(inner, self_type_name);
                quote! { Some(<#inner_ty>::try_from(*boxed)?) }
            };
            quote! {
                match #expr {
                    composite_guest::Value::Option(opt) => {
                        match opt {
                            Some(boxed) => #some_conversion,
                            None => None,
                        }
                    }
                    _ => return Err(composite_guest::ConversionError::ExpectedOption(
                        ::alloc::format!("{:?}", #expr)
                    )),
                }
            }
        }
        Type::Result { ok, err } => {
            // Handle SelfRef specially for ok/err types
            let ok_conversion = if ok.as_ref().map(|t| matches!(t.as_ref(), Type::SelfRef)).unwrap_or(false) {
                if let Some(name) = self_type_name {
                    let rust_name = to_rust_type_name(name);
                    quote! { Ok(::alloc::boxed::Box::new(<#rust_name>::try_from(*p)?)) }
                } else {
                    quote! { Ok(::alloc::boxed::Box::new(Self::try_from(*p)?)) }
                }
            } else {
                let ok_ty = ok.as_ref()
                    .map(|t| generate_type_ref(t, self_type_name))
                    .unwrap_or_else(|| quote! { () });
                quote! { Ok(<#ok_ty>::try_from(*p)?) }
            };
            let err_conversion = if err.as_ref().map(|t| matches!(t.as_ref(), Type::SelfRef)).unwrap_or(false) {
                if let Some(name) = self_type_name {
                    let rust_name = to_rust_type_name(name);
                    quote! { Err(::alloc::boxed::Box::new(<#rust_name>::try_from(*p)?)) }
                } else {
                    quote! { Err(::alloc::boxed::Box::new(Self::try_from(*p)?)) }
                }
            } else {
                let err_ty = err.as_ref()
                    .map(|t| generate_type_ref(t, self_type_name))
                    .unwrap_or_else(|| quote! { () });
                quote! { Err(<#err_ty>::try_from(*p)?) }
            };
            quote! {
                match #expr {
                    composite_guest::Value::Variant { tag: 0, payload } => {
                        let p = payload.ok_or(composite_guest::ConversionError::MissingPayload)?;
                        #ok_conversion
                    }
                    composite_guest::Value::Variant { tag: 1, payload } => {
                        let p = payload.ok_or(composite_guest::ConversionError::MissingPayload)?;
                        #err_conversion
                    }
                    _ => return Err(composite_guest::ConversionError::ExpectedVariant(
                        ::alloc::format!("{:?}", #expr)
                    )),
                }
            }
        }
        Type::Tuple(items) => {
            if items.is_empty() {
                quote! { () }
            } else {
                let extractions: Vec<_> = items.iter().enumerate()
                    .map(|(_i, t)| {
                        // Handle SelfRef specially to avoid Box<Self>::try_from issue
                        if matches!(t, Type::SelfRef) {
                            if let Some(name) = self_type_name {
                                let rust_name = to_rust_type_name(name);
                                quote! { ::alloc::boxed::Box::new(<#rust_name>::try_from(iter.next().unwrap())?) }
                            } else {
                                quote! { ::alloc::boxed::Box::new(Self::try_from(iter.next().unwrap())?) }
                            }
                        } else {
                            let ty = generate_type_ref(t, self_type_name);
                            quote! { <#ty>::try_from(iter.next().unwrap())? }
                        }
                    })
                    .collect();
                let len = items.len();
                quote! {
                    match #expr {
                        composite_guest::Value::Tuple(items) if items.len() == #len => {
                            let mut iter = items.into_iter();
                            (#(#extractions),*)
                        }
                        _ => return Err(composite_guest::ConversionError::ExpectedTuple(
                            ::alloc::format!("{:?}", #expr)
                        )),
                    }
                }
            }
        }
        Type::Named(name) => {
            let rust_name = to_rust_type_name(name);
            quote! { <#rust_name>::try_from(#expr)? }
        }
        Type::SelfRef => {
            if let Some(name) = self_type_name {
                let rust_name = to_rust_type_name(name);
                quote! { ::alloc::boxed::Box::new(<#rust_name>::try_from(#expr)?) }
            } else {
                quote! { Self::try_from(#expr)? }
            }
        }
    }
}

/// Generate a complete Rust type definition with From/TryFrom impls
pub fn generate_type_def(typedef: &TypeDef) -> TokenStream {
    match typedef {
        TypeDef::Alias { name, ty } => generate_alias(name, ty),
        TypeDef::Record { name, fields } => generate_record(name, fields),
        TypeDef::Variant { name, cases } => generate_variant(name, cases),
        TypeDef::Enum { name, cases } => generate_enum(name, cases),
        TypeDef::Flags { name, flags } => generate_flags(name, flags),
    }
}

fn generate_alias(name: &str, ty: &Type) -> TokenStream {
    let rust_name = to_rust_type_name(name);
    let rust_ty = generate_type_ref(ty, None);

    quote! {
        pub type #rust_name = #rust_ty;
    }
}

fn generate_record(name: &str, fields: &[(String, Type)]) -> TokenStream {
    let rust_name = to_rust_type_name(name);

    let field_defs: Vec<_> = fields.iter()
        .map(|(fname, ftype)| {
            let rust_fname = to_rust_field_name(fname);
            let rust_ftype = generate_type_ref(ftype, Some(name));
            quote! { pub #rust_fname: #rust_ftype }
        })
        .collect();

    let field_to_value: Vec<_> = fields.iter()
        .map(|(fname, ftype)| {
            let rust_fname = to_rust_field_name(fname);
            let wit_fname = fname.clone();
            let to_val = generate_to_value(ftype, quote! { value.#rust_fname }, Some(name));
            quote! { (#wit_fname.into(), #to_val) }
        })
        .collect();

    let field_from_value: Vec<_> = fields.iter()
        .map(|(fname, ftype)| {
            let rust_fname = to_rust_field_name(fname);
            let wit_fname = fname.clone();
            let from_val = generate_from_value(ftype, quote! { field_value }, Some(name));
            quote! {
                #rust_fname: {
                    let field_value = fields.iter()
                        .find(|(n, _)| n == #wit_fname)
                        .map(|(_, v)| v.clone())
                        .ok_or(composite_guest::ConversionError::MissingField(#wit_fname.into()))?;
                    #from_val
                }
            }
        })
        .collect();

    quote! {
        #[derive(Debug, Clone, PartialEq)]
        pub struct #rust_name {
            #(#field_defs),*
        }

        impl From<#rust_name> for composite_guest::Value {
            fn from(value: #rust_name) -> composite_guest::Value {
                composite_guest::Value::Record(::alloc::vec![#(#field_to_value),*])
            }
        }

        impl TryFrom<composite_guest::Value> for #rust_name {
            type Error = composite_guest::ConversionError;

            fn try_from(value: composite_guest::Value) -> Result<Self, Self::Error> {
                match value {
                    composite_guest::Value::Record(fields) => {
                        Ok(Self {
                            #(#field_from_value),*
                        })
                    }
                    _ => Err(composite_guest::ConversionError::ExpectedRecord(
                        ::alloc::format!("{:?}", value)
                    )),
                }
            }
        }
    }
}

fn generate_variant(name: &str, cases: &[VariantCase]) -> TokenStream {
    let rust_name = to_rust_type_name(name);

    let case_defs: Vec<_> = cases.iter()
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

    let to_value_arms: Vec<_> = cases.iter().enumerate()
        .map(|(tag, case)| {
            let case_name = to_rust_variant_name(&case.name);
            match &case.payload {
                Some(ty) => {
                    let payload_conv = generate_to_value(ty, quote! { payload }, Some(name));
                    quote! {
                        #rust_name::#case_name(payload) => composite_guest::Value::Variant {
                            tag: #tag,
                            payload: Some(::alloc::boxed::Box::new(#payload_conv)),
                        }
                    }
                }
                None => quote! {
                    #rust_name::#case_name => composite_guest::Value::Variant {
                        tag: #tag,
                        payload: None,
                    }
                },
            }
        })
        .collect();

    let from_value_arms: Vec<_> = cases.iter().enumerate()
        .map(|(tag, case)| {
            let case_name = to_rust_variant_name(&case.name);
            match &case.payload {
                Some(ty) => {
                    let payload_conv = generate_from_value(ty, quote! { (*p) }, Some(name));
                    quote! {
                        #tag => {
                            let p = payload.ok_or(composite_guest::ConversionError::MissingPayload)?;
                            Ok(#rust_name::#case_name(#payload_conv))
                        }
                    }
                }
                None => quote! {
                    #tag => Ok(#rust_name::#case_name)
                },
            }
        })
        .collect();

    let max_tag = cases.len();

    quote! {
        #[derive(Debug, Clone, PartialEq)]
        pub enum #rust_name {
            #(#case_defs),*
        }

        impl From<#rust_name> for composite_guest::Value {
            fn from(value: #rust_name) -> composite_guest::Value {
                match value {
                    #(#to_value_arms),*
                }
            }
        }

        impl TryFrom<composite_guest::Value> for #rust_name {
            type Error = composite_guest::ConversionError;

            fn try_from(value: composite_guest::Value) -> Result<Self, Self::Error> {
                match value {
                    composite_guest::Value::Variant { tag, payload } => {
                        match tag {
                            #(#from_value_arms),*
                            _ => Err(composite_guest::ConversionError::UnknownTag {
                                tag,
                                max: #max_tag,
                            }),
                        }
                    }
                    _ => Err(composite_guest::ConversionError::ExpectedVariant(
                        ::alloc::format!("{:?}", value)
                    )),
                }
            }
        }
    }
}

fn generate_enum(name: &str, cases: &[String]) -> TokenStream {
    let rust_name = to_rust_type_name(name);

    let case_defs: Vec<_> = cases.iter()
        .map(|case| to_rust_variant_name(case))
        .collect();

    let to_value_arms: Vec<_> = cases.iter().enumerate()
        .map(|(tag, case)| {
            let case_name = to_rust_variant_name(case);
            quote! {
                #rust_name::#case_name => composite_guest::Value::Variant {
                    tag: #tag,
                    payload: None,
                }
            }
        })
        .collect();

    let from_value_arms: Vec<_> = cases.iter().enumerate()
        .map(|(tag, case)| {
            let case_name = to_rust_variant_name(case);
            quote! { #tag => Ok(#rust_name::#case_name) }
        })
        .collect();

    let max_tag = cases.len();

    quote! {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum #rust_name {
            #(#case_defs),*
        }

        impl From<#rust_name> for composite_guest::Value {
            fn from(value: #rust_name) -> composite_guest::Value {
                match value {
                    #(#to_value_arms),*
                }
            }
        }

        impl TryFrom<composite_guest::Value> for #rust_name {
            type Error = composite_guest::ConversionError;

            fn try_from(value: composite_guest::Value) -> Result<Self, Self::Error> {
                match value {
                    composite_guest::Value::Variant { tag, payload: _ } => {
                        match tag {
                            #(#from_value_arms),*
                            _ => Err(composite_guest::ConversionError::UnknownTag {
                                tag,
                                max: #max_tag,
                            }),
                        }
                    }
                    _ => Err(composite_guest::ConversionError::ExpectedVariant(
                        ::alloc::format!("{:?}", value)
                    )),
                }
            }
        }
    }
}

fn generate_flags(name: &str, flags: &[String]) -> TokenStream {
    let rust_name = to_rust_type_name(name);

    let flag_consts: Vec<_> = flags.iter().enumerate()
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

        impl From<#rust_name> for composite_guest::Value {
            fn from(value: #rust_name) -> composite_guest::Value {
                composite_guest::Value::Flags(value.0)
            }
        }

        impl TryFrom<composite_guest::Value> for #rust_name {
            type Error = composite_guest::ConversionError;

            fn try_from(value: composite_guest::Value) -> Result<Self, Self::Error> {
                match value {
                    composite_guest::Value::Flags(bits) => Ok(#rust_name(bits)),
                    _ => Err(composite_guest::ConversionError::TypeMismatch {
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
    let type_defs: Vec<_> = world.types.iter()
        .map(generate_type_def)
        .collect();

    quote! {
        #(#type_defs)*
    }
}

/// Get export function info from a world
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
