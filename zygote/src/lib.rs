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

//! Implementation of a fully native Zygote architecture.
//!
//! This library contains the logic used by the Zygote server and executables.

#[cfg(not(any(
    all(feature = "android-native", target_os = "android"),
    feature = "libapp",
    feature = "mock"
)))]
compile_error!("At least one species feature must be enabled");

use core::ffi::{c_char, c_int};

pub(crate) mod arguments;
pub mod child_process;
pub mod config;
pub mod file_descriptors;
pub mod introspection;
pub mod server;
pub mod species;
#[cfg(any(test, feature = "test"))]
pub mod test;

#[unsafe(link_section = ".init_array")]
#[used]
static CONSTRUCTOR_PTR: unsafe extern "C" fn(c_int, *mut *mut c_char, *mut *mut c_char) =
    arguments::args_initializer;
