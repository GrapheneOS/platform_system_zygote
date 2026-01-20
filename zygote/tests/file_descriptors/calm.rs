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

//! Non-panicking tests of the file descriptor registry.  These tests need to
//! be run without a test harness to avoid having unknown file descriptors
//! open during execution.

use core::{assert_eq, ops::Deref};
use std::{fs::File, os::fd::AsRawFd};

use rustix::fd::IntoRawFd;

use zygote_sys::{
    self as sys,
    procfs::{assert_fd_closed, assert_single_threaded},
};

use zygote::{
    assert_ok,
    file_descriptors::{self, assert_fd_open_to, Action, FileDescriptorRegistry, ForkType},
    test,
};

// Command-line arguments to ignore, because they are not supported by libtest-mimic.
const IGNORED_ARGS: [&str; 2] = ["-Zunstable-options", "--report-time"];

fn test_file_descriptor_registry() -> Result<(), std::io::Error> {
    assert_single_threaded();

    test::manage_test(|| {
        /*
         * Initialize registry
         */

        let mut registry = FileDescriptorRegistry::new(&zygote::species::mock::Turtle).unwrap();
        assert_eq!(registry.size(), 3);
        assert_ok!(registry.audit());

        /*
         * Create and register resources
         */

        // Test registration of FIFO descriptors
        let (pipe0, pipe1) = sys::pipe().unwrap();
        registry.register(pipe0, Action::Close).unwrap();
        registry.register(pipe1, Action::Close).unwrap();

        // Test automatic registration and global file allow list
        let urandom_fd = File::open(file_descriptors::DEV_URANDOM_PATH).unwrap().into_raw_fd();

        // Test automatic registration and species file allow list
        let mock_file_1 = File::create(test::MOCK_FILE_PATH_1.to_str().unwrap()).unwrap();

        // Test automatic registration and dynamic file allow list
        let mock_file_2 = File::create(test::MOCK_FILE_PATH_2.to_str().unwrap()).unwrap();
        registry.allow_file(test::MOCK_FILE_PATH_2.to_str().unwrap().to_owned());

        // Test automatic registration and species bound socket allow list
        let bound_socket_1 =
            sys::create_bound_socket(test::SOCKET_PATH_1, libc::SOCK_DGRAM).unwrap();

        // Test automatic registration and dynamic bound socket allow list
        let bound_socket_2 =
            sys::create_bound_socket(test::SOCKET_PATH_2, libc::SOCK_DGRAM).unwrap();
        registry.allow_bound_socket(test::SOCKET_PATH_2.into());

        // Test automatic registration and species abstract socket allow list
        let abstract_socket_1 =
            sys::create_abstract_socket(test::SOCKET_NAME_1, libc::SOCK_DGRAM).unwrap();

        // Test automatic registration and dynamic abstract socket allow list
        let abstract_socket_2 =
            sys::create_abstract_socket(test::SOCKET_NAME_2, libc::SOCK_DGRAM).unwrap();
        registry.allow_abstract_socket(test::SOCKET_NAME_2.into());

        // Manually register an abstract socket for testing the Ignore action
        let abstract_socket_3 =
            sys::create_abstract_socket(test::SOCKET_NAME_3, libc::SOCK_DGRAM).unwrap();
        registry.register(abstract_socket_3.as_raw_fd(), Action::Ignore).unwrap();

        // Test automatic registration of new file descriptors.
        registry.register_new().unwrap();

        /*
         * Test action handling
         */

        registry.execute_actions(ForkType::Application).unwrap();

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

        // Test that abstract_socket_3 is still open
        assert_fd_open_to(abstract_socket_3, test::SOCKET_NAME_3);
    });

    Ok(())
}

fn main() -> Result<(), std::io::Error> {
    let args = libtest_mimic::Arguments {
        // Force single-threaded execution to ensure file descriptor operations
        // do not interfere with one another.
        test_threads: Some(1),
        ..libtest_mimic::Arguments::from_iter(
            std::env::args().filter(|arg| !IGNORED_ARGS.contains(&arg.deref())),
        )
    };

    let tests = vec![libtest_mimic::Trial::test("file_descriptor_registry_test", || {
        Ok(test_file_descriptor_registry()?)
    })];

    libtest_mimic::run(&args, tests).exit();
}
