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

//! This module provides safe wrappers around unsafe libc calls.

use core::{
    ffi::{c_char, c_int, c_short, c_void, CStr},
    mem::{self, offset_of},
};
use std::{
    fmt::{self, Debug, Display, Formatter},
    mem::MaybeUninit,
    os::fd::RawFd,
    ptr::NonNull,
};

use anyhow::Result;
use arrayvec::ArrayVec;
use static_assertions::const_assert;
use zerocopy::FromBytes;

#[cfg(target_os = "android")]
pub mod android;
mod libc_fill;
mod process_name;

pub use libc_fill::clone_args;
pub use process_name::set_new_process_name;

/// A platform-dependent type alias for rlimit resources
#[allow(non_camel_case_types)]
#[cfg(all(not(target_os = "android"), target_env = "gnu"))]
pub type rlimit_resource_t = libc::__rlimit_resource_t;

/// A platform-dependent type alias for rlimit resources
#[allow(non_camel_case_types)]
#[cfg(any(target_os = "android", not(target_env = "gnu")))]
pub type rlimit_resource_t = c_int;

/// Platform-dependent type alias for use with [`libc::getpriority`] and
/// [`libc::setpriority`].
#[allow(non_camel_case_types)]
#[cfg(not(any(target_os = "android", target_env = "musl")))]
pub type which_t = core::ffi::c_uint;

/// Platform-dependent type alias for use with [`libc::getpriority`] and
/// [`libc::setpriority`].
#[allow(non_camel_case_types)]
#[cfg(any(target_os = "android", target_env = "musl"))]
pub type which_t = c_int;

/// Number of bytes to allocate for string buffers.  The value 512 was selected
/// because it was large enough to fit all strings that were observed during
/// testing.  Testing should be performed to see if a smaller value can be
/// used.
pub const BUFFER_SIZE_STRINGS: usize = 512;
const_assert!(
    BUFFER_SIZE_STRINGS
        >= std::mem::size_of::<libc::sockaddr_un>() - offset_of!(libc::sockaddr_un, sun_path)
);

// TODO: Consider making this an ArrayVec
/// Buffers used for static string allocations
pub type CStringBuffer = [u8; BUFFER_SIZE_STRINGS];
const BUFFER_INIT_CSTRING: CStringBuffer = [0u8; BUFFER_SIZE_STRINGS];

/// Helper trait for converting types into `CStr`s
pub trait AsCStr {
    /// Create a CStr object referencing this character buffer
    fn as_cstr(&self) -> Result<&CStr>;
}

impl<const N: usize> AsCStr for [u8; N] {
    fn as_cstr(&self) -> Result<&CStr> {
        Ok(CStr::from_bytes_until_nul(self)?)
    }
}

impl AsCStr for &[u8] {
    fn as_cstr(&self) -> Result<&CStr> {
        Ok(CStr::from_bytes_until_nul(self)?)
    }
}

/// Wrapper struct for UNIX-like error numbers
#[derive(Eq, PartialEq)]
pub struct Errno {
    code: c_int,
}

impl Errno {
    /// Test equality with a libc error number
    pub fn is(&self, query_code: c_int) -> bool {
        self.code == query_code
    }

    /// Test if the wrapped code matches any of the provided error numbers
    pub fn matches<const N: usize>(&self, query_codes: &[c_int; N]) -> bool {
        query_codes.contains(&self.code)
    }
}

impl From<Errno> for std::fmt::Error {
    fn from(_: Errno) -> std::fmt::Error {
        std::fmt::Error
    }
}

impl Display for Errno {
    #[cfg(not(any(target_env = "musl", all(target_os = "linux", soong))))]
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), fmt::Error> {
        write!(
            f,
            "Errno ({}): {}",
            error_name(self.code)?.to_str().unwrap(),
            error_description(self.code)?.as_cstr().unwrap().to_str().unwrap()
        )
    }

    #[cfg(any(target_env = "musl", all(target_os = "linux", soong)))]
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), fmt::Error> {
        write!(f, "Errno: {}", error_description(self.code)?.as_cstr().unwrap().to_str().unwrap())
    }
}

impl Debug for Errno {
    #[cfg(not(any(target_env = "musl", all(target_os = "linux", soong))))]
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), fmt::Error> {
        f.debug_struct("Errno")
            .field("code", &self.code)
            .field("name", &error_name(self.code)?)
            .field("description", &error_description(self.code)?.as_cstr())
            .finish()
    }

    #[cfg(any(target_env = "musl", all(target_os = "linux", soong)))]
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), fmt::Error> {
        f.debug_struct("Errno")
            .field("code", &self.code)
            .field("description", &error_description(self.code)?.as_cstr())
            .finish()
    }
}

impl std::error::Error for Errno {}

/// Obtain the location of the `errno` variable from `libc` and dereference it
pub(crate) fn errno() -> Errno {
    Errno {
        #[cfg(not(target_os = "android"))]
        // SAFETY: Reads from the thread's errno address should never fail
        code: unsafe { *libc::__errno_location() },

        #[cfg(target_os = "android")]
        // SAFETY: Reads from the thread's errno address should never fail
        code: unsafe { *libc::__errno() },
    }
}

/// Write 0 to the `errno` `libc` global
fn errno_clear() {
    #[cfg(not(target_os = "android"))]
    // SAFETY: Writes to the thread's errno address should never fail
    unsafe {
        *libc::__errno_location() = 0
    }

    #[cfg(target_os = "android")]
    // SAFETY: Writes to the thread's errno address should never fail
    unsafe {
        *libc::__errno() = 0
    }
}

/// A Result type that uses Errno for the Error type
pub type LibcResult<T> = std::result::Result<T, Errno>;

/// A wrapper struct for [`libc::pollfd`] that ensures error codes are checked
/// before events are handled.
#[repr(transparent)]
pub struct PollFd(libc::pollfd);

impl PollFd {
    /// Initialize a new [`libc::pollfd`] wrapper struct with the provided file
    /// descriptor and events.
    pub fn new(fd: RawFd, events: c_short) -> Self {
        Self(libc::pollfd { fd, events, revents: 0 })
    }

