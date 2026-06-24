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

use core::ffi::{c_int, CStr};
use std::os::unix::net::UnixDatagram;

use itertools::Itertools;

use native_activity_thread::{
    app_process_init, get_or_init_debuggable, preload_lib, run_native_activity_thread,
};
use processgroup::{
    processgroup::{cgroup, drop_task_profiles_resource_caching},
    sched::{cpusets_enabled, SchedPolicy},
};
use rustutils::android;

use crate::{
    file_descriptors::Action as FDAction,
    species::{AllowListEntry, Species, SpeciesTag},
};
use zygote_messages::{self as messages, SpawnParamsCommon, SpawnPayload};
use zygote_sys::{self as sys, procfs::debug_assert_single_threaded, AsCStr};

const AID_APP_START: i32 = 10000;

static ALLOWED_SOCKET_PATHS: &[AllowListEntry<str>] = &[
    // logger FDs will be closed in `sync_fd_state`
    AllowListEntry::new("/dev/socket/logdw", FDAction::Ignore, "Logging", "hattorij", "2025-11-12"),
    AllowListEntry::new(
        UNSOLICITED_ZYGOTE_SOCKET_PATH,
        FDAction::Close,
        "Report child process exit status to AMS",
        "chibar",
        "2025-12-03",
    ),
    AllowListEntry::new(
        "/dev/socket/statsdw",
        FDAction::Ignore,
        "Log statsd Atoms",
        "hattorij",
        "2026-01-13",
    ),
];

static ALLOWED_FILE_PATHS: &[AllowListEntry<CStr>] = &[
    // logger FDs will be closed in `sync_fd_state`
    AllowListEntry::new(c"/dev/pmsg0", FDAction::Ignore, "Logging", "hattorij", "2025-11-12"),
    AllowListEntry::new(
        c"/sys/kernel/debug/tracing/trace_marker",
        FDAction::Ignore,
        "Tracing",
        "chriswailes",
        "2025-12-16",
    ),
    AllowListEntry::new(
        c"/sys/kernel/tracing/trace_marker",
        FDAction::Ignore,
        "Tracing",
        "chriswailes",
        "2025-12-16",
    ),
];

/// Path to the socket listened by the AMS to receive the exit status of child
/// processes.
///
/// Must be the same path as that in `kSystemServerSockAddr` in
/// core/jni/com_android_internal_os_Zygote.cpp.
const UNSOLICITED_ZYGOTE_SOCKET_PATH: &str = "/data/system/unsolzygotesocket";

// Must be the same as `UnsolicitedZygoteMessageTypes` in core/jni/com_android_internal_os_Zygote.cpp
#[repr(u32)]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
enum UnsolicitedZygoteMessageTypes {
    #[allow(dead_code)]
    Reserved = 0,
    SigChld = 1,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
struct UnsolicitedZygoteMessageHeader {
    pub typ: UnsolicitedZygoteMessageTypes,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
struct UnsolicitedZygoteMessagePayload {
    pub pid: libc::pid_t,
    pub uid: libc::uid_t,
    pub status: c_int,
}

/// A struct defining the data format used to notify the exit status of a child process.
/// Must be the same as `UnsolicitedZygoteMessageSigChld` in core/jni/com_android_internal_os_Zygote.cpp
#[repr(C)]
#[derive(Debug, Copy, Clone)]
struct UnsolicitedZygoteMessageSigChld {
    header: UnsolicitedZygoteMessageHeader,
    payload: UnsolicitedZygoteMessagePayload,
}

impl UnsolicitedZygoteMessageSigChld {
    fn as_bytes(&self) -> &[u8] {
        // SAFETY: Passing a pointer to a non-null consective memory region of size
        // `std::mem::size_of::<UnsolicitedZygoteMessageSigChld>()`.
        unsafe {
            std::slice::from_raw_parts(
                (self as *const UnsolicitedZygoteMessageSigChld) as *const u8,
                std::mem::size_of::<UnsolicitedZygoteMessageSigChld>(),
            )
        }
    }
}

/// Re-initialization data for AndroidNative applications
#[derive(Debug)]
pub struct ReInitData {
    fds_error_level: android::process::FDSanErrorLevel,
}

/// Behaviors for launching native Android applications.
#[derive(Debug)]
pub struct App;

impl Species for App {
    fn bound_abstract_socket_is_allowed(&self, _name: &str) -> bool {
        false
    }

    fn bound_socket_path_is_allowed(&self, _path: &str) -> bool {
        false
    }

    fn peer_socket_path_is_allowed(&self, path: &str) -> bool {
        ALLOWED_SOCKET_PATHS.iter().contains(path)
    }

    fn gather_reinitialization_data(&self) -> super::ReInitWrapper {
        debug_assert_single_threaded();

        super::ReInitWrapper::AndroidNative(ReInitData {
            // SAFETY: This is called in a single-threaded context
            fds_error_level: unsafe { android::process::fdsan_get_error_level() },
        })
    }

    fn get_socket_env_var_prefix(&self) -> &'static str {
        "ANDROID_SOCKET_"
    }

