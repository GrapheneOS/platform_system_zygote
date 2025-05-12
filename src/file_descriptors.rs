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

//! This module implements a file descriptor hygiene registry that is used to
//! ensure that:
//!   * Every file descriptor opened by the process is registered
//!   * File descriptors that are opened are allowed to be open
//!   * File descriptors are either duped or closed appropriate after forking

// TODO: Figure out a strategy for the dynamic allocations currently involved
//       in string handling.

use core::{
    ffi::{c_int, CStr},
    fmt,
};
use std::{
    cmp::Ordering,
    convert::TryFrom,
    os::fd::{AsRawFd, RawFd},
};

use anyhow::{anyhow, bail, Context, Result};
use arrayvec::{ArrayString, ArrayVec};
use zerocopy::IntoBytes;

use crate::{
    introspection::{self, assert_single_threaded, get_proc_fd_link_info},
    species::SpeciesRef,
    sys::{self, AsCStr, CStringBuffer},
};

const DYNAMIC_ALLOW_LIST_SIZE: usize = 64;
const REGISTRY_SIZE: usize = 512;

/// Path to the null character device for Unix-like systems
pub const DEV_NULL_PATH: &str = "/dev/null";
/// Path to the null character device for Unix-like systems, represented as a
/// null-terminated C string.
pub const DEV_NULL_PATH_C: &CStr = c"/dev/null";
/// Path to the urandom character device for Unix-like systems
pub const DEV_URANDOM_PATH: &str = "/dev/urandom";

/// Metadata string reported via Proc for SignalFDs
const PROC_METADATA_SIGNALFD: &CStr = c"anon_inode:[signalfd]";

/// File paths used by the Zygote that are allowed to be registered
static ALLOWED_FILE_PATHS: &[&str] = &[DEV_NULL_PATH, DEV_URANDOM_PATH];

/// Socket paths used by the Zygote that are allowed to be registered
static ALLOWED_SOCKET_PATHS: &[&str] = &[];

/// Information necessary to identify and perform actions for supported file
/// types.
#[derive(Eq, PartialEq)]
enum FileDescriptorInfo {
    AbstractSocket {
        name: ArrayString<{ sys::STRING_BUF_SIZE }>,
    },
    BoundSocket {
        path: ArrayString<{ sys::STRING_BUF_SIZE }>,
    },
    Fifo,
    File {
        dev: u64,
        inode: u64,
        path: CStringBuffer,
        fd_flags: c_int,
        /// Flags set via the [`libc::open`] function
        fs_flags_open: c_int,
        /// Flags set via the [`libc::fcntl`] function
        fs_flags_fcntl: c_int,
        offset: libc::off64_t,
    },
    SignalFd,
}

impl fmt::Display for FileDescriptorInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AbstractSocket { name } => write!(f, "Abstract Socket ({})", name),
            Self::BoundSocket { path } => write!(f, "Bound Socket ({})", path),
            Self::Fifo => write!(f, "FIFO"),
            Self::File { path, .. } => write!(f, "File ({:?})", path.as_cstr()),
            Self::SignalFd => write!(f, "SignalFD"),
        }
    }
}

/// Support construction of FileDescriptorInfo structs from raw file
/// descriptors.  Supported file types are:
///   * Regular files
///   * Character files
///   * FIFO files
///   * Sockets
impl TryFrom<RawFd> for FileDescriptorInfo {
    type Error = anyhow::Error;

