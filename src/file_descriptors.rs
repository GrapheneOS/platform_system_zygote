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

// TODO: Figure out a strategy for the dynamic allocations currently involved
//       in string handling.

use core::{
    ffi::{c_int, CStr},
    fmt,
};
use std::{
    cmp::Ordering,
    convert::TryFrom,
    io::Write,
    os::fd::{AsRawFd, RawFd},
};

use anyhow::{anyhow, Context, Result};
use arrayvec::{ArrayString, ArrayVec};
use zerocopy::IntoBytes;

use crate::{
    introspection::{self, assert_single_threaded},
    species::SpeciesRef,
    sys::{self, AsCStr, CStringBuffer, STRING_BUF_SIZE},
};

const DYNAMIC_ALLOW_LIST_SIZE: usize = 64;
const REGISTRY_SIZE: usize = 512;

pub const DEV_NULL_PATH: &str = "/dev/null";
pub const DEV_NULL_PATH_C: &CStr = c"/dev/null";
pub const DEV_URANDOM_PATH: &str = "/dev/urandom";

pub const PROC_PATH_FD_PREFIX: &str = "/proc/self/fd/";

static ALLOWED_FILE_PATHS: &[&str] = &[DEV_NULL_PATH, DEV_URANDOM_PATH];

static ALLOWED_SOCKET_PATHS: &[&str] = &[];

#[derive(Debug, Eq, PartialEq)]
pub enum Action {
    Close,
    Delay,
    DupeNull,
    Ignore,
    Reopen,
}

impl Action {
    fn close_delayed(&self, fd: RawFd) {
        if *self == Action::Delay {
            sys::close(fd).unwrap();
        }
    }

    fn execute(&self, fd: RawFd, info: &FileDescriptorInfo, dev_null_fd: RawFd) {
        match self {
            Action::Close => {
                sys::close(fd).unwrap();
            }
            Action::Delay | Action::Ignore => {
                // Nothing to see here
            }
            Action::DupeNull => {
                sys::dup3(dev_null_fd, fd, libc::O_CLOEXEC)
                    .unwrap_or_else(|_| panic!("Failed to dup3 fd {} to /dev/null", fd));
            }
            Action::Reopen => match info {
                FileDescriptorInfo::FileInfo {
                    path,
                    fd_flags,
                    fs_flags_open,
                    fs_flags_rest,
                    offset,
                    ..
                } => {
                    let new_fd = sys::open(path.as_cstr(), *fs_flags_open).unwrap_or_else(|_| {
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

                    sys::dup3(new_fd, fd, dup_flags).unwrap_or_else(|errno| {
                        panic!("Failed to dup3 a new file descriptor to existing descriptor (path: {:?}, existing: {}, new: {:?}): {}", path.as_cstr(), fd, new_fd, errno);
                    });

                    sys::close(new_fd).unwrap();
                }
                _ => {
                    panic!("Invalid file type ({}) registered with `Reopen` action", info);
                }
            },
        }
    }
}

// TODO: This is currently marked public so that it can be called from
//       integration tests.  Re-evaluate this and see if there is a better
//       solution.
#[derive(Eq, PartialEq)]
pub enum FileDescriptorInfo {
    AbstractSocketInfo {
        name: ArrayString<{ sys::STRING_BUF_SIZE }>,
    },
    BoundSocketInfo {
        path: ArrayString<{ sys::STRING_BUF_SIZE }>,
    },
    FifoInfo,
    FileInfo {
        dev: u64,
        inode: u64,
        path: CStringBuffer,
        fd_flags: c_int,
        fs_flags_open: c_int,
        fs_flags_rest: c_int,
        offset: libc::off64_t,
    },
}

impl fmt::Display for FileDescriptorInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AbstractSocketInfo { name } => write!(f, "Abstract Socket ({})", name),
            Self::BoundSocketInfo { path } => write!(f, "Bound Socket ({})", path),
            Self::FifoInfo => write!(f, "FIFO"),
            Self::FileInfo { path, .. } => write!(f, "File ({:?})", path.as_cstr()),
        }
    }
}

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
        //
        // TODO: Move bit-twiddling into a function
        match (stat.st_mode as libc::mode_t) & libc::S_IFMT {
            libc::S_IFIFO => Ok(FileDescriptorInfo::FifoInfo),
            libc::S_IFCHR | libc::S_IFREG => Self::get_file_info(fd, &stat),
            libc::S_IFSOCK => Self::get_socket_info(fd),
            file_type => Err(anyhow!(
                "Unable to generate info for file descriptor {}. File type: {:?}",
                fd,
                file_type
            )),
        }
    }
}

