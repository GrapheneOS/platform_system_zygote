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

//! Implementation of the Species trait for LibApp.  This species will locate,
//! load, and enter a shared library that correctly define the `zygote_entry`
//! function.

use core::ffi::CStr;

use crate::{
    file_descriptors::Action,
    messages::{Command, Message},
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

    fn command_type_spawn(&self) -> Command {
        Command::SpawnLibApp
    }

    fn name(&self) -> &'static str {
        "lib-app"
    }

    fn file_is_allowed(&self, _path: &CStr) -> bool {
        false
    }

    fn gestate(&self, message_buffer: crate::server::MessageBuffer) -> ! {
        let message = flatbuffers::root::<Message>(&message_buffer).unwrap();
        let spawn_cmd = message.command_as_spawn_lib_app().unwrap();
        println!("Path to library application: {}", spawn_cmd.path());
        std::process::exit(0)
    }

    fn get_file_action(&self, _path: &CStr) -> Option<Action> {
        None
    }
}
