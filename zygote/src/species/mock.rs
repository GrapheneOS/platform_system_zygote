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

//! A mock species implementation used for testing.

use core::ffi::CStr;

use itertools::Itertools;

use crate::{
    file_descriptors::Action as FDAction,
    species::{AllowListEntry, Species, SpeciesTag},
};
use zygote_messages::{self as messages, SpawnParamsCommon, SpawnPayload};

#[rustfmt::skip]
static ALLOWED_FILE_PATHS: &[AllowListEntry<CStr>] = &[
    AllowListEntry::new(crate::test::MOCK_FILE_PATH_1, FDAction::Ignore, "Test the allowed-paths functionality", "testname", "2026-02-19"),
];

#[rustfmt::skip]
static ALLOWED_SOCKET_NAMES: &[AllowListEntry<str>] = &[
    AllowListEntry::new(crate::test::SOCKET_NAME_1, FDAction::Ignore, "Test the allowed-socket-names functionality","testname", "2026-02-19"),
];

#[rustfmt::skip]
static ALLOWED_SOCKET_PATHS: &[AllowListEntry<str>] = &[
    AllowListEntry::new(crate::test::SOCKET_PATH_1, FDAction::Ignore, "Test the allowed-socket-paths functionality","testname", "2026-02-19"),
];

/// Behaviors for testing the Zygote process server.
///
/// See: https://en.wikipedia.org/wiki/Mock_Turtle
#[derive(Debug)]
pub struct Turtle;

impl Species for Turtle {
    fn bound_abstract_socket_is_allowed(&self, name: &str) -> bool {
        ALLOWED_SOCKET_NAMES.iter().contains(name)
    }

    fn bound_socket_path_is_allowed(&self, path: &str) -> bool {
        ALLOWED_SOCKET_PATHS.iter().contains(path)
    }

    fn file_is_allowed(&self, path: &CStr) -> bool {
        ALLOWED_FILE_PATHS.iter().contains(path)
    }

    fn gather_reinitialization_data(&self) -> super::ReInitWrapper {
        super::ReInitWrapper::Mock
    }

    fn get_file_action(&self, path: &CStr) -> Option<FDAction> {
        ALLOWED_FILE_PATHS.iter().find(|entry| entry.data == path).map(|entry| entry.action)
    }

    fn sync_fd_state(&self) {
        // Nothing to do here
    }

    fn gestate(&self, _spawn_params: &SpawnParamsCommon, spawn_payload: &SpawnPayload) -> ! {
        if let SpawnPayload::Mock { name } = spawn_payload {
            println!("Hello from the child process.  My name is {name}");
            std::process::exit(0)
        } else {
            panic!("Invalid spawn payload for species {}: {:?}", self.name(), spawn_payload);
        }
    }

    fn is_spawn_payload_type(&self, message: &messages::SpawnPayload) -> bool {
        matches!(message, SpawnPayload::Mock { .. })
    }

    fn peer_socket_path_is_allowed(&self, path: &str) -> bool {
        ALLOWED_SOCKET_PATHS.iter().any(|entry| entry.data == path)
    }

    fn get_peer_socket_action(&self, path: &str) -> Option<FDAction> {
        ALLOWED_SOCKET_PATHS.iter().find(|entry| entry.data == path).map(|entry| entry.action)
    }

    fn re_initialize_epilogue(
        &self,
        _spawn_params: &SpawnParamsCommon,
        _re_init_data: &super::ReInitWrapper,
    ) {
        // Nothing to do here
    }

    fn re_initialize_prologue(
        &self,
        _spawn_params: &SpawnParamsCommon,
        _spawn_payload: &SpawnPayload,
        _re_init_data: &super::ReInitWrapper,
    ) {
        // Nothing to do here
    }

    fn speciate(&self, _payload: &SpawnPayload) {
        // Nothing to do here
    }

    fn set_seccomp_filters(&self, _spawn_params: &SpawnParamsCommon, _payload: &SpawnPayload) {
        // Nothing to do here
    }

    fn tag(&self) -> SpeciesTag {
        SpeciesTag::Mock
    }

    //
    // Helper functions
    //

    #[cfg(any(test, feature = "test"))]
    fn allowlists_are_fresh(&self) -> bool {
        use crate::species::test::allowlist_entries_are_fresh;

        // Capture results separately to avoid short-circuiting.  We want to
        // print out all expired entries.
        let file_path_res = allowlist_entries_are_fresh(ALLOWED_FILE_PATHS.iter());
        let socket_path_res = allowlist_entries_are_fresh(ALLOWED_SOCKET_PATHS.iter());
        let socket_name_res = allowlist_entries_are_fresh(ALLOWED_SOCKET_NAMES.iter());

        file_path_res && socket_name_res && socket_path_res
    }
}
