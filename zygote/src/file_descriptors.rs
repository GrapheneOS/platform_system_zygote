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

use core::{ffi::c_int, fmt};
use std::{
    cmp::Ordering,
    convert::TryFrom,
    ffi::{CStr, CString, FromBytesWithNulError},
    os::fd::RawFd,
};

use arrayvec::{ArrayString, ArrayVec};
use itertools::{EitherOrBoth, Itertools};
use log::info;
use thiserror::Error;
use zerocopy::IntoBytes;

use crate::species::SpeciesRef;
use zygote_sys::{
    self as sys,
    procfs::{debug_assert_single_threaded, get_proc_fd_link_info, ProcFdIterator, ProcFsError},
    AsCStr, CStringBuffer, BUFFER_SIZE_STRINGS,
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
/// Peer socket paths that the Zygote is allowed to connect to
static ALLOWED_PEER_SOCKET_PATHS: &[&str] = &[];

#[derive(Clone, Debug, Eq, PartialEq)]
enum SocketAddress {
    Path(ArrayString<{ sys::BUFFER_SIZE_STRINGS }>),
    Abstract(ArrayString<{ sys::BUFFER_SIZE_STRINGS }>),
}

impl fmt::Display for SocketAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Path(path) => write!(f, "{}", path.as_str()),
            Self::Abstract(path) => write!(f, "@{}", path.as_str()),
        }
    }
}

impl AsRef<str> for SocketAddress {
    fn as_ref(&self) -> &str {
        match self {
            Self::Path(path) => path,
            Self::Abstract(name) => name,
        }
    }
}

/// Errors that can occur when gathering information about file descriptors.
#[derive(Debug, Error)]
enum FileDescriptorInfoError {
    #[error("Unable to fetch the file descriptor flags: {0}")]
    DescriptorFlagsFailure(sys::Error),

    #[error("OS provided string is not nul-terminated")]
    InvalidCString,

    #[error("File descriptor {0} is not a UNIX-domain socket.")]
    InvalidSocketType(RawFd),

    #[error(
        "Default buffer size ({buffer_size} bytes) is too small to fit the name ({name_size} bytes)"
    )]
    NameBufferTooSmall { buffer_size: usize, name_size: usize },

    #[error("Failed to get peer name for file descriptor {fd}: {error:?}")]
    PeerNameFailure { fd: RawFd, error: sys::Error },

    #[error("Error reading from procfs: {0}")]
    ProcFsError(#[from] ProcFsError),

    #[error("Failed to stat file descriptor {fd}: {error:?}")]
    StatFailure { fd: RawFd, error: sys::Error },

    #[error("Unable to fetch the file descriptor status flags: {0}")]
    StatusFlagsFailure(sys::Error),

    #[error("Unsupported file type {file_type} for file descriptor {fd}")]
    UnsupportedFileType { fd: RawFd, file_type: u32 },

    /// A provided byte array or OS string is not a valid UTF-8 encoded string
    #[error("UTF-8 error: {0}")]
    Utf8(#[from] core::str::Utf8Error),
}

impl From<FromBytesWithNulError> for FileDescriptorInfoError {
    fn from(_: FromBytesWithNulError) -> Self {
        Self::InvalidCString
    }
}

type FileDescriptorInfoResult<T> = Result<T, FileDescriptorInfoError>;

/// Information necessary to identify and perform actions for supported file
/// types.
#[derive(Clone, Debug, Eq, PartialEq)]
enum FileDescriptorInfo {
    BoundSocket(SocketAddress),
    ConnectedSocket(SocketAddress),
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
            Self::BoundSocket(address) => write!(f, "Bound Socket ({address})"),
            Self::ConnectedSocket(address) => write!(f, "Connected Socket ({address})"),
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
    type Error = FileDescriptorInfoError;

    fn try_from(fd: RawFd) -> FileDescriptorInfoResult<Self> {
        let stat =
            sys::fstat(fd).map_err(|error| FileDescriptorInfoError::StatFailure { fd, error })?;

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
                let proc_metadata_buffer = get_proc_fd_link_info(fd)?;
                let proc_metadata_cstr = proc_metadata_buffer
                    .as_cstr()
                    .map_err(|_| FileDescriptorInfoError::InvalidCString)?;

                if proc_metadata_cstr == PROC_METADATA_SIGNALFD {
                    Ok(FileDescriptorInfo::SignalFd)
                } else {
                    // The conversion of the `file_type` value is only
                    // necessary on some systems, but it is easier to leave
                    // the call in and suppress the warnings than it is to
                    // fully enumerate the conversions for each platform.
                    #[allow(clippy::useless_conversion)]
                    Err(FileDescriptorInfoError::UnsupportedFileType {
                        fd,
                        file_type: file_type.into(),
                    })
                }
            }
        }
    }
}

