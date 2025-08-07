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
use native_activity_thread::run_native_activity_thread;

use crate::{
    file_descriptors::Action,
    introspection::debug_assert_single_threaded,
    messages::{self, SpawnParamsCommon, SpawnPayload},
    species::Species,
};
use zygote_sys as sys;

const AID_APP_START: i32 = 10000;

// Must be the same value as `SdkVersion::kUnset` in art/libartbase/base/sdk_version.h.
const SDK_VERSION_UNSET: i32 = 0;

/// Re-initialization data for AndroidNative applications
pub struct ReInitData {
    fds_error_level: sys::android::FDSanErrorLevel,
}

/// Behaviors for launching native Android applications.
pub struct App;

impl Species for App {
    fn abstract_socket_is_allowed(&self, _name: &str) -> bool {
        false
    }

    fn bound_socket_is_allowed(&self, _path: &str) -> bool {
        false
    }

    fn gather_reinitialization_data(&self) -> super::ReInitWrapper {
        debug_assert_single_threaded();

        super::ReInitWrapper::AndroidNative(ReInitData {
            // SAFETY: This is called in a single-threaded context
            fds_error_level: unsafe { sys::android::fdsan_get_error_level() },
        })
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
        if let SpawnPayload::AndroidNative { package, start_seq, target_sdk_version } =
            spawn_payload
        {
            // TODO: Handle process dumpability
            // TODO: Enable debugging
            // TODO: Set heap tagging level
            // TODO: Disable heap zero-initialization

            let target =
                if *target_sdk_version <= 0 { SDK_VERSION_UNSET } else { *target_sdk_version };
            sys::android::set_application_target_sdk_version(target);

            println!("Hello from the child process.  My name is {package}");

            run_native_activity_thread(*start_seq);
        } else {
            panic!("Invalid spawn payload for species {}: {:?}", self.name(), spawn_payload);
        }
    }

    fn get_file_action(&self, _path: &CStr) -> Option<Action> {
        None
    }

    fn re_initialize_prologue(&self, re_init_data: super::ReInitWrapper) {
        debug_assert_single_threaded();

        // SAFETY: This is called in a single-threaded context
        unsafe {
            sys::android::fdsan_set_error_level(
                re_init_data.as_android_native().unwrap().fds_error_level,
            );
        }

        if sys::android::set_zygote_child().is_err() {
            log::error!("Failed to android_mallopt(M_SET_ZYGOTE_CHILD)");
        }

        if let Err(errno) = sys::mallopt(libc::M_DECAY_TIME, 1) {
            log::error!("Failed to mallopt(M_DECAY_TIME): {errno}");
        }
    }

    fn set_seccomp_filters(&self, spawn_params: &SpawnParamsCommon) {
        if spawn_params.uid.expect("No UID specified") >= AID_APP_START {
            sys::android::set_app_seccomp_filter();
        } else {
            sys::android::set_system_seccomp_filter();
        }
    }
}
