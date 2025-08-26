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

use bitflags::bitflags;
use core::ffi::{c_int, CStr};
use native_activity_thread::run_native_activity_thread;
use std::env;

use crate::{
    config,
    file_descriptors::Action,
    introspection::debug_assert_single_threaded,
    messages::{self, SpawnParamsCommon, SpawnPayload},
    species::Species,
};
use zygote_sys::{self as sys, AsCStr};

const ANDROID_SOCKET_ENV_PREFIX: &str = "ANDROID_SOCKET_";
const ANDROID_SOCKET_DIR: &str = "/dev/socket";

const AID_APP_START: i32 = 10000;

// Must be the same value as `SdkVersion::kUnset` in art/libartbase/base/sdk_version.h.
const SDK_VERSION_UNSET: i32 = 0;

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct RuntimeFlags: u32 {
        /// Runtime flag constants.
        /// Must be the same values as RuntimeFlags in frameworks/base/core/jni/com_android_internal_os_Zygote.cpp.
        const DEBUG_ENABLE_JDWP = 1;
        const PROFILE_SYSTEM_SERVER = 1 << 14;
        const PROFILE_FROM_SHELL = 1 << 15;
        const MEMORY_TAG_LEVEL_MASK = (1 << 19) | (1 << 20);
        const MEMORY_TAG_LEVEL_TBI = 1 << 19;
        const MEMORY_TAG_LEVEL_ASYNC = 2 << 19;
        const MEMORY_TAG_LEVEL_SYNC = 3 << 19;
        const GWP_ASAN_LEVEL_MASK = (1 << 21) | (1 << 22);
        const GWP_ASAN_LEVEL_NEVER = 0 << 21;
        const GWP_ASAN_LEVEL_LOTTERY = 1 << 21;
        const GWP_ASAN_LEVEL_ALWAYS = 2 << 21;
        const GWP_ASAN_LEVEL_DEFAULT = 3 << 21;
        const NATIVE_HEAP_ZERO_INIT_ENABLED = 1 << 23;
        const PROFILEABLE = 1 << 24;
        const DEBUG_ENABLE_PTRACE = 1 << 25;
        const ENABLE_PAGE_SIZE_APP_COMPAT = 1 << 26;
    }
}

impl RuntimeFlags {
    fn get_heap_tagging_level(&self) -> c_int {
        match self.intersection(Self::MEMORY_TAG_LEVEL_MASK) {
            Self::MEMORY_TAG_LEVEL_TBI => libc::M_HEAP_TAGGING_LEVEL_TBI,
            Self::MEMORY_TAG_LEVEL_ASYNC => libc::M_HEAP_TAGGING_LEVEL_ASYNC,
            Self::MEMORY_TAG_LEVEL_SYNC => libc::M_HEAP_TAGGING_LEVEL_SYNC,
            _ => libc::M_HEAP_TAGGING_LEVEL_NONE,
        }
    }

    fn is_native_heap_zero_init_enabled(&self) -> bool {
        self.contains(Self::NATIVE_HEAP_ZERO_INIT_ENABLED)
    }
}

/// Re-initialization data for AndroidNative applications
pub struct ReInitData {
    fds_error_level: sys::android::FDSanErrorLevel,
}

/// Behaviors for launching native Android applications.
pub struct App;

impl Species for App {
    fn resolve_socket(&self, config: &config::Server) -> Option<String> {
        if let Some(socket_from_config) = config.socket.as_ref() {
            Some(socket_from_config.clone())
        } else if let Ok(socket_from_env) =
            env::var(format!("{}{}", ANDROID_SOCKET_ENV_PREFIX, config.name))
        {
            Some(socket_from_env)
        } else {
            Some(format!("{}/{}", ANDROID_SOCKET_DIR, config.name))
        }
    }

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
        if let SpawnPayload::AndroidNative {
            package,
            se_info: _,
            start_seq,
            target_sdk_version,
            runtime_flags,
        } = spawn_payload
        {
            // TODO: Handle process dumpability
            // TODO: Enable debugging

            match RuntimeFlags::from_bits(*runtime_flags) {
                Some(flags) => {
                    if let Err(errno) = sys::mallopt(
                        libc::M_BIONIC_SET_HEAP_TAGGING_LEVEL,
                        flags.get_heap_tagging_level(),
                    ) {
                        log::warn!("Failed to mallopt(M_BIONIC_SET_HEAP_TAGGING_LEVEL): {}", errno);
                    }

                    if !flags.is_native_heap_zero_init_enabled() {
                        if let Err(errno) = sys::mallopt(libc::M_BIONIC_ZERO_INIT, 0) {
                            log::warn!("Failed to mallopt(M_BIONIC_ZERO_INIT): {}", errno);
                        }
                    }
                }
                None => {
                    log::warn!(
                        "runtime_flags doesn't have a valid representation: {}",
                        *runtime_flags
                    );
                }
            }

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

    fn re_initialize_epilogue(
        &self,
        spawn_params: &SpawnParamsCommon,
        spawn_payload: &SpawnPayload,
        _re_init_data: &super::ReInitWrapper,
    ) {
        let uid = spawn_params.uid.expect("No UID specified");
        let se_info = if let SpawnPayload::AndroidNative { se_info, .. } = spawn_payload {
            se_info
        } else {
            panic!("No SE Linux info specified");
        };

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
        _spawn_payload: &SpawnPayload,
        re_init_data: &super::ReInitWrapper,
    ) {
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

        // Set the cpuset policy and panic on failure
        if sys::android::cpusets_enabled() {
            sys::android::set_cpuset_policy(0, sys::android::SchedPolicy::Default).unwrap();
        }

        // Set the scheduling policy and panic on failure.  Must be called
        // before losing the permission to set scheduler policy.
        sys::android::set_sched_policy(0, sys::android::SchedPolicy::Default).unwrap();

        // We are going to lose the permission to set scheduler policy during
        // the specialization, so make sure that we don't cache the fd of
        // cgroup path that may cause sepolicy violation by writing value to
        // the cached fd directly when creating new thread.
        sys::android::drop_task_profiles_resource_caching();
    }

    fn set_seccomp_filters(&self, spawn_params: &SpawnParamsCommon) {
        if spawn_params.uid.expect("No UID specified") >= AID_APP_START {
            sys::android::set_app_seccomp_filter();
        } else {
            sys::android::set_system_seccomp_filter();
        }
    }
}
