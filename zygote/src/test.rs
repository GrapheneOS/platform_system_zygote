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

//! This module contains functions and data used by tests.

use core::ffi::CStr;
use std::{ffi::OsStr, panic, sync::Mutex};

pub(crate) static MUTEX: Mutex<()> = Mutex::new(());

/// Path to a file in the system temp directory used for testing
pub const MOCK_FILE_PATH_1: &CStr = c"/tmp/mock_file_1";
/// Path to a file in the system temp directory used for testing
pub const MOCK_FILE_PATH_2: &CStr = c"/tmp/mock_file_2";
/// Path to a file in the system temp directory used for testing
pub const MOCK_FILE_PATH_3: &CStr = c"/tmp/mock_file_3";

/// Abstract socket name used for testing
pub const SOCKET_NAME_1: &str = "test_socket_abstract_1";
/// Abstract socket name used for testing
pub const SOCKET_NAME_2: &str = "test_socket_abstract_2";
/// Abstract socket name used for testing
pub const SOCKET_NAME_3: &str = "test_socket_abstract_3";

/// Path to a socket in the system temp directory used for testing
pub const SOCKET_PATH_1: &str = "/tmp/test_socket_bound_1";
/// Path to a socket in the system temp directory used for testing
pub const SOCKET_PATH_2: &str = "/tmp/test_socket_bound_2";
/// Path to a socket in the system temp directory used for testing
pub const SOCKET_PATH_3: &str = "/tmp/test_socket_bound_3";

/// Ensure that all test resources are removed from the system after a test run
pub fn cleanup() {
    let test_file_paths = [MOCK_FILE_PATH_1, MOCK_FILE_PATH_2, MOCK_FILE_PATH_3];

    let test_socket_paths = [SOCKET_PATH_1, SOCKET_PATH_2, SOCKET_PATH_3];

    close_all(test_file_paths.iter().map(|s| s.to_str().unwrap()));
    close_all(test_socket_paths.iter());
}

/// Unlink any of the provided file paths if they exist
fn close_all<T: AsRef<OsStr>>(paths: impl std::iter::Iterator<Item = T>) {
    for path_str in paths {
        let path = std::path::Path::new(&path_str);
        if path.exists() {
            std::fs::remove_file(path).unwrap();
        }
    }
}

/// Acquire the global test serialization lock before running the provided
/// test.  This ensures that tests evaluating behavior that requires a serial
/// environment don't interfere with each other.
#[track_caller]
pub fn manage_test<F: FnOnce() + panic::UnwindSafe>(test_body: F) {
    let _guard = serialize_test();

    let unwind_result = panic::catch_unwind(test_body);
    cleanup();
    assert!(unwind_result.is_ok());
}

/// Acquire the global test serialization lock.
pub(crate) fn serialize_test<'a>() -> std::sync::MutexGuard<'a, ()> {
    loop {
        match crate::test::MUTEX.lock() {
            Ok(guard) => break guard,
            Err(..) => crate::test::MUTEX.clear_poison(),
        }
    }
}