    /// Returns `Err` in the presence of errors, else `Some(PollFdChecked)`.
    pub fn check(&self) -> Result<PollFdChecked<'_>, (RawFd, c_short)> {
        const ERROR_MASK: c_short = libc::POLLERR | libc::POLLNVAL;
        if self.0.revents & ERROR_MASK != 0 {
            Err((self.0.fd, self.0.revents & ERROR_MASK))
        } else {
            Ok(PollFdChecked(&self.0))
        }
    }
}

/// A struct used to wrap a [`libc::pollfd`] struct that has been checked for
/// errors.
#[repr(transparent)]
pub struct PollFdChecked<'a>(&'a libc::pollfd);

impl PollFdChecked<'_> {
    /// If the specified event occurred the result of calling the handler will
    /// be returned; otherwise, None.
    pub fn handle_event<T>(
        &self,
        event: c_short,
        mut handler: impl FnMut(RawFd) -> T,
    ) -> Option<T> {
        if self.0.revents & event == event {
            Some(handler(self.0.fd))
        } else {
            None
        }
    }
}

// TODO: Consider adding [`closedir`] as a destructor.
/// Wrapper class for a `libc::DIR` pointer
pub struct LibcDir {
    inner: NonNull<libc::DIR>,
}

/// Helper functions for LibcDir.
impl LibcDir {
    /// Module-private constructor for [`LibcDir`]
    fn from_raw(inner: NonNull<libc::DIR>) -> Self {
        Self { inner }
    }
}

/// Types for which any bit pattern is valid
///
/// # Safety
/// Any type this is implemented for must accept all bit patterns.
pub unsafe trait LibcFromBytes {}

macro_rules! from_bytes_c {
    ($($t:ty),*) => {
        $(
        // SAFETY: This is a C-origin type, all values are legal.
        unsafe impl LibcFromBytes for $t {}
        )*
    }
}

from_bytes_c!(libc::ucred, libc::signalfd_siginfo);

// SAFETY: Passing raw byte arrays to `libc` calls is safe.  This leaves the
//         safety of casting to and from bytes up to the caller.
unsafe impl<const N: usize> LibcFromBytes for [u8; N] {}

/// Convert an integer return value from a `libc` call into a [`LibcResult`]
/// type.  If the return value is -1 the Error type will contain the resulting
/// `errno` value.
pub fn libc_result_from_int<T: Eq + From<i8>>(retval: T) -> LibcResult<T> {
    if retval == T::from(-1) {
        Err(errno())
    } else {
        Ok(retval)
    }
}

/// Test an integer `libc` return value and returns the auxiliary value if
/// is not equal to -1 and the `errno` value if it is.  The payload thunk is
/// only evaluated when `retval` does not indicate an error.
pub fn libc_result_from_int_with_payload<T: Eq + From<i8>, P>(
    retval: T,
    payload: impl FnOnce() -> P,
) -> LibcResult<P> {
    if retval == T::from(-1) {
        Err(errno())
    } else {
        Ok(payload())
    }
}

/// Test an integer `libc` return value and return `void` if it is not equal to
/// -1 and the `errno` value if it is.
pub fn libc_result_from_int_with_void<T: Eq + From<i8>>(retval: T) -> LibcResult<()> {
    if retval == T::from(-1) {
        Err(errno())
    } else {
        Ok(())
    }
}

/// Converts a pointer into a [`LibcResult`] type, returning an [`Errno`] if it
/// is `null` and converting it into the desired return type if it isn't.
#[cfg(not(any(target_env = "musl", all(target_os = "linux", soong))))]
fn libc_result_from_ptr<T, U, C: Fn(NonNull<T>) -> U>(
    retval: *const T,
    constructor: C,
) -> LibcResult<U> {
    NonNull::new(retval as *mut T).map(constructor).ok_or(errno())
}

/// Converts a pointer into a [`LibcResult`] type, returning an [`Errno`] if it
/// is `null` and converting it into the desired return type if it isn't.
fn libc_result_from_mut_ptr<T, U, C: Fn(NonNull<T>) -> U>(
    retval: *mut T,
    constructor: C,
) -> LibcResult<U> {
    NonNull::new(retval).map(constructor).ok_or(errno())
}

/*
 * Libc helpers
 */

/// A macro for retrying libc calls that result in an EINTR errno.
macro_rules! retry_eintr {
    ($libc_call:expr) => {
        loop {
            match $libc_call {
                Err(errno) if errno.is(libc::EINTR) => {
                    continue;
                }
                result => {
                    break result;
                }
            }
        }
    };
}

/// Construct an abstract socket name inside a [`libc::sockaddr_un`] struct
/// from the provided name and family.
pub fn abstract_socket_address(name: &str, family: libc::sa_family_t) -> libc::sockaddr_un {
    let mut socket_addr = libc::sockaddr_un { sun_family: family, sun_path: [0; 108] };
    let name_view = &name.as_bytes()[0..std::cmp::min(name.len(), socket_addr.sun_path.len() - 1)];

    // Abstract socket names begin with a null character, so start copying the
    // name at index 1.
    socket_addr.sun_path[1..name_view.len() + 1]
        .copy_from_slice(<[c_char]>::ref_from_bytes(name_view).unwrap());

    socket_addr
}

/// Construct a bound socket name inside a [`libc::sockaddr_un`] struct from
/// the provided name and family.
pub fn bound_socket_address(path: &str, family: libc::sa_family_t) -> libc::sockaddr_un {
    let mut socket_addr = libc::sockaddr_un { sun_family: family, sun_path: [0; 108] };
    let name_view = &path.as_bytes()[0..std::cmp::min(path.len(), socket_addr.sun_path.len() - 1)];

    socket_addr.sun_path[0..name_view.len()]
        .copy_from_slice(<[c_char]>::ref_from_bytes(name_view).unwrap());

    socket_addr
}

/// Construct a [`libc::sigset_t`] containing the provided signals.
pub fn build_sigset(signals: &[c_int]) -> LibcResult<libc::sigset_t> {
    let mut sigset = sigemptyset()?;

    for signum in signals {
        sigaddset(&mut sigset, *signum)?;
    }

    Ok(sigset)
}

