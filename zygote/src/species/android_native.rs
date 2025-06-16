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

//! Implementation of the Species trait for Android Native Applications.

use core::ffi::CStr;

use crate::{
    file_descriptors::Action,
    messages::{self, SpawnParamsCommon, SpawnPayload},
    species::Species,
};

/// Behaviors for launching native Android applications.
pub struct App;

impl Species for App {
    fn abstract_socket_is_allowed(&self, _name: &str) -> bool {
        false
    }

    fn bound_socket_is_allowed(&self, _path: &str) -> bool {
        false
    }

    fn is_spawn_payload_type(&self, message: &messages::SpawnPayload) -> bool {
        matches!(message, SpawnPayload::AndroidNative { .. })
    }

    fn name(&self) -> &'static str {
        "android-native-app"
    }

    fn file_is_allowed(&self, _path: &CStr) -> bool {
        false
    }

    fn gestate(&self, _spawn_params: &SpawnParamsCommon, spawn_payload: &SpawnPayload) -> ! {
        if let SpawnPayload::AndroidNative { package } = spawn_payload {
            // TODO: Handle process dumpability
            // TODO: Enable debugging
            // TODO: Set heap tagging level
            // TODO: Disable heap zero-initialization

            println!("Hello from the child process.  My name is {}", package);
            std::process::exit(0)
        } else {
            panic!("Invalid spawn payload for species {}: {:?}", self.name(), spawn_payload);
        }
    }

    fn get_file_action(&self, _path: &CStr) -> Option<Action> {
        None
    }
}
