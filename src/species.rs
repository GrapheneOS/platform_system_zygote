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

use core::ffi::CStr;
use std::str::FromStr;

pub mod android_native;
#[cfg(any(test, feature = "test"))]
pub mod mock;

#[cfg(any(test, feature = "test"))]
pub(crate) struct FileAllowListEntry {
    data: &'static CStr,

    // TODO: Ensure that this data is erased from the final binary
    _reviewer: &'static str,
    _reviewed: &'static str,
}

#[cfg(any(test, feature = "test"))]
pub(crate) const fn file_entry(
    data: &'static CStr,
    _reviewer: &'static str,
    _reviewed: &'static str,
) -> FileAllowListEntry {
    FileAllowListEntry { data, _reviewer, _reviewed }
}

#[cfg(any(test, feature = "test"))]
pub(crate) struct SocketAllowListEntry {
    data: &'static str,

    // TODO: Ensure that this data is erased from the final binary
    _reviewer: &'static str,
    _reviewed: &'static str,
}

#[cfg(any(test, feature = "test"))]
pub(crate) const fn socket_entry(
    data: &'static str,
    _reviewer: &'static str,
    _reviewed: &'static str,
) -> SocketAllowListEntry {
    SocketAllowListEntry { data, _reviewer, _reviewed }
}

pub type SpeciesRef = &'static (dyn Species + Sync);

#[cfg(not(any(test, feature = "test")))]
const SPECIES_LIST: &[SpeciesRef] = &[&android_native::App];

#[cfg(any(test, feature = "test"))]
const SPECIES_LIST: &[SpeciesRef] = &[&android_native::App, &mock::Mock];

pub trait Species {
    fn abstract_socket_is_allowed(&self, name: &str) -> bool;
    fn bound_socket_is_allowed(&self, name: &str) -> bool;
    fn name(&self) -> &'static str;
    fn file_is_allowed(&self, path: &CStr) -> bool;
    fn get_file_action(&self, path: &CStr) -> Option<crate::file_descriptors::Action>;

    // Helper functions

    fn path_is_absolute(&self, path_str: &str) -> bool {
        path_str.starts_with("/") && !path_str.contains("/../")
    }
}

impl FromStr for SpeciesRef {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        for species in SPECIES_LIST {
            if species.name() == s {
                return Ok(*species);
            }
        }

        Err(format!("No species defined with name '{}'", s))
    }
}
