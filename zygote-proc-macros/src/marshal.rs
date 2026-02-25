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

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Attribute, Data, DataEnum, DeriveInput, Error, Expr, Field, Fields, Ident, Variant};

use crate::utils::{
    get_flatten_into_ident, get_inner_type_ident, get_inner_type_ident_from_variant,
};

pub(crate) fn gen_marshal_parcel(ast: &DeriveInput) -> TokenStream {
    let name = &ast.ident;

    let mut generics_for_impl = ast.generics.clone();
    generics_for_impl.params.insert(0, syn::parse_quote!('builder));
    let (impl_generics, _, _) = generics_for_impl.split_for_impl();

    let (_, ty_generics, where_clause) = ast.generics.split_for_impl();

    match &ast.data {
        Data::Enum(data) => {
            gen_marshal_enum(name, data, &impl_generics, &ty_generics, where_clause)
        }
        Data::Struct(data) => {
            if let Fields::Named(fields) = &data.fields {
                let inner_type_name =
                    get_inner_type_ident(&ast.attrs).unwrap_or_else(|| name.clone());
                gen_marshal_struct(
                    name,
                    &inner_type_name,
                    fields,
                    &impl_generics,
                    &ty_generics,
                    where_clause,
                )
            } else {
                Error::new(
                    name.span(),
                    "Parcelable can only be derived for structs with named fields",
                )
                .to_compile_error()
            }
        }
        _ => Error::new(name.span(), "Parcelable can only be derived for enums or structs")
            .to_compile_error(),
    }
}

fn gen_marshal_enum(
    name: &Ident,
    data: &DataEnum,
    impl_generics: &syn::ImplGenerics,
    ty_generics: &syn::TypeGenerics,
    where_clause: Option<&syn::WhereClause>,
) -> TokenStream {
    let inner_type_arms = data.variants.iter().map(|v| {
        let variant_id = &v.ident;
        let inner_type_id = get_inner_type_ident_from_variant(v);

        let pattern = match &v.fields {
            Fields::Unit => quote! {},
            Fields::Named(_) => quote! { {..} },
            Fields::Unnamed(_) => quote! { (..) },
        };

        quote! { #name::#variant_id #pattern => inner::#name::#inner_type_id }
    });

    let marshal_arms = data.variants.iter().map(|v| gen_marshal_arm(v, name));

    quote! {
        impl #impl_generics MarshalParcel<'builder> for #name #ty_generics #where_clause {
            type Inner = inner::#name;
            type Output = flatbuffers::UnionWIPOffset;

            fn inner_type(&self) -> Self::Inner {
                match self {
                    #(#inner_type_arms),*
                }
            }

            fn marshal(
                &self,
                builder: &mut flatbuffers::FlatBufferBuilder<'builder>,
            ) -> flatbuffers::WIPOffset<Self::Output> {
                match self {
                    #(#marshal_arms),*
                }
            }
        }
    }
}

fn gen_marshal_struct(
    name: &Ident,
    inner_type_name: &Ident,
    fields: &syn::FieldsNamed,
    impl_generics: &syn::ImplGenerics,
    ty_generics: &syn::TypeGenerics,
    where_clause: Option<&syn::WhereClause>,
) -> TokenStream {
    let inner_type_args_name = quote::format_ident!("{}Args", inner_type_name);

    let assignments = fields.named.iter().map(|field| {
        let field_name = field.ident.as_ref().expect("Field should have an identifier");
        gen_field_assignment(
            &quote! { args },
            field,
            &quote! { self.#field_name },
            &quote! { builder },
        )
    });

    quote! {
        impl #impl_generics MarshalParcel<'builder> for #name #ty_generics #where_clause {
            type Inner = ();
            type Output = inner::#inner_type_name<'builder>;

            fn inner_type(&self) -> Self::Inner {
                // This is a struct, so it doesn't have a union type.
            }

            fn marshal(
                &self,
                builder: &mut flatbuffers::FlatBufferBuilder<'builder>
            ) -> flatbuffers::WIPOffset<Self::Output> {
                let mut args = inner::#inner_type_args_name::default();
                #(#assignments);*;
                inner::#inner_type_name::create(builder, &args)
            }
        }
    }
}

// Generates the match arms for the marshel() implementation
fn gen_marshal_arm(variant: &Variant, enum_name: &Ident) -> TokenStream {
    let variant_id = &variant.ident;
    let inner_type_id = get_inner_type_ident_from_variant(variant);
    let inner_type_arg_id = Ident::new(&format!("{}Args", inner_type_id), inner_type_id.span());

    match &variant.fields {
        Fields::Unit => quote! {
            #enum_name::#variant_id => {
                inner::#inner_type_id::create(builder, &inner::#inner_type_arg_id {}).as_union_value()
            }
        },
        Fields::Named(fields) => {
            let inner_type_args_name = quote! { inner_type_args };
            let assignments = fields.named.iter().map(|field| {
                let field_name = field.ident.as_ref().expect("Field should have an identifier");
                gen_field_assignment(
                    &inner_type_args_name,
                    field,
                    &quote! { #field_name },
                    &quote! { builder },
                )
            });
            let field_names = fields
                .named
                .iter()
                .map(|f| f.ident.as_ref().expect("Field should have an identifier"));
            quote! {
                #enum_name::#variant_id { #(#field_names),* } => {
                    let mut #inner_type_args_name = inner::#inner_type_arg_id::default();
                    #(#assignments);*;
                    inner::#inner_type_id::create(builder, &#inner_type_args_name).as_union_value()
                }
            }
        }
        Fields::Unnamed(_) => quote! {
            #enum_name::#variant_id(..) => {
                return Error::new(
                    variant_id.span(), "Cannot marshal unnamed fields").to_compile_error();
            }
        },
    }
}