/// Type for specifying control flow in [`call_until_would_block`]
pub enum LoopControl<T> {
    /// Tell [`call_until_would_block`] to execute the action again.
    Continue,
    /// Tell [`call_until_would_block`] to exit the loop and return the
    /// provided value.
    Break(T),
}

/// Type for indicating the exit conditions for [`call_until_would_block`]
pub enum LoopExit<T> {
    /// Indicate that the loop exited early with the provided value
    Early(T),
    /// Indicate that the loop exited because the action would have blocked.
    WouldBlock,
}

impl<T> LoopExit<T> {
    /// Return true if `self` is [`LoopExit::Early`].
    pub fn exited_early(&self) -> bool {
        match self {
            Self::WouldBlock => false,
            Self::Early(_) => true,
        }
    }

    /// Return true if `self` is [`Loop::WouldBLock`]
    pub fn would_block(&self) -> bool {
        match self {
            Self::WouldBlock => true,
            Self::Early(_) => false,
        }
    }
}

/// This function calls the `task()` thunk and checks the LibcResult.  If the
/// thunk exited with either `EAGAIN` or `EWOULDBLOCK` the function will
/// immediately return a [`LoopExit::WouldBlock`] value.  If any other error
/// was returned by the thunk it will be returned by this function.  If the
/// thunk completed successfully the handler will be called and, based on the
/// return value, the function will either return or continue.  If the function
/// returns early the [`LoopStatus::Break`] value will be forwarded in a
/// [`LoopExit::Early`] variant.
pub fn call_until_would_block<T, U>(
    task: impl Fn() -> LibcResult<T>,
    mut handler: impl FnMut(T) -> LoopControl<U>,
) -> LibcResult<LoopExit<U>> {
    loop {
        match task() {
            Ok(result) => match handler(result) {
                LoopControl::Continue => continue,
                LoopControl::Break(val) => return Ok(LoopExit::Early(val)),
            },
            Err(errno) if errno.is(libc::EAGAIN) || errno.is(libc::EWOULDBLOCK) => {
                // Task would block
                return Ok(LoopExit::WouldBlock);
            }
            Err(errno) => {
                return Err(errno);
            }
        }
    }
}

/// Create a new UNIX domain datagram abstract socket with the provided name
pub fn create_abstract_socket(name: &str, protocol: c_int) -> Result<RawFd> {
    let socket_fd = socket(libc::AF_UNIX, protocol, 0)?;
    let socket_addr = abstract_socket_address(name, libc::AF_UNIX as libc::sa_family_t);

    bind(socket_fd, &socket_addr)?;

    Ok(socket_fd)
}

/// Create a new UNIX domain datagram socket and bind it to the provided file
/// system path
pub fn create_bound_socket(path: &str, protocol: c_int) -> Result<RawFd> {
    let socket_fd = socket(libc::AF_UNIX, protocol, 0)?;
    let socket_addr = bound_socket_address(path, libc::AF_UNIX as libc::sa_family_t);

    bind(socket_fd, &socket_addr)?;

    Ok(socket_fd)
}

/// Select the file-type bits from the `mode` value
pub fn get_file_type(stat: libc::stat) -> libc::mode_t {
    (stat.st_mode as libc::mode_t) & libc::S_IFMT
}

/// Get the PID, UID, and GID for the remote end of a UNIX domain socket.
pub fn get_socket_creds(fd: RawFd) -> LibcResult<libc::ucred> {
    getsockopt(fd, libc::SOL_SOCKET, libc::SO_PEERCRED)
}

/// Attempt to read `size_of::<T>()` bytes.  If the correct number of bytes
/// were read the function returns `Ok(T)`, otherwise an `Err(Errno)` is
/// returned with the `code` set to 0.
pub fn read_exact<T: LibcFromBytes>(fd: RawFd) -> LibcResult<T> {
    read::<T>(fd).and_then(|(read_len, result)| {
        if read_len == std::mem::size_of::<T>() as isize {
            Ok(result)
        } else {
            Err(Errno { code: 0 })
        }
    })
}

/// An RAII object that sets and resets the effective permissions of the
/// calling process
pub struct EffectiveIdContext {
    ruid: libc::uid_t,
    rgid: libc::gid_t,

    euid: Option<libc::uid_t>,
    egid: Option<libc::gid_t>,
}

impl EffectiveIdContext {
    /// Set the effective permissions of the calling process
    pub fn enter(
        euid: Option<libc::uid_t>,
        egid: Option<libc::gid_t>,
    ) -> Result<EffectiveIdContext> {
        if let Some(euid) = euid {
            // Pass the bit pattern of twos-complement `-1` as the real UID to
            // leave it unchanged.
            setreuid(-1i32 as libc::uid_t, euid)?;
        }

        if let Some(egid) = egid {
            // Pass the bit pattern of twos-complement `-1` as the real GID to
            // leave it unchanged.
            setregid(-1i32 as libc::gid_t, egid)?;
        }

        Ok(EffectiveIdContext { ruid: getuid(), rgid: getgid(), euid, egid })
    }

    /// Restore the effective permissions of the calling process
    pub fn exit(&self) -> Result<()> {
        if self.euid.is_some() {
            // Pass the bit pattern of twos-complement `-1` as the real UID to
            // leave it unchanged.
            setreuid(-1i32 as libc::uid_t, self.ruid)?;
        }

        if self.egid.is_some() {
            // Pass the bit pattern of twos-complement `-1` as the real GID to
            // leave it unchanged.
            setregid(-1i32 as libc::gid_t, self.rgid)?;
        }

        Ok(())
    }
}

impl Drop for EffectiveIdContext {
    fn drop(&mut self) {
        self.exit().unwrap()
    }
}

/*
 * Libc wrappers
 */

/// A safe wrapper around [`libc::accept`].
///
/// Because we are only interested in UNIX domain sockets, which have no useful
/// peer address information, we do not pass in a [`libc::sockaddr`] buffer.
///
/// See: `man accept`
pub fn accept(fd: RawFd) -> LibcResult<RawFd> {
    // SAFETY: If the file descriptor is invalid `accept()` will return -1 and
    //         we will extract errno and wrap it in a LibcResult.
    libc_result_from_int(unsafe { libc::accept(fd, std::ptr::null_mut(), std::ptr::null_mut()) })
}

