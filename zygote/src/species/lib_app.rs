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

use libloading::os::unix::{Library, Symbol, RTLD_GLOBAL, RTLD_NOW};
use log::{error, info, warn};

use crate::{
    file_descriptors::Action,
    messages::{self, Message},
    species::Species,
    sys,
};

/// Name of the entry symbol for LibApps
const LIBAPP_ENTRY_SYMBOL_NAME: &CStr = c"zygote_entry";

/// Behaviors for launching native Android applications.
pub struct App;

impl Species for App {
    fn abstract_socket_is_allowed(&self, _name: &str) -> bool {
        false
    }

    fn bound_socket_is_allowed(&self, _path: &str) -> bool {
        false
    }

    fn message_type_spawn(&self) -> Message {
        Message::SpawnLibApp
    }

    fn name(&self) -> &'static str {
        "lib-app"
    }

    fn file_is_allowed(&self, _path: &CStr) -> bool {
        false
    }

    fn gestate(&self, spawn_message: messages::SpawnMessage, priority_final: Option<i32>) -> ! {
        let parcel = flatbuffers::root::<messages::Parcel>(spawn_message.as_ref()).unwrap();
        let spawn_cmd = parcel.message_as_spawn_lib_app().unwrap();

        let library_path_str = spawn_cmd.path();
        let library_path = std::path::Path::new(library_path_str);

        let library_args = spawn_cmd.args().iter().map(|arg| arg.to_owned()).collect();

        if !library_path.exists() {
            error!("No library found at the specified path: {}", spawn_cmd.path());
            std::process::exit(1);
        }

        info!("Path to library application: {}", spawn_cmd.path());

        // SAFETY: The library path is obtained from the SpawnMessage which
        //         originates from a trusted source.  The path has been
        //         verified to exist and the result of the `dlopen()` call is
        //         checked.
        let library = unsafe { Library::open(Some(library_path), RTLD_NOW | RTLD_GLOBAL) }
            .unwrap_or_else(|err| {
                error!("Failure to load shared library ({:?}): {}", library_path, err);
                std::process::exit(1);
            });

        info!("Successfully loaded shared library");
        let entry_function: Symbol<unsafe fn(Vec<String>) -> i32> =
            // SAFETY: The symbol name is part of the API for LibApps.
            unsafe { library.get(LIBAPP_ENTRY_SYMBOL_NAME.to_bytes()) }.unwrap_or_else(|err| {
                error!("Symbol `zygote_entry` not found in shared library: {}", err);
                std::process::exit(1);
            });

        if let Some(priority) = priority_final {
            if sys::setpriority(libc::PRIO_PROCESS, 0, priority).is_err() {
                // EINVAL, EPERM, and ESRCH only apply when setting the
                // priority of other processes.
                warn!("Insufficient permissions to set priority: {}", priority);
            }
        }

        // SAFETY: The function signature is part of the API for LibApps.  An
        //         improper signature will result in undefined behavior.  From
        //         this point forward the library may execute arbitrary code.
        unsafe {
            std::process::exit(entry_function(library_args));
        }
    }

    fn get_file_action(&self, _path: &CStr) -> Option<Action> {
        None
    }
}
