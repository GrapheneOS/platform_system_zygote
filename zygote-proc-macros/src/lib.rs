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

//! The crate providing the proc-macro for parcels.

use quote::quote;
use syn::{parse_macro_input, DeriveInput};

mod marshal;
mod unmarshal;
mod utils;

use marshal::{gen_flatten_marshal_parcel, gen_marshal_parcel};
use unmarshal::{gen_flatten_unmarshal_parcel, gen_unmarshal_parcel};

/// A derive macro for implementing the Marshalable trait.
#[proc_macro_derive(MarshalParcel, attributes(flatten, marshal, inner_type_name, union, table))]
pub fn marshal_parcel_derive(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let ast = parse_macro_input!(input as DeriveInput);
    gen_marshal_parcel(&ast).into()
}

/// A derive macro for flattening a parcel.
#[proc_macro_derive(FlattenParcel, attributes(marshal, unmarshal, flatten_into_type))]
pub fn flatten_marshal_parcel_derive(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let ast = parse_macro_input!(input as DeriveInput);
    let marshal = gen_flatten_marshal_parcel(&ast);
    let unmarshal = gen_flatten_unmarshal_parcel(&ast);
    quote! {
        #marshal
        #unmarshal
    }
    .into()
}

/// A derive macro for implementing the UnmarshalParcel trait.
#[proc_macro_derive(
    UnmarshalParcel,
    attributes(flatten, unmarshal, unmarshal_from, inner_type_name, union, table)
)]
pub fn unmarshal_parcel_derive(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let ast = parse_macro_input!(input as DeriveInput);
    gen_unmarshal_parcel(&ast).into()
}