/// A safe wrapper around [`libc::bind`].
///
/// See: `man bind`
pub fn bind<SockAddrType>(fd: RawFd, sockaddr: &SockAddrType) -> LibcResult<()> {
    // SAFETY: The pointer argument to `libc::bind` is guaranteed to reference
    //         allocated memory and the return value is checked and wrapped in
    //         a LibcResult.
    libc_result_from_int_with_void(unsafe {
        libc::bind(
            fd,
            (sockaddr as *const SockAddrType) as *const libc::sockaddr,
            std::mem::size_of::<SockAddrType>() as libc::socklen_t,
        )
    })
}

/// An unsafe wrapper around the `clone3` system call
///
/// See: `man clone`
///
/// # Safety
/// The safety requirements for `clone3` are complex, with multiple sets of
/// mutually-incompatible arguments.  The full description of the safety
/// requirements of this call are listed in the manual pages.  Any non-trivial
/// invocation of `clone3` will involve lengthy justifications for all
/// arguments.
pub unsafe fn clone3(args: &clone_args) -> LibcResult<libc::pid_t> {
    // SAFETY: The pointer argument is derived from a valid reference and the
    //         result value is checked and wrapped in a LibcResult.
    libc_result_from_int(unsafe { libc_fill::clone3(args) as libc::pid_t })
}

/// A safe wrapper around [`libc::close`].
///
/// The [`libc::EINTR`] signal is handled internally using the [`retry_eintr!`]
/// macro.
///
/// See: `man close`
pub fn close(fd: RawFd) -> LibcResult<c_int> {
    // SAFETY: If the file descriptor is invalid `close()` will return -1 and
    //         we will extract errno and wrap it in a LibcResult.
    retry_eintr!(libc_result_from_int(unsafe { libc::close(fd) }))
}

/// A safe wrapper around [`libc::closedir`].
///
/// See: `man closedir`
pub fn closedir(dir: LibcDir) -> LibcResult<()> {
    // SAFETY: The LibcDir argument can only be constructed by the `opendir`
    //         function which also checks to ensure that the pointer is
    //         non-null.  If the LibcDir object *does* contain an invalid
    //         pointer then `libc` will return `-1` and a LibcResult::Err value
    //         will be returned wrapping `errno`.
    libc_result_from_int_with_void(unsafe { libc::closedir(dir.inner.as_ptr()) })
}

/// A safe wrapper around [`libc::connect`].
///
/// See: `man connect`
pub fn connect<SockAddrType>(fd: RawFd, sockaddr: &SockAddrType) -> LibcResult<()> {
    // SAFETY: The pointer argument to `libc::connect` is guaranteed to reference
    //         allocated memory and the return value is checked and wrapped in
    //         a LibcResult.
    libc_result_from_int_with_void(unsafe {
        libc::connect(
            fd,
            (sockaddr as *const SockAddrType) as *const libc::sockaddr,
            std::mem::size_of::<SockAddrType>() as libc::socklen_t,
        )
    })
}

/// A safe wrapper around [`libc::dirfd`].
///
/// See: `man dirfd`
pub fn dirfd(dir: &LibcDir) -> LibcResult<RawFd> {
    // SAFETY: The LibcDir argument can only be constructed by the `opendir`
    //         function which also checks to ensure that the pointer is
    //         non-null.  If the LibcDir object *does* contain an invalid
    //         pointer then `libc` will return `-1` and a LibcResult::Err value
    //         will be returned wrapping `errno`.
    libc_result_from_int(unsafe { libc::dirfd(dir.inner.as_ptr()) })
}

/// A safe wrapper around [`libc::dup3`].
///
/// See: `man dup3`
pub fn dup3(old_fd: RawFd, new_fd: RawFd, flags: c_int) -> LibcResult<()> {
    #[cfg(not(target_os = "android"))]
    // SAFETY: Invalid arguments will result in an error code being returned
    //         that will be wrapped by a LibcResult.
    return retry_eintr!(libc_result_from_int_with_void(unsafe {
        libc::dup3(old_fd, new_fd, flags)
    }));

    #[cfg(target_os = "android")]
    // SAFETY: Invalid arguments will result in an error code being returned
    //         that will be wrapped by a LibcResult.
    return retry_eintr!(libc_result_from_int_with_void(unsafe {
        libc_fill::dup3(old_fd, new_fd, flags)
    }));
}

/// A safe wrapper around [`libc::strerror_r`].
///
/// This function uses `strerror_r` because `strerror` is not thread-safe.
///
/// See: `man strerror`
pub fn error_description(code: c_int) -> LibcResult<CStringBuffer> {
    let mut buffer: CStringBuffer = BUFFER_INIT_CSTRING;

    // SAFETY: The pointer argument refers to memory allocated in this function.
    let retval = unsafe { libc::strerror_r(code, buffer.as_mut_ptr().cast(), buffer.len()) };

    if retval == 0 {
        Ok(buffer)
    } else {
        Err(Errno { code: retval })
    }
}

/// A safe wrapper around [`libc_fill::strerrorname_np`]
///
/// See: `man strerror`
#[cfg(not(any(target_env = "musl", all(target_os = "linux", soong))))]
pub fn error_name(code: c_int) -> LibcResult<&'static CStr> {
    // SAFETY: This function takes no pointers and the return result is checked
    //         and wrapped in a LibcResult.  The returned pointers point to
    //         statically allocated null-terminated strings and are safe to
    //         cast to `CStr`s.
    unsafe {
        libc_result_from_ptr(libc_fill::strerrorname_np(code), |non_null_ptr: NonNull<c_char>| {
            CStr::from_ptr(non_null_ptr.as_ptr() as *const c_char)
        })
    }
}

/// A safe wrapper around [`libc::fcntl`], passing [`libc::F_GETFD`] as the
/// `op`.
///
/// See: `man fcntl`
pub fn fcntl_getfd(fd: RawFd) -> LibcResult<c_int> {
    // SAFETY: An invalid file descriptor will result in an error code being
    //         returned that will be wrapped by a LibcResult.
    libc_result_from_int(unsafe { libc::fcntl(fd, libc::F_GETFD) })
}

