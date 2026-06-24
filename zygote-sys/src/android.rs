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

//! This module provides safe wrappers around Android-specific functionality

use core::ffi::{c_char, CStr};

use processgroup::sched;

use crate::{check_failure_with_void, Result};

/// A wrapper around [`inner::__android_log_close`]
pub fn log_close() {
    inner::__android_log_close();
}

/// A wrapper around [`inner::android_set_application_target_sdk_version`]
pub fn set_application_target_sdk_version(target: i32) {
    inner::android_set_application_target_sdk_version(target);
}

/// A wrapper function for [`processgroup::sched::set_cpuset_policy`] that
/// wraps the returned value in a Result.
pub fn set_cpuset_policy(tid: libc::pid_t, policy: sched::SchedPolicy) -> Result<()> {
    check_failure_with_void(sched::set_cpuset_policy(tid, policy))
}

/// A wrapper function for [`processgroup::sched::set_sched_policy`] that wraps
/// the returned value in a Result.
pub fn set_sched_policy(tid: libc::pid_t, policy: sched::SchedPolicy) -> Result<()> {
    check_failure_with_void(sched::set_sched_policy(tid, policy))
}

/// An alias around [`inner::setprogname`]
///
/// # Safety
/// The caller must ensure that `name` points to a valid C-style NULL
/// terminated string.
pub unsafe fn set_program_name(name: *const c_char) {
    // SAFETY: The pointer argument is guaranteed valid by the caller.
    unsafe { inner::setprogname(name) };
}

/// A wrapper around Android's SELinux context switching mechanism.
pub fn set_selinux_context(
    uid: libc::uid_t,
    is_system_server: bool,
    se_info: &CStr,
    name: &CStr,
    selinux_flags: u64,
) -> Result<()> {
    // SAFETY: Both `seinfo` and `name` are valid, null-terminated, C-strings
    check_failure_with_void(unsafe {
        selinux_bindgen::selinux_android_setcontext2(
            uid,
            is_system_server,
            se_info.as_ptr(),
            name.as_ptr(),
            selinux_flags,
        )
    })
}

mod inner {
    use core::ffi::{c_char, c_int};

    unsafe extern "C" {
        /// Close the file descriptors used for logging, i.e. the /dev/pmsg0 file and the socket connected to logd
        ///
        /// See: https://cs.android.com/android/platform/superproject/main/+/main:system/logging/liblog/include/log/log.h;l=147
        pub safe fn __android_log_close();

        /// Set the target SDK version for the app.
        ///
        /// See: https://cs.android.com/android/platform/superproject/main/+/main:bionic/libdl/libdl_android.cpp;l=77
        pub safe fn android_set_application_target_sdk_version(target: c_int);

        /// Set the process name.
        ///
        /// # Safety
        /// `progname` must be a pointer to a valid C string which lives while it's set as the process name.
        ///
        /// See https://cs.android.com/android/platform/superproject/main/+/main:bionic/libc/upstream-openbsd/lib/libc/gen/setprogname.c;l=22
        pub fn setprogname(progname: *const c_char);
    }
}
