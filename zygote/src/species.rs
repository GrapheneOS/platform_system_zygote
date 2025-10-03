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

use crate::config;
use zygote_messages::{SpawnParamsCommon, SpawnPayload, SpawnPayloadParser};

#[cfg(all(target_os = "android", feature = "android-native"))]
pub mod android_native;
#[cfg(feature = "libapp")]
pub mod lib_app;
#[cfg(feature = "mock")]
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

/// All configured species.
const SPECIES_LIST: &[SpeciesRef] = &[
    #[cfg(all(target_os = "android", feature = "android-native"))]
    &android_native::App,
    #[cfg(feature = "libapp")]
    &lib_app::App,
    #[cfg(feature = "mock")]
    &mock::Turtle,
];

/// Re-initialization data that is gathered and then consumed by species code
pub enum ReInitWrapper {
    /// Android specific data
    #[cfg(all(target_os = "android", feature = "android-native"))]
    AndroidNative(android_native::ReInitData),
    /// LibApp specific data
    #[cfg(feature = "libapp")]
    LibApp,
    /// Mock specific data
    #[cfg(feature = "mock")]
    Mock,
}

impl ReInitWrapper {
    /// Retrieve a reference to this enum's [`android_native::ReInitData`]
    /// struct
    #[cfg(all(target_os = "android", feature = "android-native"))]
    #[allow(unreachable_patterns)]
    pub fn as_android_native(&self) -> anyhow::Result<&android_native::ReInitData> {
        match self {
            ReInitWrapper::AndroidNative(data) => Ok(data),
            _ => Err(anyhow::anyhow!("Invalid re-initialization data type")),
        }
    }
}

/// A collection of callbacks implemented by Zygote payloads that determine
/// runtime behaviors such as preloading, process creation, and transfer
/// of control flow.
pub trait Species {
    /// Determines the socket to listen on from command line options
    fn resolve_socket(&self, config: &config::Server) -> Option<String> {
        config.socket.to_owned()
    }
    /// Returns true if an abstract socket name is allowed to be registered
    fn abstract_socket_is_allowed(&self, name: &str) -> bool;
    /// Returns true if a bound socket path is allowed to be registered
    fn bound_socket_is_allowed(&self, name: &str) -> bool;
    /// Gather data that will later be used to re-initialize the child process
    fn gather_reinitialization_data(&self) -> ReInitWrapper;
    /// Return true if the provided payload is associated with this species
    fn is_spawn_payload_type(&self, message: &SpawnPayload) -> bool;
    /// Returns the name of the species
    fn name(&self) -> &'static str;
    /// Returns true if the file is allowed to be registered
    fn file_is_allowed(&self, path: &CStr) -> bool;
    /// Take over control flow for the new process
    fn gestate(&self, spawn_params: &SpawnParamsCommon, spawn_payload: &SpawnPayload) -> !;
    /// Returns the default action for a given file path
    fn get_file_action(&self, path: &CStr) -> Option<crate::file_descriptors::Action>;
    /// Child-process re-initialization logic that runs before the rest of the
    /// code in [`child_process::re_initialize`]
    fn re_initialize_epilogue(
        &self,
        spawn_params: &SpawnParamsCommon,
        spawn_payload: &SpawnPayload,
        re_init_data: &ReInitWrapper,
    );
    /// Child-process re-initialization logic that runs before the rest of the
    /// code in [`child_process::re_initialize`]
    fn re_initialize_prologue(
        &self,
        spawn_params: &SpawnParamsCommon,
        spawn_payload: &SpawnPayload,
        re_init_data: &ReInitWrapper,
    );
    /// A callback for setting SecComp filters
    fn set_seccomp_filters(&self, spawn_params: &SpawnParamsCommon);

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

        Err(format!("No species defined with name '{s}'"))
    }
}

impl From<&SpawnPayloadParser> for SpeciesRef {
    fn from(value: &SpawnPayloadParser) -> Self {
        match value {
            #[cfg(all(target_os = "android", feature = "android-native"))]
            SpawnPayloadParser::AndroidNative { .. } => &android_native::App,
            #[cfg(feature = "libapp")]
            SpawnPayloadParser::LibApp { .. } => &lib_app::App,
            #[cfg(any(test, feature = "mock"))]
            SpawnPayloadParser::Mock { .. } => &mock::Turtle,
        }
    }
}

/// A helper trait for converting types into species references.
pub trait ToSpecies {
    /// Use the value to fetch a species reference.
    fn to_species(self) -> SpeciesRef;
}

impl<T> ToSpecies for T
where
    SpeciesRef: From<T>,
{
    fn to_species(self) -> SpeciesRef {
        SpeciesRef::from(self)
    }
}