impl FileDescriptorInfo {
    fn get_file_info(fd: RawFd, stat: &libc::stat) -> Result<FileDescriptorInfo> {
        let mut path_cstr_buff = ArrayVec::<u8, STRING_BUF_SIZE>::new();
        write!(path_cstr_buff, "{}/{}\0", PROC_PATH_FD_PREFIX, fd)?;
        let path_cstr = CStr::from_bytes_until_nul(path_cstr_buff.as_slice()).unwrap();

        let link_path = sys::readlink(path_cstr)
            .with_context(|| format!("Unable to read procfs symlink for fd {}", fd))?;

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

        Ok(FileDescriptorInfo::FileInfo {
            dev: stat.st_dev,
            inode: stat.st_ino as _,
            path: link_path,
            fd_flags,
            fs_flags_open,
            fs_flags_rest,
            offset,
        })
    }

    fn get_socket_info(fd: RawFd) -> Result<FileDescriptorInfo> {
        let (addr, path_len) = sys::getsockname(fd)?;
        let sun_bytes: &[u8] = &addr.sun_path.as_bytes()[..path_len];

        match sun_bytes {
            [0, text @ ..] => Ok(FileDescriptorInfo::AbstractSocketInfo {
                name: ArrayString::from(CStr::from_bytes_until_nul(text)?.to_str()?).unwrap(),
            }),
            text => Ok(FileDescriptorInfo::BoundSocketInfo {
                path: ArrayString::from(CStr::from_bytes_with_nul(text)?.to_str()?).unwrap(),
            }),
        }
    }
}

struct FileDescriptorEntry {
    fd: RawFd,
    info: FileDescriptorInfo,
    action: Action,
}

// TODO: Either arena-allocate the strings or switch to using ArrayString
pub struct FileDescriptorRegistry {
    species: SpeciesRef,
    data: ArrayVec<FileDescriptorEntry, REGISTRY_SIZE>,
    allowed_file_paths: ArrayVec<String, DYNAMIC_ALLOW_LIST_SIZE>,
    allowed_socket_names: ArrayVec<String, DYNAMIC_ALLOW_LIST_SIZE>,
    allowed_socket_paths: ArrayVec<String, DYNAMIC_ALLOW_LIST_SIZE>,
}

impl FileDescriptorRegistry {
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

    fn abstract_socket_is_allowed(&self, name: &str) -> bool {
        self.allowed_socket_names.contains(&name.to_owned())
            || self.species.abstract_socket_is_allowed(name)
    }

    pub fn allow_file(&mut self, path: String) {
        self.allowed_file_paths.push(path);
    }

    pub fn allow_abstract_socket(&mut self, name: String) {
        self.allowed_socket_names.push(name);
    }

    pub fn allow_bound_socket(&mut self, name: String) {
        self.allowed_socket_paths.push(name);
    }

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

    fn bound_socket_is_allowed(&self, path: &str) -> bool {
        ALLOWED_SOCKET_PATHS.contains(&path)
            || self.allowed_socket_paths.contains(&path.to_owned())
            || self.species.bound_socket_is_allowed(path)
    }

    pub fn close_delayed(&self) {
        assert_single_threaded();

        for entry in &self.data {
            entry.action.close_delayed(entry.fd)
        }
    }

    pub fn execute_actions(&self) {
        assert_single_threaded();

        let dev_null_fd = sys::open(DEV_NULL_PATH_C, libc::O_RDWR | libc::O_CLOEXEC).unwrap();

        for entry in &self.data {
            entry.action.execute(entry.fd, &entry.info, dev_null_fd);
        }
    }

