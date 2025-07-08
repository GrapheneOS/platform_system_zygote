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

//! This module provides extern definitions for `libc` functions that are not
//! defined by the `libc` crate for some platforms.

#[allow(unused_imports)]
use core::ffi::{c_char, c_int, c_uint, c_void};

#[cfg(target_os = "android")]
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

    /// This variant of `dup` is used by
    /// [`file_descriptor::FileDescriptorEntry::execute`] to ensure that the
    /// correct flags (e.g. libc::O_CLOEXEC) are set during the duplication
    /// operation.
    ///
    /// See `man dup`
    pub fn dup3(oldfd: c_int, newfd: c_int, flags: c_int) -> c_int;
}

#[cfg(target_os = "android")]
unsafe extern "system" {
    /// Apply Android's application seccomp filters
    #[link_name = "_Z22set_app_seccomp_filterv"]
    pub safe fn set_app_seccomp_filter();

    /// Apply Android's system seccomp filters
    #[link_name = "_Z25set_system_seccomp_filterv"]
    pub safe fn set_system_seccomp_filter();
}

#[cfg(not(target_env = "musl"))]
unsafe extern "C" {
    /// Get a pointer to an immutable string containing the name of the error
    /// code (e.g. "EPERM").  Returns `NULL` on an invalid input.
    ///
    /// # Safety
    /// This function is thread safe and returns a pointer to constant data.
    #[allow(dead_code)]
    pub fn strerrorname_np(errno: c_int) -> *const c_char;

    /// Get a pointer to an immutable string containing a description of the
    /// error code.  Returns `NULL` on an invalid input.
    ///
    /// # Safety
    /// This function is thread safe and returns a pointer to constant data.
    #[allow(dead_code)]
    pub fn strerrordesc_np(errno: c_int) -> *const c_char;
}