/// A safe wrapper around [`libc::fcntl`], passing [`libc::F_GETFL`] as the
/// `op`.
///
/// See: `man fcntl`
pub fn fcntl_getfl(fd: RawFd) -> LibcResult<c_int> {
    // SAFETY: An invalid file descriptor will result in an error code being
    //         returned that will be wrapped by a LibcResult.
    libc_result_from_int(unsafe { libc::fcntl(fd, libc::F_GETFL) })
}

/// A safe wrapper around [`libc::fcntl`], passing [`libc::F_SETFD`] as the
/// `op`.
///
/// See: `man fcntl`
pub fn fcntl_setfd(fd: RawFd, flags: c_int) -> LibcResult<()> {
    // SAFETY: Invalid arguments will result in an error code being returned
    //         that will be wrapped by a LibcResult.
    libc_result_from_int_with_void(unsafe { libc::fcntl(fd, libc::F_SETFD, flags) })
}

/// A safe wrapper around [`libc::fcntl`], passing [`libc::F_SETFL`] as the
/// `op`.
///
/// See: `man fcntl`
pub fn fcntl_setfl(fd: RawFd, flags: c_int) -> LibcResult<()> {
    // SAFETY: Invalid arguments will result in an error code being returned
    //         that will be wrapped by a LibcResult.
    libc_result_from_int_with_void(unsafe { libc::fcntl(fd, libc::F_SETFL, flags) })
}

/// A safe wrapper around [`libc::fork`].
///
/// # Safety
/// This function results in undefined behavior if it is called from a process
/// with multiple threads.  The calling process *MUST* be single threaded for
/// calls to this function to be safe.
///
/// See: `man fork`
pub unsafe fn fork() -> LibcResult<libc::pid_t> {
    // SAFETY: The `libc::fork` function takes no pointers and the return
    //         value is checked and wrapped in a LibcResult.
    libc_result_from_int(unsafe { libc::fork() })
}

/// A safe wrapper around [`libc::fstat`].
///
/// See: `man fstat`
pub fn fstat(fd: RawFd) -> LibcResult<libc::stat> {
    let mut buffer = MaybeUninit::<libc::stat>::uninit();

    libc_result_from_int_with_payload(
        // SAFETY: The stat buffer pointer is guaranteed to point to a valid
        //         memory address due to its allocation inside this function's
        //         stack.  The return value is checked and wrapped in a
        //         LibcResult.
        unsafe { libc::fstat(fd, buffer.as_mut_ptr().cast()) },
        // SAFETY: The `libc::fstat` function fully initializes the stat
        //         struct.
        || unsafe { buffer.assume_init() },
    )
}

/// A safe wrapper around [`libc::getegid`].
///
/// See: `man getegid`
pub fn getegid() -> libc::gid_t {
    // SAFETY: The `libc::getegid` function can not fail.
    unsafe { libc::getegid() }
}

/// A safe wrapper around [`libc::geteuid`].
///
/// See: `man geteuid`
pub fn geteuid() -> libc::uid_t {
    // SAFETY: The `libc::geteuid` function can not fail.
    unsafe { libc::geteuid() }
}

/// A safe wrapper around [`libc::getpriority`]
///
/// See: `man getpriority`
pub fn getpriority(which: which_t, who: libc::id_t) -> LibcResult<c_int> {
    errno_clear();

    // SAFETY: `errno` is cleared before calling the function and checked upon
    //         return.
    let result: c_int = unsafe { libc::getpriority(which, who) };
    if errno().code == 0 {
        Ok(result)
    } else {
        Err(errno())
    }
}

/// A safe wrapper around [`libc::getgid`].
///
/// See: `man getgid`
pub fn getgid() -> libc::gid_t {
    // SAFETY: The `libc::getgid` function can not fail.
    unsafe { libc::getgid() }
}

/// A safe wrapper around [`libc::getgroups`]
///
/// See: `man getgroups`
pub fn getgroups<const N: usize>() -> LibcResult<ArrayVec<libc::gid_t, N>> {
    let mut buffer = [0; N];

    // SAFETY: The pointer argument refers to a stack-allocated buffer
    //         parameterized by N.  The result value is checked and wrapped in
    //         LibcResult.
    let result = unsafe { libc::getgroups(N as c_int, buffer.as_mut_ptr()) };

    match result {
        -1 => Err(errno()),
        n => Ok(ArrayVec::from_iter(buffer.into_iter().take(n as usize))),
    }
}

/// A safe wrapper around [`libc::getpid`].
///
/// See: `man getpid`
pub fn getpid() -> libc::pid_t {
    // SAFETY: The `libc::getpid` function can not fail.
    unsafe { libc::getpid() }
}

/// A safe wrapper around [`libc::getrlimit`]
///
/// See: `man getrlimit`
pub fn getrlimit(resource: rlimit_resource_t) -> LibcResult<libc::rlimit> {
    let mut rlimit = libc::rlimit { rlim_cur: 0, rlim_max: 0 };

    // SAFETY: The pointer argument is derived from a local stack reference and
    //         the return value is checked and wrapped in a LibcResult.
    libc_result_from_int_with_payload(unsafe { libc::getrlimit(resource, &mut rlimit) }, || rlimit)
}

/// A safe wrapper around [`libc::getsockopt`].
///
/// See: `man getsockopt`
pub fn getsockopt<T: LibcFromBytes>(fd: RawFd, level: c_int, optname: c_int) -> LibcResult<T> {
    let mut optval = MaybeUninit::<T>::zeroed();
    let mut optlen = std::mem::size_of::<T>() as libc::socklen_t;

    libc_result_from_int_with_payload(
        // SAFETY: The `optval` and `optlen` arguments are valid pointers to
        //         memory allocated in this function.  The return value is
        //         checked and wrapped in a LibcResult.
        unsafe {
            libc::getsockopt(
                fd,
                level,
                optname,
                optval.as_mut_ptr() as *mut libc::c_void,
                &mut optlen,
            )
        },
        // SAFETY: The call to `libc::getsockopt` will initialize this region.
        || unsafe { optval.assume_init() },
    )
}

