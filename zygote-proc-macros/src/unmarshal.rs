//
// Copyright (C) 2025 The Android Open-Source Project
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//      http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use heck::ToSnakeCase;
use proc_macro2::TokenStream;
use quote::quote;
use syn::{Attribute, Data, DeriveInput, Expr, Field, Fields, GenericParam, Ident, Type, Variant};

use crate::utils::{
    get_flatten_into_ident, get_inner_type_ident, get_inner_type_ident_from_variant,
};

// Generates the constructor method if #[unmarshal_from] is specified,
// and FromParcel trait implementation otherwise.
// #[unmarshal_from] expects two named values, `source_type` and `field`.
pub(crate) fn gen_unmarshal_parcel(ast: &DeriveInput) -> TokenStream {
    match get_unmarshal_from(ast) {
        Some((ty, field)) => gen_unmarshal_from(ast, &ty, &field),
        None => match &ast.data {
            Data::Enum(_) => gen_from_parcel(ast),
            Data::Struct(_) => gen_unmarshal_struct(ast),
            _ => syn::Error::new(
                ast.ident.span(),
                "Parcelable can only be derived for enums or structs",
            )
            .to_compile_error(),
        },
    }
}

// Parses the #[unmarshal_from] attribute. The fields `source_type` and `field` are required.
fn get_unmarshal_from(ast: &DeriveInput) -> Option<(Type, Ident)> {
    ast.attrs.iter().find_map(|attr| {
        if !attr.path().is_ident("unmarshal_from") {
            return None;
        }
        let mut source_type = None::<Type>;
        let mut field = None::<Ident>;
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("source_type") {
                source_type = Some(meta.value()?.parse::<Type>()?);
                Ok(())
            } else if meta.path.is_ident("field") {
                field = Some(meta.value()?.parse::<Ident>()?);
                Ok(())
            } else {
                Err(meta.error("unrecognized unmarshal_from attribute"))
            }
        })
        .ok()?;
        Some((source_type.expect("source_type not found"), field.expect("field not found")))
    })
}

fn gen_unmarshal_struct(ast: &DeriveInput) -> TokenStream {
    let name = &ast.ident;
    let (impl_generics, ty_generics, where_clause) = ast.generics.split_for_impl();
    let inner_name = get_inner_type_ident(&ast.attrs).unwrap_or_else(|| name.clone());

    let fields_named = match &ast.data {
        Data::Struct(s) => match &s.fields {
            Fields::Named(f) => &f.named,
            _ => {
                return syn::Error::new(
                    name.span(),
                    "Unmarshalable can only be derived for structs with named fields.",
                )
                .to_compile_error()
            }
        },
        _ => {
            return syn::Error::new(name.span(), "Unmarshalable can only be derived for structs.")
                .to_compile_error()
        }
    };
    let field_values = fields_named.iter().map(|field| gen_field_value(field, &quote! { source }));

    quote! {
        impl #impl_generics #name #ty_generics #where_clause {
            fn from_table(source: inner::#inner_name<'_>) -> Self {
                Self {
                    #(#field_values),*,
                }
            }
        }
    }
}

// Generates the FromParcel trait implementation.
fn gen_from_parcel(ast: &DeriveInput) -> TokenStream {
    let name = &ast.ident;
    let where_clause = ast.generics.where_clause.as_ref();

    let variants = match &ast.data {
        Data::Enum(data) => &data.variants,
        _ => {
            return syn::Error::new(name.span(), "Parcelable can only be derived for enums")
                .to_compile_error()
        }
    };
    let unmarshal_arms = variants
        .iter()
        .map(|v| gen_unmarshal_arm(v, name, &quote! { parcel }, &quote::format_ident!("message")));
    let ty_generics = ast.generics.params.iter().map(|p| match p {
        GenericParam::Lifetime(_) => quote! { 'a },
        _ => quote! { p },
    });

    quote! {
        impl<'a> FromParcel<'a> for #name <#(#ty_generics),*> #where_clause {
            fn try_from_parcel(buffer: &'a [u8]) -> Result<Self> {
                let parcel: inner::Parcel<'a> = flatbuffers::root::<inner::Parcel>(buffer)?;
                match parcel.message_type() {
                    #(#unmarshal_arms),*
                    inner::#name(tag) => anyhow::bail!("Unknown #name type: {tag}"),
                }
            }
        }
    }
}

