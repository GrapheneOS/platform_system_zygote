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

use core::ffi::{c_uint, CStr};

use anyhow::{anyhow, Result};
use processgroup::{self, SchedPolicy};

pub use inner::{set_app_seccomp_filter, set_system_seccomp_filter};

use crate::{libc_result_from_int_with_void, LibcResult};

/// Error levels for Android's File Descriptor Sanitizer
///
/// See: https://android.googlesource.com/platform/bionic/+/master/docs/fdsan.md
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub enum FDSanErrorLevel {
    /// No errors
    Disabled = 0,
    /// Warn once(ish) on error, and then downgrade to [`Disabled`]
    WarnOnce,
    /// Warn always on error
    WarnAlways,
    /// Abort on error
    Fatal,
}

fn fdsan_error_level(level: c_uint) -> FDSanErrorLevel {
    match level {
        0 => FDSanErrorLevel::Disabled,
        1 => FDSanErrorLevel::WarnOnce,
        2 => FDSanErrorLevel::WarnAlways,
        3 => FDSanErrorLevel::Fatal,
        _ => panic!("Invalid result returned from libc"),
    }
}

/// Operations supported by [`mallopt`]
///
/// See: https://cs.android.com/android/platform/superproject/main/+/main:bionic/libc/platform/bionic/malloc.h;l=54
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub enum MalloptOpcode {
    /// Marks the calling process as a profileable zygote child, possibly
    /// initializing profiling infrastructure.
    InitZygoteChildProfiling = 1,
    /// Reset malloc hooks
    ResetHooks = 2,
    /// Set an upper bound on the total size in bytes of all allocations
    /// made using the memory allocation APIs.
    ///   arg = size_t*
    ///   arg_size = sizeof(size_t)
    SetAllocationLimitBytes = 3,
    /// Called after the zygote forks to indicate this is a child.
    SetZygoteChild = 4,
    /// Options to dump backtraces of allocations. These options only
    /// work when malloc debug has been enabled.
    ///
    /// Writes the backtrace information of all current allocations to a file.
    /// NOTE: arg_size has to be sizeof(FILE*) because FILE is an opaque type.
    ///   arg = FILE*
    ///   arg_size = sizeof(FILE*)
    WriteMallocLeekInfoToFile = 5,
    /// Get information about the backtraces of all
    ///   arg = android_mallopt_leak_info_t*
    ///   arg_size = sizeof(android_mallopt_leak_info_t)
    GetMallocLeakInfo = 6,
    /// Free the memory allocated and returned by M_GET_MALLOC_LEAK_INFO.
    ///   arg = android_mallopt_leak_info_t*
    ///   arg_size = sizeof(android_mallopt_leak_info_t)
    FreeMallocLeakInfo = 7,
    /// Query whether the current process is considered to be profileable by
    /// the Android platform. Result is assigned to the arg pointer's
    /// destination.
    ///   arg = bool*
    ///   arg_size = sizeof(bool)
    GetProcessProfileable = 9,
    /// Maybe enable GWP-ASan. Set *arg to force GWP-ASan to be turned on,
    /// otherwise this mallopt() will internally decide whether to sample
    /// the process. The program must be single threaded at the point when
    /// the android_mallopt function is called.
    ///   arg = android_mallopt_gwp_asan_options_t*
    ///   arg_size = sizeof(android_mallopt_gwp_asan_options_t)
    InitializeGwpAsan = 10,
    /// Query whether memtag stack is enabled for this process.
    MemtagStackIsOn = 11,
    /// Query whether the current process has the decay time enabled so
    /// that the memory from allocations are not immediately released to the
    /// OS. Result is assigned to the arg pointer's destination.
    ///   arg = bool*
    ///   arg_size = sizeof(bool)
    GetDecayTimeEnabled = 12,
}

/// A wrapper around [`inner::android_fdsan_get_error_level`]
///
/// # Safety
/// This function is not thread safe.
///
/// See: https://android.googlesource.com/platform/bionic/+/master/docs/fdsan.md
pub unsafe fn fdsan_get_error_level() -> FDSanErrorLevel {
    // SAFETY: This function takes not arguments and always succeeds.
    fdsan_error_level(unsafe { inner::android_fdsan_get_error_level() })
}

/// A wrapper around [`inner::android_fdsan_get_error_level`]
///
/// # Safety
/// This function is not thread safe.
///
/// See: https://android.googlesource.com/platform/bionic/+/master/docs/fdsan.md
pub unsafe fn fdsan_set_error_level(level: FDSanErrorLevel) -> FDSanErrorLevel {
    // SAFETY: This function takes an integer argument and always succeeds.
    fdsan_error_level(unsafe { inner::android_fdsan_set_error_level(level as c_uint) })
}

/// A safe wrapper around [`libc_fill::android_mallopt`] and the
/// M_SET_ZYGOTE_CHILD opcode
pub fn set_zygote_child() -> Result<()> {
    // SAFETY: This opcode takes no arguments so a nullptr is passed
    //         instead.
    unsafe { inner::android_mallopt(MalloptOpcode::SetZygoteChild as _, std::ptr::null_mut(), 0) }
        .then_some(())
        .ok_or_else(|| anyhow!("Call to android_mallopt failed: Opcode = M_SET_ZYGOTE_CHILD"))
}

/// Reset the thread local stack protection salt.
///
/// The caller should not return after calling this function.  If it does,
/// and stack protection is enabled, the program will crash.
///
/// TODO: Make this function take a `noreturn` thunk.
#[inline(always)]
pub fn reset_stack_guards() {
    inner::android_reset_stack_guards();
}

/// A wrapper around [`libc_fill::android_set_application_target_sdk_version`]
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
    use core::ffi::{c_int, c_uint, c_void};

    unsafe extern "C" {
        /// Return the current process's FDSan error level
        ///
        /// # Safety
        /// This function is not thread safe.
        ///
        /// See: https://android.googlesource.com/platform/bionic/+/master/docs/fdsan.md
        pub fn android_fdsan_get_error_level() -> c_uint;

        /// Sets the process's FDSan error level and returns the previous value
        ///
        /// # Safety
        /// This function is not thread safe.
        ///
        /// See: https://android.googlesource.com/platform/bionic/+/master/docs/fdsan.md
        pub fn android_fdsan_set_error_level(level: c_uint) -> c_uint;

        /// Set Android-specific allocation options.
        ///
        /// See: https://cs.android.com/android/platform/superproject/main/+/main:bionic/libc/bionic/android_mallopt.cpp
        pub fn android_mallopt(opcode: c_int, arg: *mut c_void, arg_size: usize) -> bool;

        /// Reset the thread's stack protection salt
        ///
        /// The caller should not return after calling this function.  If it does,
        /// and stack protection is enabled, the program will crash.
        ///
        /// See: https://cs.android.com/android/platform/superproject/main/+/main:bionic/libc/bionic/__libc_init_main_thread.cpp;l=104
        pub safe fn android_reset_stack_guards();

        /// Set the target SDK version for the app.
        ///
        /// See: https://cs.android.com/android/platform/superproject/main/+/main:bionic/libdl/libdl_android.cpp;l=77
        pub safe fn android_set_application_target_sdk_version(target: c_int);

        /// Apply Android's application seccomp filters
        #[link_name = "_Z22set_app_seccomp_filterv"]
        pub safe fn set_app_seccomp_filter();

        /// Apply Android's system seccomp filters
        #[link_name = "_Z25set_system_seccomp_filterv"]
        pub safe fn set_system_seccomp_filter();
    }
}