    fn file_is_allowed(&self, path: &CStr) -> bool {
        ALLOWED_FILE_PATHS.contains(&path.to_str().unwrap())
            || self.allowed_file_paths.contains(&path.to_str().unwrap().to_owned())
            || self.species.file_is_allowed(path)
    }

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

    pub fn register(&mut self, fd: impl AsRawFd, action: Action) {
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
                FileDescriptorInfo::AbstractSocketInfo { ref name } => {
                    if self.abstract_socket_is_allowed(name) {
                        Action::DupeNull
                    } else {
                        panic!("Abstract socket name not found in allow list ({}): {}", fd, name);
                    }
                }
                FileDescriptorInfo::BoundSocketInfo { ref path, .. } => {
                    if self.bound_socket_is_allowed(path) {
                        Action::DupeNull
                    } else {
                        panic!("Bound socket name not found in allow list ({}): {}", fd, path);
                    }
                }
                FileDescriptorInfo::FifoInfo => {
                    panic!("Unregistered FIFO fd found: {}", fd);
                }
                FileDescriptorInfo::FileInfo { ref path, .. } => {
                    if self.file_is_allowed(path.as_cstr()) {
                        self.species.get_file_action(path.as_cstr()).unwrap_or(Action::Reopen)
                    } else {
                        panic!("File path not found on allow list ({}): {:?}", fd, path);
                    }
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
    pub fn scan() {
        assert_single_threaded();

        println!("Scanning open file descriptors:");

        for fd in introspection::get_open_file_descriptors().unwrap() {
            println!("\t{} -> {}", fd, FileDescriptorInfo::try_from(fd).unwrap());
        }
    }

    pub fn size(&self) -> usize {
        self.data.len()
    }
}

#[cfg(test)]
mod test {
    use std::{fs::File, os::fd::AsRawFd};

    use super::FileDescriptorInfo;
    use crate::{
        introspection::PROC_SELF_EXE_PATH_STR,
        sys::{self, AsCStr},
        test::{get_abstract_socket, get_bound_socket, manage_test},
    };

    #[test]
    #[rustfmt::skip]
    fn test_file_descriptor_info() {
        manage_test(|| {
            // Test stdin
            assert!(
                matches!(
                    super::FileDescriptorInfo::try_from(0).unwrap(),
                    super::FileDescriptorInfo::FileInfo { .. } |
                    super::FileDescriptorInfo::FifoInfo));

            // Test FileInfo
            let exec_path = std::fs::read_link(PROC_SELF_EXE_PATH_STR).unwrap();
            assert!(
                matches!(
                    FileDescriptorInfo::try_from(File::open(&exec_path).unwrap().as_raw_fd()).unwrap(),
                    FileDescriptorInfo::FileInfo { path, .. } if path.as_cstr().to_str().unwrap() == exec_path.to_str().unwrap()));

            // Test FifoInfo
            let (pipe0, pipe1) = sys::pipe().unwrap();
            assert!(
                matches!(
                    FileDescriptorInfo::try_from(pipe0).unwrap(),
                    FileDescriptorInfo::FifoInfo));
            assert!(
                matches!(
                    FileDescriptorInfo::try_from(pipe1).unwrap(),
                    FileDescriptorInfo::FifoInfo));

            // Test AbstractSocket
            let abstract_socket_fd = get_abstract_socket(crate::test::SOCKET_NAME_1).unwrap();
            let info = FileDescriptorInfo::try_from(abstract_socket_fd).unwrap();
            assert!(
                matches!(
                    info,
                    FileDescriptorInfo::AbstractSocketInfo { name } if name.to_string() == crate::test::SOCKET_NAME_1));

            // Test BoundSocket
            let bound_socket_fd = get_bound_socket(crate::test::SOCKET_PATH_1).unwrap();
            assert!(
                matches!(
                    FileDescriptorInfo::try_from(bound_socket_fd).unwrap(),
                    FileDescriptorInfo::BoundSocketInfo { path, .. } if path.to_string() == crate::test::SOCKET_PATH_1));

            sys::close(pipe0).unwrap();
            sys::close(pipe1).unwrap();
            sys::close(abstract_socket_fd).unwrap();
            sys::close(bound_socket_fd).unwrap();
        });
    }
}