/// A safe wrapper around [`libc::getsockname`].
///
/// An Errno error with code `0` will be returned if the socket family is not
/// `AF_UNIX`.
///
/// See: `man getsockname`
pub fn getsockname(fd: RawFd) -> LibcResult<(libc::sockaddr_un, usize)> {
    let mut addr = std::mem::MaybeUninit::<libc::sockaddr_un>::zeroed();
    let mut addr_len = std::mem::size_of::<libc::sockaddr_un>() as libc::socklen_t;

    // SAFETY: The address buffer pointer is guaranteed to reference valid
    //         memory that has been zeroed out.  This ensures that the any
    //         strings contained in the buffer will be valid null-terminated
    //         C strings.  The return value is checked and wrapped in a
    //         LibcResult.
    libc_result_from_int(unsafe {
        libc::getsockname(fd, addr.as_mut_ptr().cast(), &mut addr_len)
    })?;

    debug_assert!(addr_len <= std::mem::size_of::<libc::sockaddr_un>() as libc::socklen_t);

    if addr_len as usize == std::mem::size_of::<libc::sa_family_t>() {
        return Err(Errno { code: 0 });
    }

    // SAFETY: The memory had been zero initialized before `libc::getsockname`
    //         filled in any relevant data.  All strings should be valid C
    //         strings.
    let addr = unsafe { addr.assume_init() };

    if addr.sun_family != libc::AF_UNIX as u16 {
        return Err(Errno { code: 0 });
    }

    let path_len = addr_len as usize - offset_of!(libc::sockaddr_un, sun_path);

    Ok((addr, path_len))
}

/// A safe wrapper around [`libc::getuid`].
///
/// See: `man getuid`
pub fn getuid() -> libc::uid_t {
    // SAFETY: The `libc::getuid` function can not fail.
    unsafe { libc::getuid() }
}

/// A safe wrapper around [`libc::listen`].
///
/// See: `man listen`
pub fn listen(fd: RawFd, backlog: c_int) -> LibcResult<()> {
    // SAFETY: The `libc::listen` function takes no pointers and the return
    //         value is checked and wrapped in a LibcResult.
    libc_result_from_int_with_void(unsafe { libc::listen(fd, backlog) })
}

/// A safe wrapper around [`libc::lseek64`].
///
/// See: `man lseek64`
pub fn lseek64(fd: RawFd, offset: libc::off64_t, whence: c_int) -> LibcResult<libc::off64_t> {
    // SAFETY: The `libc::lseek64` function takes no pointers and the return
    //         value is checked and wrapped in a LibcResult.
    libc_result_from_int(unsafe { libc::lseek64(fd, offset, whence) })
}

/// A safe wrapper around [`libc::mallopt`].
///
/// See: `man mallopt`
#[cfg(not(target_env = "musl"))]
pub fn mallopt(cmd: c_int, arg: c_int) -> LibcResult<()> {
    // SAFETY: This function takes no pointer arguments and the return value
    //         is checked and wrapped in a LibcResult.
    let retval = unsafe { libc::mallopt(cmd, arg) };

    // This function returns 1 on success and 0 on error.
    if retval == 0 {
        Err(errno())
    } else {
        Ok(())
    }
}

/// A safe wrapper around [`libc::open`].
///
/// See: `man open`
pub fn open(path: &CStr, flags: c_int) -> LibcResult<RawFd> {
    // SAFETY: Path points to a valid, null-terminated C string and the return
    //         value is checked and wrapped in a LibcResult.
    retry_eintr!(libc_result_from_int(unsafe { libc::open(path.as_ptr().cast(), flags) }))
}

/// A safe wrapper around [`libc::opendir`].
///
/// See: `man opendir`
pub fn opendir(path: &CStr) -> LibcResult<LibcDir> {
    // SAFETY: Path points to a valid, null-terminated C string and the return
    //         value is checked and wrapped in a LibcResult.  The pointer
    //         itself has been verified as non-null and is thus wrapped in a
    //         LibcResult.
    libc_result_from_mut_ptr(unsafe { libc::opendir(path.as_ptr()) }, LibcDir::from_raw)
}

/// A safe wrapper around [`libc::pipe`].
///
/// See: `man pipe`
pub fn pipe() -> LibcResult<(RawFd, RawFd)> {
    let mut pipes = [0; 2];
    libc_result_from_int_with_payload(
        // SAFETY: The file descriptor buffer is guaranteed to point to valid
        //         memory and the return value is checked before the resulting
        //         pipes are wrapped in a LibcResult.
        unsafe { libc::pipe(pipes.as_mut_ptr()) },
        || (pipes[0], pipes[1]),
    )
}

/// A safe wrapper around [`libc::poll`].
///
/// See `man poll`
pub fn poll(pollfds: &mut [PollFd], timeout: c_int) -> LibcResult<c_int> {
    // SAFETY: The `pollfds` pointer is valid because it is derived from a
    //         a valid slice reference and the length argument is obtained from
    //         the provided slice.  The return value is checked and wrapped in
    //         a LibcResult.
    //
    //         The cast between `*mut PollFd` and `*mut libc::pollfd` is safe
    //         due to the use of `#[repr(transparent)]` on PollFd.
    libc_result_from_int(unsafe {
        libc::poll(
            pollfds.as_mut_ptr() as *mut libc::pollfd,
            pollfds.len() as libc::nfds_t,
            timeout,
        )
    })
}

/// A safe wrapper around the [`libc::prctl`] `PR_SET_NAME` operation.
///
/// See: `man PR_SET_NAME`
pub fn prctl_set_name<S: AsRef<[u8]> + ?Sized>(name: &S) {
    let mut name_buffer = [0u8; 16];
    let copy_len = std::cmp::min(name.as_ref().len(), name_buffer.len() - 1);
    name_buffer[..copy_len].copy_from_slice(&name.as_ref()[..copy_len]);

    // SAFETY: The only error that can be returned by the `libc::prctl`
    //         `PR_SET_NAME` operation is `EINVAL`.  This error can not occur
    //         for this operation as the pointer argument refers to a 16-byte
    //         buffer allocated in this stack frame.
    unsafe { libc::prctl(libc::PR_SET_NAME, name_buffer.as_ptr() as *const c_void) };
}