// Geneartes an internal flatten() method.
// This method takes a parameter `args` and sets field values to it.
// For example,
// ```
// #[derive(FlattenParcel)]
// #[flatten_into_type = "Inner"]
// struct Outer {
//   field1: i32,
//   field2: i32,
// }
// ```
// will generate something like the following code
// ```
// impl Outer {
//   fn flatten<'a>(&self,
//                  args: &mut inner::InnerArgs<'a>,
//                  builder: &mut flatbuffers::FlatBufferBuilder<'a>) {
//     args.field1 = self.field1;
//     args.field2 = self.field2;
//   }
// }
// ```
// flatten() method will be called inside `marshal()` when a field has an attribute #[flatten]
pub(crate) fn gen_flatten_marshal_parcel(ast: &DeriveInput) -> TokenStream {
    let name = &ast.ident;
    let flatten_into = get_flatten_into_ident(&ast.attrs);
    let flatten_into_args = quote::format_ident!("{}Args", flatten_into);
    let fields_named = match &ast.data {
        Data::Struct(s) => match &s.fields {
            Fields::Named(f) => f,
            _ => {
                return Error::new(
                    name.span(),
                    "FlattenParcel can only be derived for structs with named fields.",
                )
                .to_compile_error();
            }
        },
        _ => {
            return Error::new(name.span(), "FlattenParcel can only be derived for structs.")
                .to_compile_error();
        }
    };

    let assignments = fields_named.named.iter().map(|field| {
        let field_name = field.ident.as_ref().expect("Field should have an identifier");
        gen_field_assignment(
            &quote! { args },
            field,
            &quote! { self.#field_name },
            &quote! { builder },
        )
    });

    quote! {
        impl #name {
            fn flatten<'a>(
                    &self,
                    args: &mut inner::#flatten_into_args<'a>,
                    builder: &mut flatbuffers::FlatBufferBuilder<'a>) {
                #(#assignments);*;
            }
        }
    }
}

// Stores a value to the field #field_name of the given #args, depending on #[marshal(..)].
fn gen_field_assignment(
    args: &TokenStream,
    field: &Field,
    field_access: &TokenStream,
    builder: &TokenStream,
) -> TokenStream {
    let field_name = field.ident.as_ref().expect("Field should have an identifier");
    match MarshalAttr::new(field) {
        Some(MarshalAttr::Default(lit)) => {
            quote! { #args.#field_name = #field_access.unwrap_or(#lit) }
        }
        Some(MarshalAttr::Packed) => {
            quote! { #args.#field_name = Some(#field_access.to_packed(#builder)) }
        }
        Some(MarshalAttr::Table) => {
            quote! { #args.#field_name = Some(#field_access.marshal(#builder)) }
        }
        Some(MarshalAttr::Union) => {
            let type_field_name = quote::format_ident!("{}_type", field_name);
            quote! {
                #args.#type_field_name = #field_access.inner_type();
                #args.#field_name = Some(#field_access.marshal(#builder))
            }
        }
        Some(MarshalAttr::Flatten) => {
            quote! { #field_access.flatten(&mut #args, #builder) }
        }
        Some(MarshalAttr::Map(map)) => {
            quote! { #args.#field_name = #map(&#field_access, #builder) }
        }
        None => quote! { #args.#field_name = *#field_access },
    }
}

// Parses #[marshal(..)] attributes.
enum MarshalAttr {
    // #[marshal(default = Expr)]
    // Expects an Option value to be passed, and sets the default value when serializing.
    Default(Box<Expr>),
    // #[flatten]
    // Calls the flatten() defined in gen_flatten_marshal_parcel() when serializing.
    // This attribute is also used in UnmarshalParcel.
    Flatten,
    // #[marshal(packed)]
    // Expects a value with a type that implements ToPacked trait, and calls to_packed() when
    // serializing.
    Packed,
    // #[table]
    // Indicates that the field is an inner table, to be marshaled using the field's `MarshalParcel`
    // implementation. This attribute is also used in UnmarshalParcel.
    Table,
    // #[union]
    // Treat the flatbuffer's type as union, which requires another field `<field_name>_type`,
    // indicating the variant. This attribute is also used in UnmarshalParcel.
    Union,
    // #[marshal(map = <fn>)]
    // Maps a value when serializing based on the given function.
    Map(Ident),
}

impl MarshalAttr {
    fn new(field: &Field) -> Option<Self> {
        field.attrs.iter().find_map(Self::from_attr)
    }

    fn from_attr(attr: &Attribute) -> Option<Self> {
        if attr.path().is_ident("flatten") {
            return Some(Self::Flatten);
        }
        if attr.path().is_ident("union") {
            return Some(Self::Union);
        }
        if attr.path().is_ident("table") {
            return Some(Self::Table);
        }

        if !attr.path().is_ident("marshal") {
            return None;
        }

        let mut ret = None::<Self>;

        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("packed") {
                ret = Some(Self::Packed);
                Ok(())
            } else if meta.path.is_ident("default") {
                let lit = meta.value()?.parse::<Expr>()?;
                ret = Some(Self::Default(Box::new(lit)));
                Ok(())
            } else if meta.path.is_ident("map") {
                let map = meta.value()?.parse::<Ident>()?;
                ret = Some(Self::Map(map));
                Ok(())
            } else {
                Err(meta.error("unsupported marshal attribute"))
            }
        })
        .expect("error parsing marshal attribute");

        ret
    }
}
