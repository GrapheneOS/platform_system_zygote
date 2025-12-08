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

//! A library Zygote "application" used to test memory interactions between the
//! Zygote and its child processes.

use std::ffi::{c_char, c_int, CStr};

/// Entry point for the MemMark Zygote LibApp.
///
/// # Safety
/// The caller must ensure that `argc` and `argv` are valid and can be passed
/// to `std::slice::from_raw_parts`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn zygote_entry(argc: c_int, argv: *const *const c_char) -> c_int {
    println!("Hello from MemMark!");

    // SAFETY: The correctness of these arguments is the responsibility of the
    //         LibApp Zygote species
    let args: Vec<&CStr> = unsafe { std::slice::from_raw_parts(argv, argc as usize) }
        .iter()
        .map(|arg| unsafe { CStr::from_ptr(*arg) })
        .collect();

    println!("Arguments: {args:?}");

    0
}