    fn try_from(fd: RawFd) -> Result<Self> {
        let stat = sys::fstat(fd)?;

        // The file descriptor registry supports regular files, character
        // devices, sockets, and pipes. Character devices must provide a
        // guarantee of sensible behavior when reopened.
        //
        // S_ISDIR : Not supported. (We could if we wanted to, but it's unused).
        // S_ISLINK : Not supported.
        // S_ISBLK : Not supported.
        match sys::get_file_type(stat) {
            libc::S_IFIFO => Ok(FileDescriptorInfo::Fifo),
            libc::S_IFCHR | libc::S_IFREG => Self::get_file_info(fd, &stat),
            libc::S_IFSOCK => Self::get_socket_info(fd),
            file_type => {
                let proc_metadata_buffer = get_proc_fd_link_info(fd).unwrap();
                let proc_metadata_cstr = proc_metadata_buffer.as_cstr().unwrap();

                if proc_metadata_cstr == PROC_METADATA_SIGNALFD {
                    Ok(FileDescriptorInfo::SignalFd)
                } else {
                    Err(anyhow!(
                        "Unable to generate info for file descriptor {}. Type: '{:?}' Metadata: '{:?}'",
                        fd,
                        file_type,
                        proc_metadata_cstr,
                    ))
                }
            }
        }
    }
}

impl FileDescriptorInfo {
    /// Gather information about the provided file descriptor from procfs,
    /// fcntl, and lseek64.  Combine this new information with the provided
    /// stat data to build a new FileInfo struct.
    fn get_file_info(fd: RawFd, stat: &libc::stat) -> Result<FileDescriptorInfo> {
        let link_path = get_proc_fd_link_info(fd)?;

        // File descriptor flags : currently on FD_CLOEXEC. We can set these
        // using F_SETFD - we're single threaded at this point of execution so
        // there won't be any races.
        let fd_flags = sys::fcntl_getfd(fd)
            .with_context(|| format!("Unable to call fcntl(F_GETFD) for FD {}", fd))?;

        // File status flags :
        // - File access mode : (O_RDONLY, O_WRONLY...) we'll pass these through
        //   to the open() call.
        //
        // - File creation flags : (O_CREAT, O_EXCL...) - there's not much we can
        //   do about these, since the file has already been created. We shall ignore
        //   them here.
        //
        // - Other flags : We'll have to set these via F_SETFL. On linux, F_SETFL
        //   can only set O_APPEND, O_ASYNC, O_DIRECT, O_NOATIME, and O_NONBLOCK.
        //   In particular, it can't set O_SYNC and O_DSYNC. We'll have to test for
        //   their presence and pass them in to open().
        let fs_flags = sys::fcntl_getfl(fd)
            .with_context(|| format!("Unable to call fcntl(F_GETFL) for FD {}", fd))?;

        // File offset : Ignore the offset for non seekable files.
        let offset = sys::lseek64(fd, 0, libc::SEEK_CUR).unwrap_or_default();

        let fs_flags_open_bitmask =
            libc::O_RDONLY | libc::O_WRONLY | libc::O_RDWR | libc::O_DSYNC | libc::O_SYNC;
        let fs_flags_open = fs_flags & fs_flags_open_bitmask;
        let fs_flags_rest = fs_flags & !fs_flags_open_bitmask;

        Ok(FileDescriptorInfo::File {
            dev: stat.st_dev,
            inode: stat.st_ino as _,
            path: link_path,
            fd_flags,
            fs_flags_open,
            fs_flags_fcntl: fs_flags_rest,
            offset,
        })
    }

    /// Get the socket name and parse it into either a Abstract or Bound
    /// SocketInfo struct.
    fn get_socket_info(fd: RawFd) -> Result<FileDescriptorInfo> {
        let (addr, path_len) = sys::getsockname(fd)?;
        let sun_bytes: &[u8] = &addr.sun_path.as_bytes()[..path_len];

        match sun_bytes {
            [0, text @ ..] => Ok(FileDescriptorInfo::AbstractSocket {
                name: ArrayString::from(CStr::from_bytes_until_nul(text)?.to_str()?).unwrap(),
            }),
            text => Ok(FileDescriptorInfo::BoundSocket {
                path: ArrayString::from(CStr::from_bytes_with_nul(text)?.to_str()?).unwrap(),
            }),
        }
    }
}

