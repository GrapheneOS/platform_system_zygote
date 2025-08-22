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

pub(crate) const CAP_DATA_EMPTY: [__user_cap_data_struct; 2] =
    [__user_cap_data_struct { effective: 0, permitted: 0, inheritable: 0 }; 2];

pub(crate) const LINUX_CAPABILITIES_VERSION_1: u32 = 0x19980330;
pub(crate) const LINUX_CAPABILITIES_VERSION_2: u32 = 0x20071026;
pub(crate) const LINUX_CAPABILITIES_VERSION_3: u32 = 0x20080522;

/// Capabilities header as defined in `linux/capabilities.h`
///
/// See: `man capabilities`
#[repr(C)]
#[allow(non_camel_case_types)]
pub(crate) struct __user_cap_header_struct {
    pub version: u32,
    pub pid: c_int,
}

/// Opaque wrapper type for [`__user_cap_header_struct`]
#[allow(non_camel_case_types)]
pub(crate) type cap_user_header_t = *mut __user_cap_header_struct;

/// Capabilities user data as defined in `linux/capabilities.h`
///
/// See: `man capabilities`
#[repr(C)]
#[derive(Default, Copy, Clone)]
#[allow(non_camel_case_types)]
pub(crate) struct __user_cap_data_struct {
    pub effective: u32,
    pub permitted: u32,
    pub inheritable: u32,
}

/// Opaque wrapper type for [`__user_cap_data_struct`]
#[allow(non_camel_case_types)]
pub(crate) type cap_user_data_t = *mut __user_cap_data_struct;

/// Opaque wrapper type for constant [`__user_cap_data_struct`]
#[allow(non_camel_case_types)]
pub(crate) type cap_user_data_const_t = *const __user_cap_data_struct;

/// Value for representing capabilities as defined in
/// `libcap/include/sys/capabilities.h`
#[allow(non_camel_case_types)]
pub(crate) type cap_value_t = c_int;

unsafe extern "C" {
    pub(crate) fn cap_drop_bound(cap: cap_value_t) -> c_int;
    pub(crate) fn cap_get_bound(cap: cap_value_t) -> c_int;
    pub(crate) fn cap_max_bits() -> cap_value_t;

    pub(crate) fn capget(header: cap_user_header_t, data: cap_user_data_t) -> c_int;
    pub(crate) fn capset(header: cap_user_header_t, data: cap_user_data_const_t) -> c_int;
}
