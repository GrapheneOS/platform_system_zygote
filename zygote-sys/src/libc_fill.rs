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
use core::ffi::{c_char, c_int, c_long, c_uint, c_void};
use std::os::fd::RawFd;

use libc::{pid_t, syscall};

/// Arguments for calls to [`clone3`]
///
/// See: `man clone`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[allow(non_camel_case_types)]
#[repr(C)]
pub struct clone_args {
    flags: u64,
    pidfd: u64,
    child_tid: u64,
    parent_tid: u64,
    exit_signal: u64,
    stack: u64,
    stack_size: u64,
    tls: u64,
    set_tid: u64,
    set_tid_size: u64,
    cgroup: u64,
}

impl Default for clone_args {
    /// Returns a set of arguments for `clone3` that emulates a call to `fork`
    fn default() -> Self {
        Self {
            flags: 0,
            pidfd: std::ptr::null_mut::<c_void>() as _,
            child_tid: std::ptr::null_mut::<c_void>() as _,
            parent_tid: std::ptr::null_mut::<c_void>() as _,
            exit_signal: libc::SIGCHLD as _,
            stack: std::ptr::null_mut::<c_void>() as _,
            stack_size: 0,
            tls: 0,
            set_tid: std::ptr::null_mut::<c_void>() as _,
            set_tid_size: 0,
            cgroup: 0,
        }
    }
}
#[allow(dead_code)]
impl clone_args {
    /// Create an empty set of clone arguments
    pub fn new() -> Self {
        Self::default()
    }

    fn with_flags(mut self, flags: u64) -> Self {
        self.flags |= flags;
        self
    }

    fn with_pidfd(mut self, pidfd: &mut c_int) -> Self {
        self.pidfd = pidfd as *mut _ as u64;
        self.flags |= libc::CLONE_PIDFD as u64;
        self
    }

    fn with_child_tid(mut self, child_tid: &mut pid_t) -> Self {
        self.child_tid = child_tid as *mut _ as u64;
        self.flags |= libc::CLONE_CHILD_SETTID as u64;
        self
    }

    fn with_parent_tid(mut self, parent_tid: &mut pid_t) -> Self {
        self.parent_tid = parent_tid as *mut _ as u64;
        self.flags |= libc::CLONE_PARENT_SETTID as u64;
        self
    }

    fn with_exit_signal(mut self, exit_signal: c_int) -> Self {
        self.exit_signal = exit_signal as u64;
        self
    }

    fn with_stack(mut self, stack: &mut [u8]) -> Result<Self, String> {
        self.stack = stack.iter_mut().last().ok_or("Stack is empty")? as *mut _ as u64;
        self.stack_size = stack.len() as u64;
        Ok(self)
    }

    /// Pass `clone3` the [`libc::CLONE_SETTLS`] flag and an appropriate argument.
    ///
    /// # Safety
    /// The TLS argument is architecture dependent:
    /// On x86, `tls` is interpreted as a `struct user_desc *` (see
    /// set_thread_area(2)).  On x86-64 it is the new value to be set for the
    /// `%fs` base register (see the ARCH_SET_FS argument to arch_prctl(2)).
    /// On architectures with a dedicated TLS register, it is the new value of
    /// that register.
    ///
    /// See: `man clone3`
    unsafe fn with_tls(mut self, tls: u64) -> Self {
        self.tls = tls;
        self.flags |= libc::CLONE_SETTLS as u64;
        self
    }

    fn with_set_tid(mut self, set_tid: &mut [pid_t]) -> Self {
        self.set_tid = set_tid.as_ptr() as u64;
        self.set_tid_size = set_tid.len() as u64;
        self
    }

    // The type of [`libc::CLONE_INTO_CGROUP`] is platform dependent and
    // sometimes requires a cast.
    #[allow(clippy::unnecessary_cast)]
    fn with_cgroup(mut self, cgroup_fd: RawFd) -> Self {
        self.cgroup = cgroup_fd as u64;
        self.flags |= libc::CLONE_INTO_CGROUP as u64;
        self
    }
}

/// A wrapper around the `clone3` system call.
///
/// # Safety
/// Safe usage of this function is very complicated and has many conditions.
/// Refer to the manual page for a detailed description of all arguments.  The
/// default value for the [`clone_args`] struct will emulate a call to `fork`.
///
/// See: `man clone3`
pub unsafe fn clone3(args: &clone_args) -> c_long {
    // SAFETY: The pointer argument to this system call is taken from a valid
    //         reference and the size is computed directly by the compiler.
    unsafe {
        syscall(
            libc::SYS_clone3,
            args as *const _ as u64,
            std::mem::size_of::<clone_args>() as libc::size_t,
        )
    }
}

#[cfg(target_os = "android")]
unsafe extern "C" {
    /// This variant of `dup` is used by
    /// [`file_descriptor::FileDescriptorEntry::execute`] to ensure that the
    /// correct flags (e.g. libc::O_CLOEXEC) are set during the duplication
    /// operation.
    ///
    /// See `man dup`
    pub fn dup3(oldfd: c_int, newfd: c_int, flags: c_int) -> c_int;

    /// Set the process name.
    ///
    /// # Safety
    /// `progname` must be a pointer to a valid C string which lives while it's set as the process name.
    ///
    /// See https://cs.android.com/android/platform/superproject/main/+/main:bionic/libc/upstream-openbsd/lib/libc/gen/setprogname.c;l=22
    pub fn setprogname(progname: *const c_char);
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