/// Use `procfs` to assert that a provided file descriptor is open and either
/// has has a file path equal to `target` or is an abstract socket with
/// `target` as its name.
#[cfg(feature = "test")]
#[track_caller]
pub fn assert_fd_open_to(fd: RawFd, target: &str) {
    assert!(crate::introspection::get_proc_fd_path(fd).exists());

    match FileDescriptorInfo::try_from(fd).unwrap() {
        FileDescriptorInfo::AbstractSocket { name } => assert_eq!(name.as_str(), target),
        FileDescriptorInfo::BoundSocket { path, .. } => {
            assert_eq!(path.as_str(), target)
        }
        FileDescriptorInfo::File { path, .. } => {
            assert_eq!(path.as_cstr().unwrap().to_str().unwrap(), target)
        }
        _ => panic!(
            "File descriptor {} refers to kernel object without a file system path",
            fd.as_raw_fd()
        ),
    }
}

/// Tags used by the registry to specify what operations to perform when
/// [`FileDescriptorRegistry::execute_actions`] and
/// [`FileDescriptorRegistry::close_delayed`] are called.
#[derive(Debug, Eq, PartialEq)]
pub enum Action {
    /// Close the file when `execute_action` is called
    Close,
    /// Close the file when `close_delayed` is called
    Delay,
    /// Use the `dup3` call to make the file descriptor point to /dev/null
    DupeNull,
    /// Do nothing with the file descriptor
    Ignore,
    /// Re-open the file when `execute_action` is called, thus ensuring that
    /// child processes point to a unique kernel file structure
    Reopen,
}

struct FileDescriptorEntry {
    fd: RawFd,
    info: FileDescriptorInfo,
    action: Action,
}

impl FileDescriptorEntry {
    /// Close the associated file descriptor if it is registered with the Delay
    /// action.  This function shall only be called after a fork even in the
    /// context of the child process.
    fn close_delayed(&self) {
        if self.action == Action::Delay {
            sys::close(self.fd).unwrap();
        }
    }

    /// Handle Close, DupeNull, and Reopen actions for the associated file
    /// descriptor.  This function shall only be called after a fork even in
    /// the context of the child process.
    fn execute(&self, dev_null_fd: RawFd) {
        match self.action {
            Action::Close => {
                sys::close(self.fd).unwrap();
            }
            Action::Delay | Action::Ignore => {
                // Nothing to see here
            }
            Action::DupeNull => {
                sys::dup3(dev_null_fd, self.fd, libc::O_CLOEXEC)
                    .unwrap_or_else(|_| panic!("Failed to dup3 fd {} to /dev/null", self.fd));
            }
            Action::Reopen => match &self.info {
                FileDescriptorInfo::File {
                    path,
                    fd_flags,
                    fs_flags_open,
                    fs_flags_fcntl: fs_flags_rest,
                    offset,
                    ..
                } => {
                    let new_fd =
                        sys::open(path.as_cstr().unwrap(), *fs_flags_open).unwrap_or_else(|_| {
                            panic!(
                                "Failed to open new file descriptor to existing path: {:?}",
                                path.as_cstr()
                            )
                        });

                    sys::fcntl_setfd(new_fd, *fd_flags).unwrap_or_else(|errno| {
                        panic!("Failed to set descriptor flags for new file descriptor (path: {:?}, flags: {:?}): {}", path.as_cstr(), fd_flags, errno);
                    });

                    sys::fcntl_setfl(new_fd, *fs_flags_rest).unwrap_or_else(|errno| {
                        panic!("Failed to set status flags for new file descriptor (path: {:?}, flags: {:?}): {}", path.as_cstr(), fs_flags_rest, errno);
                    });

                    sys::lseek64(new_fd, *offset, libc::SEEK_SET).unwrap_or_else(|errno| {
                        panic!("Failed to set seek head for new file descriptor (path: {:?}, offset: {}): {}", path.as_cstr(), offset, errno);
                    });

                    // TODO: Move bit-twiddling into a function
                    let dup_flags = if (fd_flags & libc::FD_CLOEXEC) == libc::FD_CLOEXEC {
                        libc::O_CLOEXEC
                    } else {
                        0
                    };

                    sys::dup3(new_fd, self.fd, dup_flags).unwrap_or_else(|errno| {
                        panic!("Failed to dup3 a new file descriptor to existing descriptor (path: {:?}, existing: {}, new: {:?}): {}", path.as_cstr(), self.fd, new_fd, errno);
                    });

                    sys::close(new_fd).unwrap();
                }
                _ => {
                    panic!("Invalid file type ({}) registered with `Reopen` action", &self.info);
                }
            },
        }
    }
}