impl FileDescriptorInfo {
    /// Gather information about the provided file descriptor from procfs,
    /// fcntl, and lseek64.  Combine this new information with the provided
    /// stat data to build a new FileInfo struct.
    fn get_file_info(fd: RawFd, stat: &libc::stat) -> FileDescriptorInfoResult<Self> {
        let link_path = get_proc_fd_link_info(fd)?;

        // File descriptor flags : currently on FD_CLOEXEC. We can set these
        // using F_SETFD - we're single threaded at this point of execution so
        // there won't be any races.
        let fd_flags =
            sys::fcntl_getfd(fd).map_err(FileDescriptorInfoError::DescriptorFlagsFailure)?;

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
        let fs_flags = sys::fcntl_getfl(fd).map_err(FileDescriptorInfoError::StatusFlagsFailure)?;

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
    /// SocketInfo struct.  The operation will fail if the provided fd is not
    /// a socket.
    fn get_socket_info(fd: RawFd) -> FileDescriptorInfoResult<Self> {
        match sys::getsockname(fd)
            .map_err(|error| FileDescriptorInfoError::PeerNameFailure { fd, error })?
        {
            Some((addr, path_len)) => {
                let sun_bytes = &addr.sun_path.as_bytes()[..path_len];
                let socket_address = Self::get_socket_address(sun_bytes)?;
                Ok(FileDescriptorInfo::BoundSocket(socket_address))
            }
            None => {
                let (addr, path_len) = sys::getpeername(fd)
                    .map_err(|error| FileDescriptorInfoError::PeerNameFailure { fd, error })?
                    .ok_or(FileDescriptorInfoError::InvalidSocketType(fd))?;
                let sun_bytes = &addr.sun_path.as_bytes()[..path_len];
                let socket_address = Self::get_socket_address(sun_bytes)?;
                Ok(FileDescriptorInfo::ConnectedSocket(socket_address))
            }
        }
    }

    fn get_socket_address(sun_bytes: &[u8]) -> FileDescriptorInfoResult<SocketAddress> {
        match sun_bytes {
            [0, text @ ..] => Ok(SocketAddress::Abstract(
                // Retain any NUL bytes in abstract socket. These are *not* truncated when looking
                // up abstract sockets, unlike bound sockets which uses NUL-terminated filesystem
                // paths.
                ArrayString::from(std::str::from_utf8(text)?).map_err(|_| {
                    FileDescriptorInfoError::NameBufferTooSmall {
                        buffer_size: BUFFER_SIZE_STRINGS,
                        name_size: text.len(),
                    }
                })?,
            )),
            text => Ok(SocketAddress::Path(
                // getsockname() on bound sockets always return NUL-terminated names, so use
                // from_bytes_with_nul() to truncated that here.
                ArrayString::from(CStr::from_bytes_with_nul(text)?.to_str()?).map_err(|_| {
                    FileDescriptorInfoError::NameBufferTooSmall {
                        buffer_size: BUFFER_SIZE_STRINGS,
                        name_size: text.len(),
                    }
                })?,
            )),
        }
    }

    fn type_str(self) -> &'static str {
        match self {
            Self::BoundSocket(_) => "Bound Socket",
            Self::ConnectedSocket(_) => "Connected Socket",
            Self::Fifo => "FIFO",
            Self::File { .. } => "File",
            Self::SignalFd => "SignalFD",
        }
    }
}

/// Use `procfs` to assert that a provided file descriptor is open and either
/// has has a file path equal to `target` or is an abstract socket with
/// `target` as its name.
#[cfg(feature = "test")]
#[track_caller]
pub fn assert_fd_open_to(fd: RawFd, target: &str) {
    assert!(zygote_sys::procfs::get_proc_fd_path(fd).exists());

    match FileDescriptorInfo::try_from(fd).unwrap() {
        FileDescriptorInfo::BoundSocket(address) => {
            assert_eq!(address.as_ref(), target)
        }
        FileDescriptorInfo::ConnectedSocket(address) => {
            assert_eq!(address.as_ref(), target)
        }
        FileDescriptorInfo::File { path, .. } => {
            assert_eq!(path.as_cstr().unwrap().to_str().unwrap(), target)
        }
        _ => panic!("File descriptor {} refers to kernel object without a file system path", fd),
    }
}