    fn is_spawn_payload_type(&self, message: &messages::SpawnPayload) -> bool {
        matches!(
            message,
            SpawnPayload::AndroidNative { .. } | SpawnPayload::AndroidNativeSubspecies { .. }
        )
    }

    fn file_is_allowed(&self, path: &CStr) -> bool {
        ALLOWED_FILE_PATHS.iter().contains(path)
    }

    fn sync_fd_state(&self) {
        // We need to call `__android_log_close` to close the logger FDs to sync the internal state
        // of LogdSocket.
        // c.f. https://cs.android.com/android/platform/superproject/main/+/main:system/logging/liblog/logd_writer.cpp;l=57
        sys::android::log_close();
    }

    fn gestate(&self, _spawn_params: &SpawnParamsCommon, spawn_payload: &SpawnPayload) -> ! {
        if let SpawnPayload::AndroidNative {
            start_seq, target_sdk_version, runtime_flags, ..
        } = spawn_payload
        {
            let scope_init =
                tracing::span!(tracing::Level::TRACE, "AndroidNative::App::gestate").entered();
            app_process_init(*target_sdk_version, *runtime_flags);
            let _scope_init = scope_init.exit();

            run_native_activity_thread(*start_seq);
        } else {
            panic!("Invalid spawn payload for species {}: {:?}", self.name(), spawn_payload);
        }
    }

    #[tracing::instrument(skip_all)]
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

    fn get_peer_socket_action(&self, path: &str) -> Option<FDAction> {
        ALLOWED_SOCKET_PATHS.iter().find(|entry| entry.data == path).map(|entry| entry.action)
    }

    fn get_file_action(&self, path: &CStr) -> Option<FDAction> {
        ALLOWED_FILE_PATHS.iter().find(|entry| entry.data == path).map(|entry| entry.action)
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
            se_info_buffer.as_cstr().expect("se_info_buffer should be null-terminated"),
            process_name_buffer.as_cstr().expect("process_name_buffer should be null-terminated"),
            spawn_params.selinux_flags.expect("no selinux_flags"),
        )
        .expect("Unable to transition SE Linux contexts");
    }

    fn re_initialize_prologue(
        &self,
        spawn_params: &SpawnParamsCommon,
        spawn_payload: &SpawnPayload,
        re_init_data: &super::ReInitWrapper,
    ) {
        debug_assert_single_threaded();

        // SAFETY: This is called in a single-threaded context
        unsafe {
            android::process::fdsan_set_error_level(
                re_init_data
                    .as_android_native()
                    .expect("Expected AndroidNative ReInitData")
                    .fds_error_level,
            );
        }

        if android::process::set_zygote_child().is_err() {
            log::error!("Failed to android_mallopt(M_SET_ZYGOTE_CHILD)");
        }

        if let Err(errno) = sys::mallopt(libc::M_DECAY_TIME, 1) {
            log::error!("Failed to mallopt(M_DECAY_TIME): {errno}");
        }

        // Create a cgroup for the process
        if sys::getuid() == 0 {
            cgroup::create(
                spawn_params.uid.expect("No UID specified").try_into().expect("Invalid UID"),
                sys::getpid(),
            )
            .expect("Unable to create cgroup for process");
        }

        let policy = if let SpawnPayload::AndroidNative { top_app, .. } = spawn_payload {
            if *top_app {
                SchedPolicy::TopApp
            } else {
                SchedPolicy::Default
            }
        } else {
            SchedPolicy::Default
        };

        // Set the cpuset policy and panic on failure
        if cpusets_enabled() {
            sys::android::set_cpuset_policy(0, policy).expect("Failed to set cpuset policy");
        }

        // Set the scheduling policy and panic on failure.  Must be called
        // before losing the permission to set scheduler policy.
        sys::android::set_sched_policy(0, policy).expect("Failed to set scheduler policy");

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

    fn on_start(&self) {
        let atom = statslog_native_zygote::native_zygote_started::NativeZygoteStarted {};
        if let Err(err) = atom.stats_write() {
            log::error!("Error logging the NativeZygoteStarted Atom: {err}");
        }

        // Read the `ro.debuggable` property and cache it while the process is still allowed to read
        // the value.
        get_or_init_debuggable();
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

    fn handle_sigchld(&self, pid: libc::pid_t, uid: libc::uid_t, status: c_int) {
        if let Ok(sock) = UnixDatagram::unbound()
            .and_then(|sock| sock.connect(UNSOLICITED_ZYGOTE_SOCKET_PATH).map(|_| sock))
        {
            let data = UnsolicitedZygoteMessageSigChld {
                header: UnsolicitedZygoteMessageHeader {
                    typ: UnsolicitedZygoteMessageTypes::SigChld,
                },
                payload: UnsolicitedZygoteMessagePayload { pid, uid, status },
            };
            log::debug!("about to send sigchld status to the unsolicited socket {:?}", data);
            if let Err(e) = sock.send(data.as_bytes()) {
                log::error!("Failed to send sigchld status to the unsolicited socket: {e}");
            }
        }
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

        file_path_res && socket_path_res
    }
}
