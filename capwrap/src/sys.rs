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

//! External function declarations for symbols exported by libcap

use core::ffi::c_int;

/// Capabilities header as defined in `linux/capabilities.h`
///
/// See: `man capabilities`
#[repr(C)]
#[allow(non_camel_case_types)]
pub struct cap_user_header_t {
    version: u32,
    pid: c_int,
}

/// Value for representing capabilities as defined in
/// `libcap/include/sys/capabilities.h`
#[allow(non_camel_case_types)]
pub type cap_value_t = c_int;

#[link(name = "cap", kind = "dylib")]
#[allow(dead_code)]
extern "C" {
    pub(crate) fn cap_max_bits() -> cap_value_t;
}