/// Tags used by the registry to specify what operations to perform when
/// [`FileDescriptorRegistry::execute_actions`] is called.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    /// Close the file when `execute_action` is called
    Close,
    /// Close the file when `execute_action` is called unless the child process
    /// is a subspecies process.
    CloseUnlessSpawnSubspecies,
    /// Use the `dup3` call to make the file descriptor point to /dev/null
    DupeNull,
    /// Do nothing with the file descriptor
    Ignore,
    /// Re-open the file when `execute_action` is called, thus ensuring that
    /// child processes point to a unique kernel file structure
    Reopen,
}

#[derive(Debug, Clone, Copy)]
/// The type of the spawned child process.
pub enum ForkType {
    /// The process is for regular applications
    Application,
    /// The process is a subspecies Zygote tailored for a specific application
    Subspecies,
}

#[derive(Clone, Debug, Error)]
enum FileDescriptorEntryErrorRepr {
    /// Failed to close a file descriptor
    #[error("Failed to close file descriptor {fd}: {error}")]
    CloseFailed { fd: RawFd, error: sys::Error },

    /// Failed to use `dup3` to clone a file descriptor
    #[error(
        "Failed to dup3 new file descriptor ({new_fd}) to existing descriptor ({existing_fd} -> {path:?}): {error}"
    )]
    DupFailed { existing_fd: RawFd, new_fd: RawFd, path: CString, error: sys::Error },

    /// Failed to dup3 a file descriptor to /dev/null
    #[error("Failed to dup3 file descriptor {fd} to /dev/null: {error}")]
    DupNullFailed { fd: RawFd, error: sys::Error },

    /// OS provided string is not nul-terminated
    #[error("OS provided string is not nul-terminated")]
    InvalidCString,

    /// Failed to create a new file descriptor the entry's path
    #[error("Failed to open new file descriptor to the entry's path ({path:?}): {error:?}")]
    OpenFailure { path: CString, error: sys::Error },

    /// Failed to seek to the recorded offset
    #[error(
        "Failed to set seek head for new file descriptor (fd: {fd}, path: {path:?}, offset: {offset}): {error:?}"
    )]
    SeekFailure { fd: RawFd, path: CString, offset: i64, error: sys::Error },

    /// Failed to set the file descriptor flags
    #[error(
        "Failed to set descriptor flags for new file descriptor (fd: {fd}, path: {path:?}, flags: {flags}): {error}"
    )]
    SetFdFlagsFailure { fd: RawFd, path: CString, flags: i32, error: sys::Error },

    /// Failed to set the file status flags
    #[error(
        "Failed to set status flags for new file descriptor (fd: {fd}, path: {path:?}, flags: {flags}): {error}"
    )]
    SetFdStatusFlagsFailure { fd: RawFd, path: CString, flags: i32, error: sys::Error },
}

