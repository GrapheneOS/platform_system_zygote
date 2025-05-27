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

use crate::{
    file_descriptors::Action,
    messages::{self, Message},
    species::{file_entry, socket_entry, FileAllowListEntry, SocketAllowListEntry, Species},
};

#[rustfmt::skip]
static ALLOWED_FILE_PATHS: [FileAllowListEntry; 1] = [
    file_entry(crate::test::MOCK_FILE_PATH_1, "testname", "2025-02-19"),
];

#[rustfmt::skip]
static ALLOWED_SOCKET_NAMES: [SocketAllowListEntry; 1] = [
    socket_entry(crate::test::SOCKET_NAME_1, "testname", "2025-02-19"),
];

#[rustfmt::skip]
static ALLOWED_SOCKET_PATHS: [SocketAllowListEntry; 1] = [
    socket_entry(crate::test::SOCKET_PATH_1, "testname", "2025-02-19"),
];

/// Behaviors for testing the Zygote process server.
///
/// See: https://en.wikipedia.org/wiki/Mock_Turtle
pub struct Turtle;

impl Species for Turtle {
    fn abstract_socket_is_allowed(&self, name: &str) -> bool {
        ALLOWED_SOCKET_NAMES.iter().any(|entry| entry.data == name)
    }

    fn bound_socket_is_allowed(&self, path: &str) -> bool {
        ALLOWED_SOCKET_PATHS.iter().any(|entry| entry.data == path)
    }

    fn message_type_spawn(&self) -> Message {
        Message::SpawnMock
    }

    fn name(&self) -> &'static str {
        "mock"
    }

    fn file_is_allowed(&self, path_str: &CStr) -> bool {
        ALLOWED_FILE_PATHS.iter().any(|entry| entry.data == path_str)
    }

    fn gestate(&self, spawn_message: messages::SpawnMessage) -> ! {
        let parcel = flatbuffers::root::<messages::Parcel>(spawn_message.as_ref()).unwrap();
        let spawn_cmd = parcel.message_as_spawn_mock().unwrap();
        println!("Hello from the child process.  My name is {}", spawn_cmd.name());
        std::process::exit(0)
    }

    fn get_file_action(&self, _path: &CStr) -> Option<Action> {
        None
    }
}
