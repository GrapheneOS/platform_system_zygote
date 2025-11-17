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
use native_activity_thread::{app_process_init, preload_lib, run_native_activity_thread};
use rustutils::android;

use processgroup::{
    processgroup::drop_task_profiles_resource_caching,
    sched::{cpusets_enabled, SchedPolicy},
};

use crate::{
    file_descriptors::Action,
    introspection::debug_assert_single_threaded,
    species::{Species, SpeciesTag},
};
use zygote_messages::{self as messages, SpawnParamsCommon, SpawnPayload};
use zygote_sys::{self as sys, AsCStr};

const AID_APP_START: i32 = 10000;

/// Re-initialization data for AndroidNative applications
pub struct ReInitData {
    fds_error_level: android::process::FDSanErrorLevel,
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

        // TODO: Add TopApp information
        super::ReInitWrapper::AndroidNative(ReInitData {
            // SAFETY: This is called in a single-threaded context
            fds_error_level: unsafe { android::process::fdsan_get_error_level() },
        })
    }

    fn is_spawn_payload_type(&self, message: &messages::SpawnPayload) -> bool {
        matches!(
            message,
            SpawnPayload::AndroidNative { .. } | SpawnPayload::AndroidNativeSubspecies { .. }
        )
    }

    fn file_is_allowed(&self, _path: &CStr) -> bool {
        false
    }

    fn gestate(&self, _spawn_params: &SpawnParamsCommon, spawn_payload: &SpawnPayload) -> ! {
        if let SpawnPayload::AndroidNative {
            package,
            start_seq,
            target_sdk_version,
            runtime_flags,
        } = spawn_payload
        {
            app_process_init(*target_sdk_version, *runtime_flags);
            println!("Hello from the child process.  My name is {package}");
            run_native_activity_thread(*start_seq);
        } else {
            panic!("Invalid spawn payload for species {}: {:?}", self.name(), spawn_payload);
        }
    }

    fn speciate(&self, payload: &SpawnPayload) {
        let SpawnPayload::AndroidNativeSubspecies {
            target_sdk_version,
            runtime_flags,
            library_path,
            library_dirs,
            permitted_library_paths,
            shared,
            zip_path,
            native_shared_lib_path,
            preload_func,
            uid_gid_min,
            uid_gid_max,
        } = payload
        else {
            panic!("Invalid spawn payload for species {}: {:?}", self.name(), payload);
        };
        // We install this seccomp filter here and not in `set_seccomp_filters()` since a
        // `setresuid(2)` call follows the `set_seccomp_filters()` call, and the specified UID is
        // not in [uid_gid_min, uid_gid_max].
        android::process::install_setuidgid_seccomp_filter(*uid_gid_min, *uid_gid_max);
        app_process_init(*target_sdk_version, *runtime_flags);
        // SAFETY: We trust that library name and the namespace parameters are valid, and
        //         the `preload_func` has the correct signature (takes no arguments, returns
        //         nothing).
        unsafe {
            preload_lib(
                library_path,
                library_dirs,
                permitted_library_paths,
                *target_sdk_version,
                *shared,
                zip_path,
                native_shared_lib_path,
                *preload_func,
            );
        };
    }

    fn get_file_action(&self, _path: &CStr) -> Option<Action> {
        None
    }

    fn re_initialize_epilogue(
        &self,
        spawn_params: &SpawnParamsCommon,
        _re_init_data: &super::ReInitWrapper,
    ) {
        let uid = spawn_params.uid.expect("No UID specified");
        let se_info = spawn_params.se_info.as_ref().expect("No SE Linux info specified");

        let mut se_info_buffer = sys::BUFFER_INIT_CSTRING;
        se_info_buffer[0..se_info.len()].copy_from_slice(se_info.as_bytes());

        let process_name = spawn_params.process_name.as_ref().expect("No process name specified");
        let mut process_name_buffer = sys::BUFFER_INIT_CSTRING;
        process_name_buffer[0..process_name.len()].copy_from_slice(process_name.as_bytes());

        sys::android::set_selinux_context(
            uid as libc::uid_t,
            false,
            se_info_buffer.as_cstr().unwrap(),
            process_name_buffer.as_cstr().unwrap(),
        )
        .expect("Unable to transition SE Linux contexts");
    }

    fn re_initialize_prologue(
        &self,
        _spawn_params: &SpawnParamsCommon,
        re_init_data: &super::ReInitWrapper,
    ) {
        debug_assert_single_threaded();

        // SAFETY: This is called in a single-threaded context
        unsafe {
            android::process::fdsan_set_error_level(
                re_init_data.as_android_native().unwrap().fds_error_level,
            );
        }

        if android::process::set_zygote_child().is_err() {
            log::error!("Failed to android_mallopt(M_SET_ZYGOTE_CHILD)");
        }

        if let Err(errno) = sys::mallopt(libc::M_DECAY_TIME, 1) {
            log::error!("Failed to mallopt(M_DECAY_TIME): {errno}");
        }

        // Set the cpuset policy and panic on failure
        if cpusets_enabled() {
            sys::android::set_cpuset_policy(0, SchedPolicy::Default).unwrap();
        }

        // Set the scheduling policy and panic on failure.  Must be called
        // before losing the permission to set scheduler policy.
        sys::android::set_sched_policy(0, SchedPolicy::Default).unwrap();

        // We are going to lose the permission to set scheduler policy during
        // the specialization, so make sure that we don't cache the fd of
        // cgroup path that may cause sepolicy violation by writing value to
        // the cached fd directly when creating new thread.
        drop_task_profiles_resource_caching();
    }

    fn set_seccomp_filters(&self, spawn_params: &SpawnParamsCommon, spawn_payload: &SpawnPayload) {
        if matches!(spawn_payload, SpawnPayload::AndroidNativeSubspecies { .. }) {
            android::process::set_app_zygote_seccomp_filter();
            sys::prctl_set_no_new_privs().expect("Failed to set NO_NEW_PRIVS");
        } else if spawn_params.uid.expect("No UID specified") >= AID_APP_START {
            android::process::set_app_seccomp_filter();
        } else {
            android::process::set_system_seccomp_filter();
        }
    }

    fn tag(&self) -> SpeciesTag {
        SpeciesTag::AndroidNative
    }

    fn on_server_ready(&self) {
        if let Err(e) = android::system_properties::write("zygote.zygote_next.server_ready", "true")
        {
            log::error!("Failed to set zygote.zygote_next.server_ready: {e}");
        }
    }

    fn on_server_destroy(&self) {
        if let Err(e) =
            android::system_properties::write("zygote.zygote_next.server_ready", "false")
        {
            log::error!("Failed to set zygote.zygote_next.server_ready: {e}");
        }
    }
}