/// Errors that can occur when executing an Entry's action.
#[derive(Debug, Error)]
#[error(transparent)]
pub struct FileDescriptorEntryError(#[from] FileDescriptorEntryErrorRepr);

impl From<std::ffi::FromBytesUntilNulError> for FileDescriptorEntryError {
    fn from(_: std::ffi::FromBytesUntilNulError) -> Self {
        Self(FileDescriptorEntryErrorRepr::InvalidCString)
    }
}

impl FileDescriptorEntryError {
    fn close_failed(fd: RawFd, error: sys::Error) -> Self {
        Self(FileDescriptorEntryErrorRepr::CloseFailed { fd, error })
    }

    fn dup_failed(existing_fd: RawFd, new_fd: RawFd, path: &CStr, error: sys::Error) -> Self {
        Self(FileDescriptorEntryErrorRepr::DupFailed {
            existing_fd,
            new_fd,
            path: path.to_owned(),
            error,
        })
    }

    fn dup_null_failed(fd: RawFd, error: sys::Error) -> Self {
        Self(FileDescriptorEntryErrorRepr::DupNullFailed { fd, error })
    }

    fn invalid_cstring() -> Self {
        Self(FileDescriptorEntryErrorRepr::InvalidCString)
    }

    fn open_failure(path: &CStr, error: sys::Error) -> Self {
        Self(FileDescriptorEntryErrorRepr::OpenFailure { path: path.to_owned(), error })
    }

    fn seek_failure(fd: RawFd, path: &CStr, offset: i64, error: sys::Error) -> Self {
        Self(FileDescriptorEntryErrorRepr::SeekFailure { fd, path: path.to_owned(), offset, error })
    }

    fn set_fd_flags_failure(fd: RawFd, path: &CStr, flags: i32, error: sys::Error) -> Self {
        Self(FileDescriptorEntryErrorRepr::SetFdFlagsFailure {
            fd,
            path: path.to_owned(),
            flags,
            error,
        })
    }

    fn set_fd_status_flags_failure(fd: RawFd, path: &CStr, flags: i32, error: sys::Error) -> Self {
        Self(FileDescriptorEntryErrorRepr::SetFdStatusFlagsFailure {
            fd,
            path: path.to_owned(),
            flags,
            error,
        })
    }
}

#[derive(Debug)]
struct FileDescriptorEntry {
    fd: RawFd,
    info: FileDescriptorInfo,
    action: Action,
    executed: bool,
}

impl FileDescriptorEntry {
    /// Create a new file descriptor registry entry
    fn new(fd: RawFd, info: FileDescriptorInfo, action: Action) -> Self {
        Self { fd, info, action, executed: false }
    }

    /// Check to see if the file descriptor has been closed
    fn closed(&self) -> bool {
        match self.action {
            Action::Close | Action::CloseUnlessSpawnSubspecies => self.executed,
            Action::DupeNull | Action::Ignore | Action::Reopen => false,
        }
    }

    /// Handle Close, DupeNull, and Reopen actions for the associated file
    /// descriptor.  This function shall only be called after a fork even in
    /// the context of the child process.  The action will only be executed the
    /// first time the function is called, in which case `true` is returned.
    /// Subsequent calls will not execute the action and return `false`.
    fn execute(
        &mut self,
        dev_null_fd: RawFd,
        fork_type: ForkType,
    ) -> Result<bool, FileDescriptorEntryError> {
        if !self.executed {
            match self.action {
                Action::Close => {
                    sys::close(self.fd)
                        .map_err(|error| FileDescriptorEntryError::close_failed(self.fd, error))?;
                    self.executed = true;
                }
                Action::CloseUnlessSpawnSubspecies => {
                    if matches!(fork_type, ForkType::Application) {
                        sys::close(self.fd).map_err(|error| {
                            FileDescriptorEntryError::close_failed(self.fd, error)
                        })?;
                        self.executed = true;
                    }
                }
                Action::DupeNull => {
                    sys::dup3(dev_null_fd, self.fd, libc::O_CLOEXEC).map_err(|error| {
                        FileDescriptorEntryError::dup_null_failed(self.fd, error)
                    })?;
                    self.executed = true;
                }
                Action::Ignore => {
                    // Nothing to see here
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
                        let path_cstr = path
                            .as_cstr()
                            .map_err(|_| FileDescriptorEntryError::invalid_cstring())?;
                        let new_fd =
                            sys::open(path_cstr, *fs_flags_open, None).map_err(|error| {
                                FileDescriptorEntryError::open_failure(path_cstr, error)
                            })?;

                        sys::fcntl_setfd(new_fd, *fd_flags).map_err(|error| {
                            FileDescriptorEntryError::set_fd_flags_failure(
                                new_fd, path_cstr, *fd_flags, error,
                            )
                        })?;

                        sys::fcntl_setfl(new_fd, *fs_flags_rest).map_err(|error| {
                            FileDescriptorEntryError::set_fd_status_flags_failure(
                                new_fd,
                                path_cstr,
                                *fs_flags_rest,
                                error,
                            )
                        })?;

                        sys::lseek64(new_fd, *offset, libc::SEEK_SET).map_err(|error| {
                            FileDescriptorEntryError::seek_failure(
                                new_fd, path_cstr, *offset, error,
                            )
                        })?;

                        // TODO: Move bit-twiddling into a function
                        let dup_flags = if (fd_flags & libc::FD_CLOEXEC) == libc::FD_CLOEXEC {
                            libc::O_CLOEXEC
                        } else {
                            0
                        };

                        sys::dup3(new_fd, self.fd, dup_flags).map_err(|error| {
                            FileDescriptorEntryError::dup_failed(self.fd, new_fd, path_cstr, error)
                        })?;

                        sys::close(new_fd).map_err(|error| {
                            FileDescriptorEntryError::close_failed(self.fd, error)
                        })?;
                        self.executed = true;
                    }
                    _ => {
                        // The FileDescriptorRegistry::register() function
                        // returns an error if the caller attempts to register
                        // any other file type with the Reopen action.
                        unreachable!()
                    }
                },
            }
        }
        Ok(self.executed)
    }

    /// Close the file descriptor if it is registered with `DupeNull`, `Close`,
    /// `CloseUnlessSpawnSubspecies`, or `Reopen` actions.
    fn override_and_close(&mut self) -> Result<(), FileDescriptorEntryError> {
        match self.action {
            Action::Close
            | Action::CloseUnlessSpawnSubspecies
            | Action::DupeNull
            | Action::Reopen => match sys::close(self.fd) {
                Ok(_) => Ok(()),
                Err(error) => Err(FileDescriptorEntryError::close_failed(self.fd, error)),
            },
            Action::Ignore => {
                // Nothing to do here
                Ok(())
            }
        }
    }
}

