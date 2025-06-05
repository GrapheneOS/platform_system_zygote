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
    messages::{self, SpawnPayload},
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

    fn spawn_payload_type(&self) -> SpawnPayload {
        SpawnPayload::SpawnAndroidNative
    }

    fn name(&self) -> &'static str {
        "android-native-app"
    }

    fn file_is_allowed(&self, _path: &CStr) -> bool {
        false
    }

    fn gestate(&self, spawn_message: messages::SpawnMessage, _priority_final: Option<i32>) -> ! {
        let parcel = flatbuffers::root::<messages::Parcel>(spawn_message.as_ref()).unwrap();
        let spawn_cmd = parcel.message_as_spawn().unwrap();
        let spawn_payload = spawn_cmd.payload_as_spawn_android_native().unwrap();
        println!("Hello from the child process.  My name is {}", spawn_payload.package());
        std::process::exit(0)
    }

    fn get_file_action(&self, _path: &CStr) -> Option<Action> {
        None
    }
}