// TODO: Either arena-allocate the strings or switch to using ArrayString

/// A struct for associating file descriptor information with [`Action`]s.
/// This can be used to ensure file descriptors are accounted for and handled
/// appropriately when the Zygote forks a new process.
pub struct FileDescriptorRegistry {
    species: SpeciesRef,
    data: ArrayVec<FileDescriptorEntry, REGISTRY_SIZE>,
    allowed_file_paths: ArrayVec<String, DYNAMIC_ALLOW_LIST_SIZE>,
    allowed_socket_names: ArrayVec<String, DYNAMIC_ALLOW_LIST_SIZE>,
    allowed_socket_paths: ArrayVec<String, DYNAMIC_ALLOW_LIST_SIZE>,
}

impl FileDescriptorRegistry {
    /// Construct a new registry for a given species.  The Species reference
    /// is used to query for species specific allowed files and sockets.
    pub fn new(species: SpeciesRef) -> Self {
        let mut registry = Self {
            species,
            data: ArrayVec::new(),
            allowed_file_paths: ArrayVec::new(),
            allowed_socket_names: ArrayVec::new(),
            allowed_socket_paths: ArrayVec::new(),
        };

        // Register the stdio file descriptors
        registry.register(0, Action::Ignore);
        registry.register(1, Action::Ignore);
        registry.register(2, Action::Ignore);

        registry
    }

    /// Queries the Zygote and species abstract socket allow lists
    fn abstract_socket_is_allowed(&self, name: &str) -> bool {
        self.allowed_socket_names.contains(&name.to_owned())
            || self.species.abstract_socket_is_allowed(name)
    }

    /// Adds a file path to the allow list
    pub fn allow_file(&mut self, path: String) {
        self.allowed_file_paths.push(path);
    }

    /// Adds a socket name to the allow list
    pub fn allow_abstract_socket(&mut self, name: String) {
        self.allowed_socket_names.push(name);
    }

    /// Adds a socket path to the allow list
    pub fn allow_bound_socket(&mut self, name: String) {
        self.allowed_socket_paths.push(name);
    }

    /// Iterate over all open file descriptors and compare them to the
    /// descriptors in the registry.  Panic if:
    ///   * There are any files open that are not in the registry
    ///   * There are files descriptors in the registry that are no longer open
    ///   * A file descriptor's saved state is not equal to the current state
    pub fn audit(&self) {
        assert_single_threaded();

        let open_fds: Vec<RawFd> = introspection::get_open_file_descriptors().unwrap();
        for index in 0..std::cmp::max(self.data.len(), open_fds.len()) {
            if open_fds.len() <= index || self.data[index].fd < open_fds[index] {
                panic!(
                    "File descriptor {} ({}) was CLOSED unexpectedly.",
                    self.data[index].fd, self.data[index].info
                );
            } else if self.data.len() <= index || self.data[index].fd > open_fds[index] {
                panic!(
                    "File descriptor {} ({}) was OPENED unexpectedly.",
                    open_fds[index],
                    FileDescriptorInfo::try_from(open_fds[index]).unwrap()
                );
            } else
            /* if self.data[index].fd == open_fds[index] */
            {
                let current_info = FileDescriptorInfo::try_from(open_fds[index]).unwrap();
                if self.data[index].info != current_info {
                    panic!(
                        "File descriptor {} ({}) has been REOPENED or MODIFIED",
                        self.data[index].fd, self.data[index].info
                    );
                }
            }
        }
    }