// Generates the constructor for the enum, which takes a value of `source_type` and a field name.
// This method is used to construct a value from a union in flatbuffer.
// For example, for SpawnPayload, which specifies #[unmarshal_from(source_type = inner::Spawn<'a>, field = payload)],
// the generated code looks like the following:
// ```
// impl<'a> SpawnPayload<'a> {
//   fn from_spawn(source: &inner::Spawn<'a>) -> Self {
//     match source.payload_type() {
//       inner::SpawnPayload::SpawnMock => {
//         let payload: inner::SpawnMock<'a> = source.payload_as_spawn_mock().unwrap();
//         Ok(SpawnPayload::Mock { name: payload.name() })
//       }
//       ..
//       inner::SpawnPayload(tag) => {
//         bail!("Unknown SpawnPayload type: {tag}")
//       }
//     }
//   }
// }
// ```
fn gen_unmarshal_from(ast: &DeriveInput, source_type: &Type, field_name: &Ident) -> TokenStream {
    let name = &ast.ident;
    let (impl_generics, ty_generics, where_clause) = ast.generics.split_for_impl();

    let variants = match &ast.data {
        Data::Enum(data) => &data.variants,
        _ => {
            return syn::Error::new(name.span(), "Parcelable can only be derived for enums")
                .to_compile_error()
        }
    };
    let unmarshal_arms =
        variants.iter().map(|v| gen_unmarshal_arm(v, name, &quote! { source }, field_name));
    let type_method = quote::format_ident!("{}_type", field_name);

    quote! {
        impl #impl_generics #name #ty_generics #where_clause {
            fn from(source: #source_type) -> Result<Self> {
                match source.#type_method() {
                    #(#unmarshal_arms),*
                    inner::#name(tag) => anyhow::bail!("Unknown #name type: {tag}"),
                }
            }
        }
    }
}

// Generates an match arm for unmarshaling a value.
// The generated code for Message looks like
// ```
// inner::Message::Exit => Ok(Message::Exit),
// inner::Message::IdentityQuery => Ok(Message::IdentityQuery),
// ..
// inner::Message::SpawnResponse => {
//   let spawn_response = parcel.message_as_spawn_response().unwrap();
//   Ok(Message::SpawnResponse { pid: spawn_response.pid() })
// }
// ..
// ```
fn gen_unmarshal_arm(
    variant: &Variant,
    enum_name: &Ident,
    source: &TokenStream,
    field_name: &Ident,
) -> TokenStream {
    let variant_id = &variant.ident;
    let inner_type_id = get_inner_type_ident_from_variant(variant);
    match &variant.fields {
        Fields::Unit => {
            quote! { inner::#enum_name::#inner_type_id => Ok(#enum_name::#variant_id) }
        }
        Fields::Named(fields) => {
            let parsed_variant =
                Ident::new(&variant_id.to_string().to_snake_case(), variant_id.span());
            let field_values = fields
                .named
                .iter()
                .map(|field| gen_field_value(field, &quote! { #parsed_variant }));
            let inner_type_snake_name = inner_type_id.to_string().to_snake_case();
            let cast_method = quote::format_ident!("{}_as_{}", field_name, inner_type_snake_name);
            quote! {
                inner::#enum_name::#inner_type_id => {
                    let #parsed_variant = #source.#cast_method().unwrap();
                    Ok(#enum_name::#variant_id {
                        #(#field_values),*
                    })
                }
            }
        }
        Fields::Unnamed(_) => quote! {
            #enum_name::#variant_id(..) => {
                todo!("marshal for this variant is not yet implemented")
            }
        },
    }
}

// Generates an internal unflatten() method.
// This method takes a value of a type specified by #[flatten_into_type = ..],
// which contains the flattened values, and constructs an instance of this struct.
// The resulting code for SpawnParamsCommon looks like
// ```
// impl SpawnParamsCommon {
//   fn unflatten(source: inner::Spawn<'_>) -> Self {
//     Self {
//       uid: source.uid(),
//       gid: source.gid(),
//       ..
//     }
//   }
// }
// ```
pub(crate) fn gen_flatten_unmarshal_parcel(ast: &DeriveInput) -> TokenStream {
    let name = &ast.ident;

    let fields_named = match &ast.data {
        Data::Struct(s) => match &s.fields {
            Fields::Named(f) => f,
            _ => {
                return syn::Error::new(
                    name.span(),
                    "FlattenParcel can only be derived for structs with named fields.",
                )
                .to_compile_error()
            }
        },
        _ => {
            return syn::Error::new(name.span(), "FlattenParcel can only be derived for structs.")
                .to_compile_error()
        }
    };
    let field_values =
        fields_named.named.iter().map(|field| gen_field_value(field, &quote! { source }));
    let flatten_into = get_flatten_into_ident(&ast.attrs);

    quote! {
        impl #name {
            fn unflatten(source: inner::#flatten_into<'_>) -> Self {
                Self {
                    #(#field_values),*,
                }
            }
        }
    }
}

