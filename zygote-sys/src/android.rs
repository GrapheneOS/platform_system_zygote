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

use core::ffi::CStr;

use processgroup::{self, SchedPolicy};

use crate::{libc_result_from_int_with_void, LibcResult};

/// A wrapper around [`inner::android_set_application_target_sdk_version`]
pub fn set_application_target_sdk_version(target: i32) {
    inner::android_set_application_target_sdk_version(target);
}

/// A wrapper function for [`processgropu::set_cpuset_policy`] that wraps the returned
/// value in a LibcResult.
pub fn set_cpuset_policy(tid: libc::pid_t, policy: SchedPolicy) -> LibcResult<()> {
    libc_result_from_int_with_void(processgroup::set_cpuset_policy(tid, policy))
}

/// A wrapper function for [`processgropu::set_sched_policy`] that wraps the returned
/// value in a LibcResult.
pub fn set_sched_policy(tid: libc::pid_t, policy: SchedPolicy) -> LibcResult<()> {
    libc_result_from_int_with_void(processgroup::set_sched_policy(tid, policy))
}

/// A wrapper around Android's SELinux context switching mechanism.
pub fn set_selinux_context(
    uid: libc::uid_t,
    is_system_server: bool,
    se_info: &CStr,
    name: &CStr,
) -> LibcResult<()> {
    // SAFETY: Both `seinfo` and `name` are valid, null-terminated, C-strings
    libc_result_from_int_with_void(unsafe {
        selinux_bindgen::selinux_android_setcontext(
            uid,
            is_system_server,
            se_info.as_ptr(),
            name.as_ptr(),
        )
    })
}

mod inner {
    use core::ffi::c_int;

    unsafe extern "C" {
        /// Set the target SDK version for the app.
        ///
        /// See: https://cs.android.com/android/platform/superproject/main/+/main:bionic/libdl/libdl_android.cpp;l=77
        pub safe fn android_set_application_target_sdk_version(target: c_int);
    }
}