/// A safe wrapper around the [`libc::prctl`] `PR_GET_SECUREBITS` operation.
///
/// See: `man prctl`
/// See: `man capabilities`
pub fn prctl_get_securebits() -> LibcResult<c_int> {
    // SAFETY: The `libc::prctl` `PR_GET_SECUREBITS` operation takes no pointer
    //         arguments and can only return `EINVAL` if the argument is
    //         invalid.  The argument is guaranteed to be valid in this case.
    libc_result_from_int(unsafe { libc::prctl(libc::PR_GET_SECUREBITS) })
}

/// A safe wrapper around the [`libc::prctl`] `PR_SET_SECUREBITS` operation.
///
/// See: `man prctl`
/// See: `man capabilities`
pub fn prctl_set_securebits(securebits: c_int) -> LibcResult<()> {
    // SAFETY: The `libc::prctl` `PR_SET_SECUREBITS` operation takes an integer
    //         argument and can only return `EINVAL` if the argument is
    //         invalid.  The argument is guaranteed to be valid in this case.
    libc_result_from_int_with_void(unsafe { libc::prctl(libc::PR_SET_SECUREBITS, securebits) })
}

/// A safe wrapper around [`libc::read`].
///
/// See: `man read`
pub fn read<T: LibcFromBytes>(fd: RawFd) -> LibcResult<(isize, T)> {
    let mut buffer = MaybeUninit::<T>::zeroed();

    // SAFETY: The `buff` pointer is valid because it is derived from a value
    //         allocated in this function and the `count` argument is
    //         calculated using `size_of()`.  The return value is checked and
    //         wrapped in a LibcResult.
    retry_eintr!(libc_result_from_int(unsafe {
        libc::read(fd, buffer.as_mut_ptr() as *mut c_void, std::mem::size_of::<T>())
    }))
    // SAFETY: The buffer was zero-initialized when it was allocated.  The call
    //         to `libc::read` will have rewritten the first `retval` bytes of
    //         the buffer.
    .and_then(|retval| Ok((retval, unsafe { buffer.assume_init() })))
}

/// A safe wrapper around [`libc::readdir`].
///
/// See: `man readdir`
pub fn readdir(dir: &LibcDir) -> Option<NonNull<libc::dirent>> {
    // SAFETY: The pointer to the directory is guaranteed to be NonNull and the
    //         return value is checked and wrapped in a LibcResult.  The
    //         pointer itself has been verified as non-null and is thus wrapped
    //         in a LibcResult.
    libc_result_from_mut_ptr(unsafe { libc::readdir(dir.inner.as_ptr()) }, std::convert::identity)
        .ok()
}

/// A safe wrapper around [`libc::readlink`].
///
/// See: `man readlink`
pub fn readlink(path_name: &CStr) -> LibcResult<CStringBuffer> {
    let mut link_buffer: CStringBuffer = BUFFER_INIT_CSTRING;

    libc_result_from_int_with_payload(
        // SAFETY: The path name is guaranteed to be a valid null-terminated C
        //         string.  The link buffer is guaranteed to be a
        //         zero-initialized buffer and the size_of function is used to
        //         provide the buffer length to the libc call.  The return
        //         value is checked and wrapped in a LibcResult.
        unsafe {
            libc::readlink(
                path_name.as_ptr(),
                link_buffer.as_mut_ptr().cast(),
                std::mem::size_of::<CStringBuffer>(),
            )
        },
        || link_buffer,
    )
}

/// A limited wrapper around [`libc::recvmsg`].
///
/// This wrapper only provides functionality for receiving a single buffer from
/// a UNIX-domain datagram session socket.
///
/// The [`libc::EINTR`] signal is handled internally using the [`retry_eintr!`]
/// macro.
///
/// See `man recvmsg` and `man readv`
pub fn recvmsg<const BUFFER_SIZE: usize>(fd: RawFd) -> LibcResult<(isize, [u8; BUFFER_SIZE])> {
    let mut buffer = [0; BUFFER_SIZE];

    // Usage of `mem::zeroed()` is required as implementations of `msghdr`
    // can include private fields (e.g. aarch64-musl).
    //
    // SAFETY: The man page for recvmsg specifies that it is valid to zero-
    //         initialize a msghdr struct.
    let mut msghdr: libc::msghdr = unsafe { mem::zeroed() };

    let mut io_vec =
        libc::iovec { iov_base: buffer.as_mut_ptr() as *mut c_void, iov_len: buffer.len() };

    msghdr.msg_iov = &mut io_vec;
    msghdr.msg_iovlen = 1;

    // SAFETY: All of the pointers passed to `libc::recvmsg` point to regions
    //         allocated inside this function.  The return value is checked and
    //         wrapped in a LibcResult.
    retry_eintr!(libc_result_from_int(unsafe { libc::recvmsg(fd, &mut msghdr, 0) }))
        .and_then(|retval| Ok((retval, buffer)))
}

/// A limited wrapper around [`libc::sendmsg`].
///
/// This wrapper only provides functionality for sending a single buffer over a
/// UNIX-domain datagram session socket.
///
/// The [`libc::EINTR`] signal is handled internally using the [`retry_eintr!`]
/// macro.
///
/// See: `man sendmsg`
pub fn sendmsg(socket_fd: RawFd, buffer: &[u8]) -> LibcResult<isize> {
    // Usage of `mem::zeroed()` is required as implementations of `msghdr`
    // can include private fields (e.g. aarch64-musl).
    //
    // SAFETY: The man page for sendmsg specifies that it is valid to zero-
    //         initialize a msghdr struct.
    let mut msghdr: libc::msghdr = unsafe { mem::zeroed() };

    let mut io_vec =
        libc::iovec { iov_base: buffer.as_ptr() as *mut c_void, iov_len: buffer.len() };

    msghdr.msg_iov = &mut io_vec;
    msghdr.msg_iovlen = 1;

    // SAFETY: All of the pointers passed to `libc::sendmsg` point to regions
    //         allocated inside this function.  The return value is checked and
    //         wrapped in a LibcResult.
    retry_eintr!(libc_result_from_int(unsafe { libc::sendmsg(socket_fd, &msghdr, 0) }))
}