    /// Queries the Zygote and species bound socket allow lists
    fn bound_socket_is_allowed(&self, path: &str) -> bool {
        ALLOWED_SOCKET_PATHS.contains(&path)
            || self.allowed_socket_paths.contains(&path.to_owned())
            || self.species.bound_socket_is_allowed(path)
    }

    /// Close all file descriptors registered with the Delay action.  This
    /// should occur after the Zygote is done using all management file
    /// descriptors.
    pub fn close_delayed(&self) {
        assert_single_threaded();

        for entry in &self.data {
            entry.close_delayed()
        }
    }

    /// Iterate through the registry and perform all Close, DupeNull, and
    /// Reopen actions.  This should be performed immediately after a fork
    /// event.
    pub fn execute_actions(&self) {
        assert_single_threaded();

        let dev_null_fd = sys::open(DEV_NULL_PATH_C, libc::O_RDWR | libc::O_CLOEXEC).unwrap();

        for entry in &self.data {
            entry.execute(dev_null_fd);
        }
    }

    /// Queries the Zygote and species allowed files lists.
    fn file_is_allowed(&self, path: &CStr) -> bool {
        ALLOWED_FILE_PATHS.contains(&path.to_str().unwrap())
            || self.allowed_file_paths.contains(&path.to_str().unwrap().to_owned())
            || self.species.file_is_allowed(path)
    }

    /// Searches through the sorted list of file descriptors (starting from
    /// `index_state`) to check if a given descriptor is registered.
    fn is_registered(&self, fd: RawFd, index_state: &mut usize, index_end: usize) -> bool {
        while *index_state < index_end {
            match self.data[*index_state].fd.cmp(&fd) {
                Ordering::Less => *index_state += 1,
                Ordering::Equal => {
                    *index_state += 1;
                    return true;
                }
                _ => return false,
            }
        }

        false
    }

    /// Adds a file descriptor to the registry and associates it with the
    /// provided action.
    pub fn register(&mut self, fd: RawFd, action: Action) {
        assert_single_threaded();

        match self.data.binary_search_by(|entry| entry.fd.cmp(&fd.as_raw_fd())) {
            Ok(_) => {
                panic!(
                    "Attempting to register an already registered file descriptor: {}",
                    fd.as_raw_fd()
                );
            }
            Err(insert_index) => {
                self.data.insert(
                    insert_index,
                    FileDescriptorEntry {
                        fd: fd.as_raw_fd(),
                        info: FileDescriptorInfo::try_from(fd.as_raw_fd()).unwrap(),
                        action,
                    },
                );
            }
        }
    }

    /// Remove the given file descriptor from the registry
    pub fn remove(&mut self, fd: RawFd) -> Result<()> {
        self.data.remove(
            self.data
                .as_slice()
                .binary_search_by(|entry| entry.fd.cmp(&fd))
                .or_else(|_| bail!("File descriptor is not registered: {}", fd))?,
        );

        Ok(())
    }

    /// Iterate through all open file descriptors and register any unregistered
    /// descriptors using a default action.
    pub fn register_new(&mut self) {
        assert_single_threaded();
        assert!(self.data.is_sorted_by_key(|entry| entry.fd));

        let mut registry_index: usize = 0;
        let num_preexisting_entries = self.data.len();

        let open_fds: Vec<RawFd> = introspection::get_open_file_descriptors().unwrap();

        for fd in open_fds {
            if self.is_registered(fd, &mut registry_index, num_preexisting_entries) {
                continue;
            }

            let info = FileDescriptorInfo::try_from(fd).unwrap();
            let action = match info {
                FileDescriptorInfo::AbstractSocket { ref name } => {
                    if self.abstract_socket_is_allowed(name) {
                        Action::DupeNull
                    } else {
                        panic!("Abstract socket name not found in allow list ({}): {}", fd, name);
                    }
                }
                FileDescriptorInfo::BoundSocket { ref path, .. } => {
                    if self.bound_socket_is_allowed(path) {
                        Action::DupeNull
                    } else {
                        panic!("Bound socket name not found in allow list ({}): {}", fd, path);
                    }
                }
                FileDescriptorInfo::Fifo => {
                    panic!("Unregistered FIFO fd found: {}", fd);
                }
                FileDescriptorInfo::File { ref path, .. } => {
                    if self.file_is_allowed(path.as_cstr().unwrap()) {
                        self.species
                            .get_file_action(path.as_cstr().unwrap())
                            .unwrap_or(Action::Reopen)
                    } else {
                        panic!("File path not found on allow list ({}): {:?}", fd, path);
                    }
                }
                FileDescriptorInfo::SignalFd => {
                    panic!("Unregistered signal fd found: {}", fd);
                }
            };

            self.data.push(FileDescriptorEntry { fd, info, action });
        }

        if self.data.len() != num_preexisting_entries {
            self.data.sort_by_key(|entry| entry.fd);
        }
    }

