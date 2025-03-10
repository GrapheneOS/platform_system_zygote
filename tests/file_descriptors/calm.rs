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

use core::assert_eq;
use std::{
    fs::File,
    os::fd::{AsRawFd, RawFd},
};

use rustix::fd::IntoRawFd;

use zygote::{
    file_descriptors::{self, Action, FileDescriptorInfo, FileDescriptorRegistry},
    introspection::assert_single_threaded,
    sys::{self, AsCStr},
    test,
};

#[track_caller]
fn assert_fd_closed(fd: RawFd) {
    assert!(!std::path::Path::exists(
        &std::path::Path::new(file_descriptors::PROC_PATH_FD_PREFIX)
            .join(fd.as_raw_fd().to_string())
    ));
}

#[track_caller]
fn assert_fd_open_to(fd: RawFd, target: &str) {
    assert!(std::path::Path::exists(
        &std::path::Path::new(file_descriptors::PROC_PATH_FD_PREFIX)
            .join(fd.as_raw_fd().to_string())
    ));

    match FileDescriptorInfo::try_from(fd).unwrap() {
        FileDescriptorInfo::AbstractSocketInfo { name } => assert_eq!(name.as_str(), target),
        FileDescriptorInfo::BoundSocketInfo { path, .. } => {
            assert_eq!(path.as_str(), target)
        }
        FileDescriptorInfo::FileInfo { path, .. } => {
            assert_eq!(path.as_cstr().to_str().unwrap(), target)
        }
        _ => panic!(
            "File descriptor {} refers to kernel object without a file system path",
            fd.as_raw_fd()
        ),
    }
}

// TODO: Clean up if this test fails
fn main() -> Result<(), std::io::Error> {
    assert_single_threaded();

    // Test Plan:
    //   * register ✓
    //   * register_new ✓
    //   * audit ✓
    //   * execute_actions
    //   * close_delayed
    //   * Allow lists:
    //     * Global ✓
    //     * Species ✓
    //     * Dynamic ✓
    //   * File types:
    //     * FIFO ✓
    //     * File ✓
    //     * Abstract Socket ✓
    //     * Bound socket ✓
    //   * Actions
    //     * Close ✓
    //     * Delay ✓
    //     * DupeNull ✓
    //     * Ignore ✓
    //     * Reopen ✓

    // TODO: If a panic occurs inside this body the cleanup code won't get called.  Why?
    test::manage_test(|| {
        /*
         * Initialize registry
         */

        let mut registry = FileDescriptorRegistry::new(&zygote::species::mock::Mock);
        assert_eq!(registry.size(), 3);
        registry.audit();

        /*
         * Create and register resources
         */

        // Test registration of FIFO descriptors
        let (pipe0, pipe1) = sys::pipe().unwrap();
        registry.register(pipe0, Action::Close);
        registry.register(pipe1, Action::Close);

        // Test automatic registration and global file allow list
        let urandom_fd = File::open(file_descriptors::DEV_URANDOM_PATH).unwrap().into_raw_fd();

        // Test automatic registration and species file allow list
        let mock_file_1 = File::create(test::MOCK_FILE_PATH_1.to_str().unwrap()).unwrap();

        // Test automatic registration and dynamic file allow list
        let mock_file_2 = File::create(test::MOCK_FILE_PATH_2.to_str().unwrap()).unwrap();
        registry.allow_file(test::MOCK_FILE_PATH_2.to_str().unwrap().to_owned());

        // Test automatic registration and species bound socket allow list
        let bound_socket_1 = test::get_bound_socket(test::SOCKET_PATH_1).unwrap();

        // Test automatic registration and dynamic bound socket allow list
        let bound_socket_2 = test::get_bound_socket(test::SOCKET_PATH_2).unwrap();
        registry.allow_bound_socket(test::SOCKET_PATH_2.into());

        // Test automatic registration and species abstract socket allow list
        let abstract_socket_1 = test::get_abstract_socket(test::SOCKET_NAME_1).unwrap();

        // Test automatic registration and dynamic abstract socket allow list
        let abstract_socket_2 = test::get_abstract_socket(test::SOCKET_NAME_2).unwrap();
        registry.allow_abstract_socket(test::SOCKET_NAME_2.into());

        // Manually register a file for testing the Delay action
        let bound_socket_3 = test::get_bound_socket(test::SOCKET_PATH_3).unwrap();
        registry.register(bound_socket_3.as_raw_fd(), Action::Delay);

        // Manually register an abstract socket for testing the Ignore action
        let abstract_socket_3 = test::get_abstract_socket(test::SOCKET_NAME_3).unwrap();
        registry.register(abstract_socket_3.as_raw_fd(), Action::Ignore);

        // Test automatic registration of new file descriptors.
        registry.register_new();

        /*
         * Test action handling
         */

        registry.execute_actions();

        // Test that the FIFO descriptors have been closed
        assert_fd_closed(pipe0);
        assert_fd_closed(pipe1);

        // Test that urandom, mock_file_1, and mock_file_2 have been re-opened
        assert_fd_open_to(urandom_fd, file_descriptors::DEV_URANDOM_PATH);
        assert_fd_open_to(mock_file_1.as_raw_fd(), test::MOCK_FILE_PATH_1.to_str().unwrap());
        assert_fd_open_to(mock_file_2.as_raw_fd(), test::MOCK_FILE_PATH_2.to_str().unwrap());

        // Test that our default action abstract and bound sockets have been duped
        // to /dev/null
        assert_fd_open_to(bound_socket_1, file_descriptors::DEV_NULL_PATH);
        assert_fd_open_to(bound_socket_2, file_descriptors::DEV_NULL_PATH);
        assert_fd_open_to(abstract_socket_1, file_descriptors::DEV_NULL_PATH);
        assert_fd_open_to(abstract_socket_2, file_descriptors::DEV_NULL_PATH);

        // Test that bound_socket_3 is still open and refers to the correct path
        assert_fd_open_to(bound_socket_3, test::SOCKET_PATH_3);

        // Test that abstract_socket_3 is still open
        assert_fd_open_to(abstract_socket_3, test::SOCKET_NAME_3);

        /*
         * Test delayed action handling
         */

        registry.close_delayed();

        // Test that bound_socket_3 has been closed
        assert_fd_closed(bound_socket_3);

        // Test that abstract_socket_3 is still open
        assert_fd_open_to(abstract_socket_3, test::SOCKET_NAME_3);
    });

    Ok(())
}