#[allow(missing_docs)]
#[derive(Debug, Error)]
#[error(transparent)]
pub struct FileDescriptorInfoErrorWrapper(#[from] FileDescriptorInfoError);

/// Errors for the [`FileDescriptorRegistry`]
#[allow(missing_docs)]
#[derive(Debug, Error)]
pub enum FdRegistryError {
    /// An audit of the registry failed
    #[error(transparent)]
    AuditFailure(#[from] AuditError),

    /// Failed to open /dev/null
    #[error("Failed to open /dev/null: {0}")]
    DevNullFailure(sys::Error),

    /// Failed to execute entry action
    #[error("Failed to execute entry action: {0}")]
    EntryActionFailure(#[from] FileDescriptorEntryError),

    /// Failed to fetch file descriptor information
    #[error(transparent)]
    FdInfoFailure(#[from] FileDescriptorInfoErrorWrapper),

    /// Attempted to register a fd with an invalid Action variant
    #[error("Attempting to register fd with invalid Action variant: {fd}, {type_str}, {action:?}")]
    InvalidActionForFileType { fd: RawFd, type_str: &'static str, action: Action },

    /// A string is not nul-terminated
    #[error("String is not nul-terminated")]
    InvalidCString,

    /// Reading from procfs failed
    #[error("Error reading /proc/self/fd entries: {0}")]
    ProcFsError(#[from] ProcFsError),

    /// File descriptor is already registered
    #[error("File descriptor {0} is already registered")]
    ReregisteredFd(RawFd),

    /// Unregistered bound socket is not allowlisted
    #[error("Unregistered bound socket is not allowlisted: {fd} -> {address}")]
    UnregisteredBoundSocket { fd: RawFd, address: String },

    /// Unregistered connected socket is not allowlisted
    #[error("Unregistered connected socket is not allowlisted: {fd} -> {address}")]
    UnregisteredConnectedSocket { fd: RawFd, address: String },

    /// Unregistered fifo was found
    #[error("Unregistered FIFO found: {fd}")]
    UnregisteredFifoFd { fd: RawFd },

    /// Unregistered file is not allowlisted
    #[error("Unregistered file is not allowlisted: {fd} -> {path:?}")]
    UnregisteredFile { fd: RawFd, path: CString },

    /// Unregistered signalfd was found
    #[error("Unregistered signalfd found: {fd}")]
    UnregisteredSignalFd { fd: RawFd },

    /// Regular file descriptor is not registered
    #[error("File descriptor is not registered: {0}")]
    UnregisteredFd(RawFd),

    /// File path was not a valid UTF-8 string
    #[error("File path string is not valid UTF-8: {0}")]
    Utf8(#[from] core::str::Utf8Error),
}

impl From<FileDescriptorInfoError> for FdRegistryError {
    fn from(error: FileDescriptorInfoError) -> Self {
        Self::FdInfoFailure(error.into())
    }
}

type FdRegistryResult<T> = Result<T, FdRegistryError>;

/// Errors for the [`FileDescriptorRegistry::audit`] function
#[allow(missing_docs)]
#[derive(Debug, Error)]
enum AuditErrorRepr {
    /// A file descriptor was CLOSED unexpectedly.
    #[error("File descriptor {fd} ({info:?}) was CLOSED unexpectedly.")]
    FileDescriptorAbsent { fd: RawFd, info: Box<FileDescriptorInfo> },

    /// A failure was encountered when fetching file descriptor information.
    #[error(transparent)]
    FileDescriptorInfoFailure(#[from] FileDescriptorInfoError),

    /// A file descriptor was OPENED unexpectedly.
    #[error("File descriptor {fd} ({info:?}) was OPENED unexpectedly.")]
    FileDescriptorUnexpected { fd: RawFd, info: Box<FileDescriptorInfo> },

    /// Error reading /proc/self/fd entries
    #[error("Error reading /proc/self/fd entries: {0}")]
    ProcFsError(#[from] ProcFsError),

    /// Registry ordering is out of sync with the kernel.
    #[error("Registry is out of sync. Expected fd {expected} but found fd {actual}")]
    RegistryOrderMismatch { expected: RawFd, actual: RawFd },

    /// Registry state is out of sync with the kernel.
    #[error(
        "Registry is out of sync: File descriptor {fd} has been REOPENED or MODIFIED. Expected state {expected:?} but found {actual:?}"
    )]
    RegistryStateMismatch {
        fd: RawFd,
        expected: Box<FileDescriptorInfo>,
        actual: Box<FileDescriptorInfo>,
    },
}

/// Errors for the [`FileDescriptorRegistry::audit`] function
#[derive(Debug, Error)]
#[error(transparent)]
pub struct AuditError(#[from] AuditErrorRepr);

type AuditResult<T> = Result<T, AuditError>;

impl From<FileDescriptorInfoError> for AuditError {
    fn from(error: FileDescriptorInfoError) -> Self {
        AuditError(AuditErrorRepr::FileDescriptorInfoFailure(error))
    }
}

impl From<ProcFsError> for AuditError {
    fn from(error: ProcFsError) -> Self {
        AuditError(AuditErrorRepr::ProcFsError(error))
    }
}

/// A struct for associating file descriptor information with [`Action`]s.
/// This can be used to ensure file descriptors are accounted for and handled
/// appropriately when the Zygote forks a new process.
//
// TODO: Either arena-allocate the strings or switch to using ArrayString
#[derive(Debug)]
pub struct FileDescriptorRegistry {
    species: SpeciesRef,
    data: ArrayVec<FileDescriptorEntry, REGISTRY_SIZE>,
    allowed_file_paths: ArrayVec<String, DYNAMIC_ALLOW_LIST_SIZE>,
    allowed_socket_names: ArrayVec<String, DYNAMIC_ALLOW_LIST_SIZE>,
    allowed_socket_paths: ArrayVec<String, DYNAMIC_ALLOW_LIST_SIZE>,
    allowed_peer_socket_paths: ArrayVec<String, DYNAMIC_ALLOW_LIST_SIZE>,
}

#[allow(dead_code)]
impl FileDescriptorRegistry {
    /// Construct a new registry for a given species.  The Species reference
    /// is used to query for species specific allowed files and sockets.
    pub fn new(species: SpeciesRef) -> FdRegistryResult<Self> {
        let mut registry = Self {
            species,
            data: ArrayVec::new(),
            allowed_file_paths: ArrayVec::new(),
            allowed_socket_names: ArrayVec::new(),
            allowed_socket_paths: ArrayVec::new(),
            allowed_peer_socket_paths: ArrayVec::new(),
        };

        // Register the stdio file descriptors
        registry.register(0, Action::Ignore)?;
        registry.register(1, Action::Ignore)?;
        registry.register(2, Action::Ignore)?;

        Ok(registry)
    }

    /// Reset the registry after spawning a subspecies process.
    pub fn reset_for_subspecies(&mut self) -> FdRegistryResult<()> {
        self.execute_actions(ForkType::Subspecies)?;

        self.species.sync_fd_state();
        // This line needs to come after calling [`sys::android::log_close`],
        // which is done in [`sync_fd_state`], so that it will open new file
        // descriptors for logging.
        info!("Resetting FileDescriptorRegistry for subspecies");
        // Register FDs opened by the app's preload routine.
        self.register_new()?;
        self.audit().map_err(FdRegistryError::AuditFailure)
    }

    /// Queries the Zygote and species abstract socket allow lists
    fn bound_abstract_socket_is_allowed(&self, name: &str) -> bool {
        self.allowed_socket_names.contains(&name.to_owned())
            || self.species.bound_abstract_socket_is_allowed(name)
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
    /// descriptors in the registry.  Return [`Err`] if:
    ///   * There are any files open that are not in the registry
    ///   * There are files descriptors in the registry that are no longer open
    ///   * A file descriptor's saved state is not equal to the current state
    #[tracing::instrument(level = "trace", skip_all)]
    pub fn audit(&self) -> AuditResult<()> {
        debug_assert_single_threaded();

        for item in
            self.data.iter().filter(|entry| !entry.closed()).zip_longest(ProcFdIterator::new()?)
        {
            match item {
                EitherOrBoth::Both(entry, Ok(open_fd)) => {
                    if entry.fd != open_fd {
                        return Err(AuditErrorRepr::RegistryOrderMismatch {
                            expected: entry.fd,
                            actual: open_fd,
                        }
                        .into());
                    }
                    let current_info = FileDescriptorInfo::try_from(open_fd)?;
                    if entry.info != current_info {
                        return Err(AuditErrorRepr::RegistryStateMismatch {
                            fd: entry.fd,
                            expected: Box::new(entry.info.clone()),
                            actual: Box::new(current_info.clone()),
                        }
                        .into());
                    }
                }
                EitherOrBoth::Left(entry) => {
                    return Err(AuditErrorRepr::FileDescriptorAbsent {
                        fd: entry.fd,
                        info: Box::new(entry.info.clone()),
                    }
                    .into());
                }
                EitherOrBoth::Right(Ok(open_fd)) => {
                    let info = FileDescriptorInfo::try_from(open_fd)?;
                    return Err(AuditErrorRepr::FileDescriptorUnexpected {
                        fd: open_fd,
                        info: Box::new(info),
                    }
                    .into());
                }
                EitherOrBoth::Both(_, Err(e)) | EitherOrBoth::Right(Err(e)) => return Err(e.into()),
            }
        }

        Ok(())
    }

    /// Queries the Zygote and species bound socket allow lists
    fn bound_socket_path_is_allowed(&self, path: &str) -> bool {
        ALLOWED_SOCKET_PATHS.contains(&path)
            || self.allowed_socket_paths.contains(&path.to_owned())
            || self.species.bound_socket_path_is_allowed(path)
    }

    /// Queries the Zygote and species peer socket path allow lists
    fn peer_socket_path_is_allowed(&self, path: &str) -> bool {
        ALLOWED_PEER_SOCKET_PATHS.contains(&path)
            || self.allowed_peer_socket_paths.contains(&path.to_owned())
            || self.species.peer_socket_path_is_allowed(path)
    }

    /// Iterate through the registry and perform all Close,
    /// CloseUnlessSpawnSubspecies, DupeNull, and Reopen actions. This should be
    /// performed immediately after a fork event.
    #[tracing::instrument(level = "trace", skip(self))]
    pub fn execute_actions(&mut self, fork_type: ForkType) -> FdRegistryResult<()> {
        debug_assert_single_threaded();

        let dev_null_fd = sys::open(DEV_NULL_PATH_C, libc::O_RDWR | libc::O_CLOEXEC, None)
            .map_err(FdRegistryError::DevNullFailure)?;

        for entry in &mut self.data {
            entry.execute(dev_null_fd, fork_type)?;
        }

        self.species.sync_fd_state();
        Ok(())
    }

    /// Queries the Zygote and species allowed files lists.
    fn file_is_allowed(&self, path: &CStr) -> FdRegistryResult<bool> {
        let path_str = path.to_str().map_err(FdRegistryError::Utf8)?;

        Ok(ALLOWED_FILE_PATHS.contains(&path_str)
            || self.allowed_file_paths.iter().any(|allowed_path| allowed_path == path_str)
            || self.species.file_is_allowed(path))
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

    /// Close all file descriptors registered with `DupeNull`, `Close`, and
    /// `Reopen` actions.
    #[tracing::instrument(level = "trace", skip_all)]
    pub fn override_and_close(&mut self) -> FdRegistryResult<()> {
        debug_assert_single_threaded();

        for entry in &mut self.data {
            entry.override_and_close()?;
        }

        self.species.sync_fd_state();
        Ok(())
    }

    /// Adds a file descriptor to the registry and associates it with the
    /// provided action.
    pub fn register(&mut self, fd: RawFd, action: Action) -> FdRegistryResult<()> {
        debug_assert_single_threaded();

        match self.data.binary_search_by(|entry| entry.fd.cmp(&fd)) {
            Ok(entry_index) => {
                if self.data[entry_index].closed() {
                    self.data[entry_index] =
                        FileDescriptorEntry::new(fd, FileDescriptorInfo::try_from(fd)?, action);
                    Ok(())
                } else {
                    Err(FdRegistryError::ReregisteredFd(fd))
                }
            }
            Err(insert_index) => {
                let info = FileDescriptorInfo::try_from(fd)?;
                if action == Action::Reopen && !matches!(info, FileDescriptorInfo::File { .. }) {
                    Err(FdRegistryError::InvalidActionForFileType {
                        fd,
                        type_str: info.type_str(),
                        action,
                    })
                } else {
                    self.data.insert(insert_index, FileDescriptorEntry::new(fd, info, action));
                    Ok(())
                }
            }
        }
    }

    /// Remove the given file descriptor from the registry
    pub fn remove(&mut self, fd: RawFd) -> FdRegistryResult<()> {
        self.data.remove(
            self.data
                .as_slice()
                .binary_search_by(|entry| entry.fd.cmp(&fd))
                .map_err(|_| FdRegistryError::UnregisteredFd(fd))?,
        );

        Ok(())
    }

    /// Iterate through all open file descriptors and register any unregistered
    /// descriptors using a default action.
    #[tracing::instrument(level = "trace", skip_all)]
    pub fn register_new(&mut self) -> FdRegistryResult<()> {
        debug_assert_single_threaded();
        assert!(self.data.is_sorted_by_key(|entry| entry.fd));

        let mut registry_index: usize = 0;
        let num_preexisting_entries = self.data.len();

        for fd_result in ProcFdIterator::new()? {
            let fd = fd_result?;
            if self.is_registered(fd, &mut registry_index, num_preexisting_entries) {
                continue;
            }

            let info = FileDescriptorInfo::try_from(fd)?;
            let action = match info {
                FileDescriptorInfo::BoundSocket(SocketAddress::Abstract(ref name))
                    if self.bound_abstract_socket_is_allowed(name) =>
                {
                    Action::DupeNull
                }
                FileDescriptorInfo::BoundSocket(SocketAddress::Path(ref path))
                    if self.bound_socket_path_is_allowed(path) =>
                {
                    Action::DupeNull
                }
                FileDescriptorInfo::BoundSocket(address) => {
                    return Err(FdRegistryError::UnregisteredBoundSocket {
                        fd,
                        address: address.to_string(),
                    });
                }
                FileDescriptorInfo::ConnectedSocket(SocketAddress::Path(ref path))
                    if self.peer_socket_path_is_allowed(path) =>
                {
                    self.species.get_peer_socket_action(path).unwrap_or(Action::DupeNull)
                }
                FileDescriptorInfo::ConnectedSocket(address) => {
                    return Err(FdRegistryError::UnregisteredConnectedSocket {
                        fd,
                        address: address.to_string(),
                    });
                }
                FileDescriptorInfo::Fifo => {
                    return Err(FdRegistryError::UnregisteredFifoFd { fd });
                }
                FileDescriptorInfo::File { ref path, .. } => {
                    let path_cstr = path.as_cstr().map_err(|_| FdRegistryError::InvalidCString)?;

                    if self.file_is_allowed(path_cstr)? {
                        self.species.get_file_action(path_cstr).unwrap_or(Action::Reopen)
                    } else {
                        return Err(FdRegistryError::UnregisteredFile {
                            fd,
                            path: path_cstr.to_owned(),
                        });
                    }
                }
                FileDescriptorInfo::SignalFd => {
                    return Err(FdRegistryError::UnregisteredSignalFd { fd });
                }
            };

            self.data.push(FileDescriptorEntry::new(fd, info, action));
        }

        if self.data.len() != num_preexisting_entries {
            self.data.sort_by_key(|entry| entry.fd);
        }

        Ok(())
    }

    /// Return the number of registered file descriptors.
    pub fn size(&self) -> usize {
        self.data.len()
    }
}

#[cfg(test)]
mod test {
    use std::{fs::File, os::fd::AsRawFd};

    use zygote_sys::{self as sys, create_abstract_socket, create_bound_socket, AsCStr};

    use super::{FileDescriptorInfo, SocketAddress};
    use crate::test::manage_test;
    use zygote_sys::procfs::get_executable_path;

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
                    FileDescriptorInfo::BoundSocket(SocketAddress::Abstract(name)) if name.to_string() == crate::test::SOCKET_NAME_1));

            // Test BoundSocket
            let bound_socket_fd = create_bound_socket(crate::test::SOCKET_PATH_1, libc::SOCK_DGRAM).unwrap();
            assert!(
                matches!(
                    FileDescriptorInfo::try_from(bound_socket_fd).unwrap(),
                    FileDescriptorInfo::BoundSocket(SocketAddress::Path(path)) if path.to_string() == crate::test::SOCKET_PATH_1));

            sys::close(pipe0).unwrap();
            sys::close(pipe1).unwrap();
            sys::close(abstract_socket_fd).unwrap();
            sys::close(bound_socket_fd).unwrap();
        });
    }
}