/// A safe wrapper around [`libc::setgroups`]
///
/// See: `man setgroups`
pub fn setgroups(groups: &[libc::gid_t]) -> LibcResult<()> {
    // SAFETY: The length and pointer arguments are derived from a reference
    //         that outlives the call, and the return value is wrapped in a
    //         LibcResult.
    libc_result_from_int_with_void(unsafe { libc::setgroups(groups.len(), groups.as_ptr()) })
}

/// A safe wrapper around [`libc::setpgid`].
///
/// See: `man setpgid`
pub fn setpgid(pid: libc::pid_t, pgid: libc::pid_t) -> LibcResult<()> {
    // SAFETY: The `libc::setpgid` function takes no pointers and the return
    //         value is checked and wrapped in a LibcResult.
    libc_result_from_int_with_void(unsafe { libc::setpgid(pid, pgid) })
}

/// A safe wrapper around [`libc::setpriority`]
///
/// See: `man setpriority`
pub fn setpriority(which: which_t, who: libc::id_t, prio: c_int) -> LibcResult<c_int> {
    errno_clear();

    // SAFETY: `errno` is cleared before calling the function and checked upon
    //         return.
    let result: c_int = unsafe { libc::setpriority(which, who, prio) };
    if errno().code == 0 {
        Ok(result)
    } else {
        Err(errno())
    }
}

/// A safe wrapper around [`libc::setregid`]
///
/// See: `man setregid`
pub fn setregid(rgid: libc::gid_t, egid: libc::gid_t) -> LibcResult<()> {
    // SAFETY: The `libc::setregid` function takes no pointers and the return
    //         value is checked and wrapped in a LibcResult.
    libc_result_from_int_with_void(unsafe { libc::setregid(rgid, egid) })
}

/// A safe wrapper around [`libc::setresgid`]
///
/// See: `man setresgid`
pub fn setresgid(rgid: libc::gid_t, egid: libc::gid_t, sgid: libc::gid_t) -> LibcResult<()> {
    // SAFETY: The `libc::setresgid` function takes no pointers and the return
    //         value is checked and wrapped in a LibcResult.
    libc_result_from_int_with_void(unsafe { libc::setresgid(rgid, egid, sgid) })
}

/// A safe wrapper around [`libc::setreuid`]
///
/// See : `man setreuid`
pub fn setreuid(ruid: libc::uid_t, euid: libc::uid_t) -> LibcResult<()> {
    // SAFETY: The `libc::setreuid` function takes no pointers and the return
    //         value is checked and wrapped in a LibcResult.
    libc_result_from_int_with_void(unsafe { libc::setreuid(ruid, euid) })
}

/// A safe wrapper around [`libc::setresuid`]
///
/// See : `man setresuid`
pub fn setresuid(ruid: libc::uid_t, euid: libc::uid_t, suid: libc::uid_t) -> LibcResult<()> {
    // SAFETY: The `libc::setresuid` function takes no pointers and the return
    //         value is checked and wrapped in a LibcResult.
    libc_result_from_int_with_void(unsafe { libc::setresuid(ruid, euid, suid) })
}

/// A safe wrapper around [`libc::setrlimit`]
///
/// See: `man setrlimit`
pub fn setrlimit(resource: rlimit_resource_t, rlimit: &libc::rlimit) -> LibcResult<()> {
    // SAFETY: The pointer argument is derived from a valid reference and the
    //         return value is checked and wrapped in a LibcResult.
    libc_result_from_int_with_void(unsafe { libc::setrlimit(resource, rlimit) })
}

/// A safe wrapper around [`libc::sigaddset`]
///
/// See: `man sigsetops`
pub fn sigaddset(sigset: &mut libc::sigset_t, signum: c_int) -> LibcResult<()> {
    // SAFETY: The argument pointer is constructed from a valid reference.  The
    //         return value is checked and wrapped in a LibcResult.
    libc_result_from_int_with_void(unsafe { libc::sigaddset(sigset, signum) })
}

/// A safe wrapper around [`libc::sigemptyset`]
///
/// See: `man sigsetops`
pub fn sigemptyset() -> LibcResult<libc::sigset_t> {
    let mut sigset = MaybeUninit::<libc::sigset_t>::uninit();

    libc_result_from_int_with_payload(
        // SAFETY: The passed in pointer is valid and points to a location
        //         allocated on the previous line.
        unsafe { libc::sigemptyset(sigset.as_mut_ptr()) },
        // SAFETY: Libc guarantees that this signal set is now initialized.
        || unsafe { sigset.assume_init() },
    )
}

/// A safe wrapper around [`libc::signalfd`].
///
/// See: `man signalfd`
pub fn signalfd(fd: RawFd, sigset: &libc::sigset_t, flags: c_int) -> LibcResult<RawFd> {
    // SAFETY: The sigset pointer is guaranteed to be valid because it is
    //         passed in as a reference.  Invalid argument values will
    //         result in an error being returned by `libc::signalfd`.  The
    //         return value is checked and wrapped in a LibcResult.
    libc_result_from_int(unsafe { libc::signalfd(fd, sigset, flags) })
}

/// A safe wrapper around [`libc::sigprocmask`]
///
/// See `man sigprocmask`
pub fn sigprocmask(how: c_int, sigset: &libc::sigset_t) -> LibcResult<libc::sigset_t> {
    let mut sigset_out = MaybeUninit::<libc::sigset_t>::uninit();

    libc_result_from_int_with_payload(
        // SAFETY: The pointer arguments are guaranteed to be valid memory
        //         addresses as they are created from references.  The return
        //         value is checked and wrapped in a LibcResult.
        unsafe { libc::sigprocmask(how, sigset, sigset_out.as_mut_ptr()) },
        // SAFETY: Lib guarantees that this memory now contains a valid
        //         libc::sigset_t.
        || unsafe { sigset_out.assume_init() },
    )
}

/// A safe wrapper around [`libc::socket`].
///
/// See: `man socket`
pub fn socket(domain: c_int, ty: c_int, protocol: c_int) -> LibcResult<RawFd> {
    // SAFETY: Invalid argument values will result in a error being returned
    //         by `libc::socket`.  The return value is checked and wrapped in
    //         a LibcResult.
    libc_result_from_int(unsafe { libc::socket(domain, ty, protocol) })
}
