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

use syn::{Attribute, Expr, Ident, Lit, Meta, Variant};

pub(crate) fn get_inner_type_ident(attrs: &[Attribute]) -> Option<Ident> {
    get_attribute_ident(attrs, "inner_type_name")
}

pub(crate) fn get_inner_type_ident_from_variant(variant: &Variant) -> Ident {
    get_attribute_ident(&variant.attrs, "inner_type_name").unwrap_or_else(|| variant.ident.clone())
}

pub(crate) fn get_flatten_into_ident(attrs: &[Attribute]) -> Ident {
    get_attribute_ident(attrs, "flatten_into_type").expect("should have a flatten_into attribute")
}

fn get_attribute_ident(attrs: &[Attribute], attr_name: &str) -> Option<Ident> {
    attrs.iter().find_map(|attr| match &attr.meta {
        Meta::NameValue(name_value) if name_value.path.is_ident(attr_name) => {
            let lit_str = match &name_value.value {
                Expr::Lit(lit) => match &lit.lit {
                    Lit::Str(s) => s,
                    _ => panic!("{attr_name} must be a literal string"),
                },
                _ => panic!("{attr_name} must be a literal string"),
            };
            Some(Ident::new(&lit_str.value(), lit_str.span()))
        }
        _ => None,
    })
}