    // TODO: This is a debugging/development function and should be deleted
    //       before shipping the native Zygote.

    /// Debugging function
    pub fn scan() {
        assert_single_threaded();

        println!("Scanning open file descriptors:");

        for fd in introspection::get_open_file_descriptors().unwrap() {
            println!("\t{} -> {}", fd, FileDescriptorInfo::try_from(fd).unwrap());
        }
    }

    /// Return the number of registered file descriptors.
    pub fn size(&self) -> usize {
        self.data.len()
    }
}

#[cfg(test)]
mod test {
    use std::{fs::File, os::fd::AsRawFd};

    use super::FileDescriptorInfo;
    use crate::{
        introspection::get_executable_path,
        sys::{self, create_abstract_socket, create_bound_socket, AsCStr},
        test::manage_test,
    };

    #[test]
    #[rustfmt::skip]
    fn test_file_descriptor_info() {
        manage_test(|| {
            // Test stdin
            assert!(
                matches!(
                    super::FileDescriptorInfo::try_from(0).unwrap(),
                    super::FileDescriptorInfo::File { .. } |
                    super::FileDescriptorInfo::Fifo));

            // Test FileInfo
            let exec_path = get_executable_path().unwrap();
            assert!(
                matches!(
                    FileDescriptorInfo::try_from(File::open(&exec_path).unwrap().as_raw_fd()).unwrap(),
                    FileDescriptorInfo::File { path, .. } if path.as_cstr().unwrap().to_str().unwrap() == exec_path.to_str().unwrap()));

            // Test FifoInfo
            let (pipe0, pipe1) = sys::pipe().unwrap();
            assert!(
                matches!(
                    FileDescriptorInfo::try_from(pipe0).unwrap(),
                    FileDescriptorInfo::Fifo));
            assert!(
                matches!(
                    FileDescriptorInfo::try_from(pipe1).unwrap(),
                    FileDescriptorInfo::Fifo));

            // Test AbstractSocket
            let abstract_socket_fd = create_abstract_socket(crate::test::SOCKET_NAME_1, libc::SOCK_DGRAM).unwrap();
            let info = FileDescriptorInfo::try_from(abstract_socket_fd).unwrap();
            assert!(
                matches!(
                    info,
                    FileDescriptorInfo::AbstractSocket { name } if name.to_string() == crate::test::SOCKET_NAME_1));

            // Test BoundSocket
            let bound_socket_fd = create_bound_socket(crate::test::SOCKET_PATH_1, libc::SOCK_DGRAM).unwrap();
            assert!(
                matches!(
                    FileDescriptorInfo::try_from(bound_socket_fd).unwrap(),
                    FileDescriptorInfo::BoundSocket { path, .. } if path.to_string() == crate::test::SOCKET_PATH_1));

            sys::close(pipe0).unwrap();
            sys::close(pipe1).unwrap();
            sys::close(abstract_socket_fd).unwrap();
            sys::close(bound_socket_fd).unwrap();
        });
    }
}
