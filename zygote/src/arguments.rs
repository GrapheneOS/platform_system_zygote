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

//! Code for capturing the address of ARGV.

use core::ffi::{c_char, c_int, CStr};
use std::sync::Mutex;

pub(crate) static ARG: Mutex<Option<ArgumentRegion>> = Mutex::new(None);

/// A wrapper struct of the pointer to argv[0] to make it Send and Sync.
pub(crate) struct Argv0Ptr {
    pub(crate) argv0: *mut c_char,
}

#[allow(dead_code)]
impl Argv0Ptr {
    /// # Safety
    /// Users must ensure that `argv0` is a valid unique pointer to "argv[0]" prepared by the C
    /// runtime which is non-null and has the static lifetime.
    pub(crate) unsafe fn new(argv0: *mut c_char) -> Self {
        Self { argv0 }
    }

    /// # Safety
    /// Users must ensure that the raw pointer is accessed by 1 thread at a time and not copied to
    /// any other locations.
    pub(crate) unsafe fn as_raw(&self) -> *mut c_char {
        self.argv0
    }
}

// SAFETY: Users guarantee that the underlying pointer has the static lifetime.
unsafe impl Send for Argv0Ptr {}

// SAFETY: Users guarantee that the underlying pointer is unique, and is accessed by 1 thread at a
// time by `as_raw()`.
unsafe impl Sync for Argv0Ptr {}

pub(crate) struct ArgumentRegion {
    pub(crate) argv0: Argv0Ptr,
    pub(crate) capacity: usize,
}

/// Store argument information.
///
/// This function will be called from the C runtime before the main function.
/// The compiled code of this function must be in an executable file, not libraries.
///
/// libc allows us to run initialization routines registered to the .init_array section, and
/// passes argc and argv to them. We ended up using this mechanism because we cannot obtain
/// argc and argv directly in Rust.
///
/// # Safety
/// This function assumes the C runtime passes valid arguments, which is a specific behavior to
/// some libc implementations (including BIONIC).
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn args_initializer(
    argc: c_int,
    argv: *mut *mut c_char,
    _envp: *mut *mut c_char,
) {
    assert!(argc > 0);

    // SAFETY: The pointers in argv are prepared by the C runtime to point to valid memory locations.
    let (argv_first_ptr, argv_last_ptr) = unsafe { (*argv, *argv.add(argc as usize - 1)) };
    // SAFETY: Arguments are valid C strings terminated with null.
    let argv_last_cstr = unsafe { CStr::from_ptr(argv_last_ptr) };
    let last_arg_size = argv_last_cstr.count_bytes() + 1;

    // Assuming the arguments are placed into contiguous memory.
    let capacity = argv_last_ptr as usize + last_arg_size - argv_first_ptr as usize;
    assert!(capacity > 0);

    assert!(!argv_first_ptr.is_null());
    // SAFETY: `argv0` is a valid unique pointer to "argv[0]" prepared by the C runtime which
    // is non-null and has the static lifetime.
    let argv0_ptr = unsafe { Argv0Ptr::new(argv_first_ptr) };

    let mut arg = ARG.lock().unwrap();
    *arg = Some(ArgumentRegion { argv0: argv0_ptr, capacity });
}