// Sets a value of the field #field_name of the given #args, depending on the #[unmarshal(..)] attribute
fn gen_field_value(field: &Field, source: &TokenStream) -> TokenStream {
    let field_name = field.ident.as_ref().unwrap();
    let ty = &field.ty;
    match UnmarshalAttr::new(field) {
        Some(UnmarshalAttr::ValidRange(range)) => quote! {
            #field_name: if #range.contains(&#source.#field_name()) {
                Some(#source.#field_name())
            } else {
                None
            }
        },
        Some(UnmarshalAttr::Map(map)) => quote! { #field_name: #map(&#source.#field_name()) },
        Some(UnmarshalAttr::Table) => {
            quote! { #field_name: <#ty>::from_table(#source.#field_name().unwrap()) }
        }
        Some(UnmarshalAttr::Flatten) => quote! { #field_name: <#ty>::unflatten(#source) },
        Some(UnmarshalAttr::Union) => quote! { #field_name: <#ty>::from(#source)?  },
        None => quote! { #field_name: #source.#field_name() },
    }
}

// Parses #[unmarshal(..)] attributes.
enum UnmarshalAttr {
    // #[unmarshal(valid_range = <expr>)]
    // Expects a range. If the value is out of range, None is set.
    ValidRange(Box<Expr>),
    // #[unmarshal(map = <fn>)]
    // Maps a value when deserializing based on the given function.
    Map(Ident),
    // #[table]
    // Indicates that the field is an inner table, to be unmarshaled using the field's
    // `UnmarshalParcel` implementation. This attribute is also used in MarshalParcel.
    Table,
    // #[union]
    // Treat the flatbuffer's type as union, which has another field `<field_name>_type`, indicating the variant.
    // This attribute is also used in MarshalParcel.
    Union,
    // #[flatten]
    // Calls the unflatten() defined in gen_flatten_unmarshal_parcel() when deserializing.
    // This attribute is also used in MarshalParcel.
    Flatten,
}

impl UnmarshalAttr {
    fn new(field: &Field) -> Option<Self> {
        field.attrs.iter().find_map(Self::from_attr)
    }

    fn from_attr(attr: &Attribute) -> Option<Self> {
        if attr.path().is_ident("flatten") {
            return Some(Self::Flatten);
        }
        if attr.path().is_ident("table") {
            return Some(Self::Table);
        }
        if attr.path().is_ident("union") {
            return Some(Self::Union);
        }

        if !attr.path().is_ident("unmarshal") {
            return None;
        }

        let mut ret = None::<Self>;

        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("valid_range") {
                let expr = meta.value()?.parse::<Expr>()?;
                ret = Some(Self::ValidRange(Box::new(expr)));
                Ok(())
            } else if meta.path.is_ident("map") {
                let map = meta.value()?.parse::<Ident>()?;
                ret = Some(Self::Map(map));
                Ok(())
            } else {
                Err(meta.error("unsupported unmarshal attribute"))
            }
        })
        .expect("error parsing unmarshal attribute");

        ret
    }
}
