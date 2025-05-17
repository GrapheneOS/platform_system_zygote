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

//! "Species" are an abstraction over the preloaded resources, process
//! creation details, and control flow transfer mechanism utilized by different
//! clients of the Zygote process server architecture.

use core::ffi::CStr;
use std::str::FromStr;

pub mod android_native;
pub mod lib_app;
#[cfg(any(test, feature = "test"))]
pub mod mock;

/// An entry structure for file allow lists.  This is marked as test-only
/// because the only current user is the mock testing class.
#[cfg(any(test, feature = "test"))]
pub(crate) struct FileAllowListEntry {
    data: &'static CStr,

    // TODO: Ensure that this data is erased from the final binary
    _reviewer: &'static str,
    _reviewed: &'static str,
}

/// A constructor for [`FileAllowListEntry`] structs
#[cfg(any(test, feature = "test"))]
pub(crate) const fn file_entry(
    data: &'static CStr,
    _reviewer: &'static str,
    _reviewed: &'static str,
) -> FileAllowListEntry {
    FileAllowListEntry { data, _reviewer, _reviewed }
}

/// An entry structure for socket allow lists.  This is marked as test-only
/// because the only current user is the mock testing class.
#[cfg(any(test, feature = "test"))]
pub(crate) struct SocketAllowListEntry {
    data: &'static str,

    // TODO: Ensure that this data is erased from the final binary
    _reviewer: &'static str,
    _reviewed: &'static str,
}

/// A constructor for [`SocketAllowListEntry`] structs.
#[cfg(any(test, feature = "test"))]
pub(crate) const fn socket_entry(
    data: &'static str,
    _reviewer: &'static str,
    _reviewed: &'static str,
) -> SocketAllowListEntry {
    SocketAllowListEntry { data, _reviewer, _reviewed }
}

/// A reference type for a statically allocated Species VTable.
pub type SpeciesRef = &'static (dyn Species + Sync);

/// All production species.
#[cfg(not(any(test, feature = "test")))]
const SPECIES_LIST: &[SpeciesRef] = &[&android_native::App, &lib_app::App];

/// All production and test species.
#[cfg(any(test, feature = "test"))]
const SPECIES_LIST: &[SpeciesRef] = &[&android_native::App, &lib_app::App, &mock::Turtle];

/// A collection of callbacks implemented by Zygote payloads that determine
/// runtime behaviors such as preloading, process creation, and transfer
/// of control flow.
pub trait Species {
    /// Returns true if an abstract socket name is allowed to be registered
    fn abstract_socket_is_allowed(&self, name: &str) -> bool;
    /// Returns true if a bound socket path is allowed to be registered
    fn bound_socket_is_allowed(&self, name: &str) -> bool;
    /// Return the [`crate::messages::Command`] tag corresponding to this
    /// species' Spawn command
    fn command_type_spawn(&self) -> crate::messages::Command;
    /// Returns the name of the species
    fn name(&self) -> &'static str;
    /// Returns true if the file is allowed to be registered
    fn file_is_allowed(&self, path: &CStr) -> bool;
    /// Take over control flow for the new process
    fn gestate(&self, message_buffer: crate::server::MessageBuffer) -> !;
    /// Returns the default action for a given file path
    fn get_file_action(&self, path: &CStr) -> Option<crate::file_descriptors::Action>;

    // Helper functions

    /// A simple test to check of a provided path string is absolute or not.
    fn path_is_absolute(&self, path_str: &str) -> bool {
        path_str.starts_with("/") && !path_str.contains("/../")
    }
}

impl FromStr for SpeciesRef {
    type Err = String;

    /// Iterates through [`SPECIES_LIST`] to find a reference to a species with
    /// the provided name.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        for species in SPECIES_LIST {
            if species.name() == s {
                return Ok(*species);
            }
        }

        Err(format!("No species defined with name '{}'", s))
    }
}
