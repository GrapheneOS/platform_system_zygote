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
    ffi::{c_char, c_int, c_short, c_uint, c_void, CStr, FromBytesUntilNulError},
    mem::{self, offset_of},
};
use std::{
    fmt::{self, Debug, Display, Formatter},
    mem::MaybeUninit,
    os::fd::RawFd,
    ptr::NonNull,
};

use arrayvec::ArrayVec;
use static_assertions::const_assert;
use thiserror::Error;
use zerocopy::{error::ConvertError, FromBytes};

#[cfg(target_os = "android")]
pub mod android;
mod libc_fill;
pub mod procfs;

pub use libc_fill::clone_args;

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
/// A zero-initialized buffer, ensuring null-terminated strings
pub const BUFFER_INIT_CSTRING: CStringBuffer = [0u8; BUFFER_SIZE_STRINGS];

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
#[derive(Clone, Eq, PartialEq)]
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

impl Display for Errno {
    #[cfg(not(any(target_env = "musl", all(target_os = "linux", soong))))]
    fn fmt(&self, f: &mut Formatter<'_>) -> std::result::Result<(), fmt::Error> {
        write!(f, "Errno")?;
        if let Ok(name) = error_name(self.code) {
            write!(f, " ({:?})", name)?;
        }
        if let Ok(buffer) = error_description(self.code)
            && let Ok(desc) = buffer.as_cstr()
        {
            write!(f, ": {:?}", desc)
        } else {
            Ok(())
        }
    }

    #[cfg(any(target_env = "musl", all(target_os = "linux", soong)))]
    fn fmt(&self, f: &mut Formatter<'_>) -> std::result::Result<(), fmt::Error> {
        write!(f, "Errno")?;
        if let Ok(buff) = error_description(self.code)
            && let Ok(desc) = buff.as_cstr()
        {
            write!(f, ": {:?}", desc)
        } else {
            Ok(())
        }
    }
}

impl Debug for Errno {
    #[cfg(not(any(target_env = "musl", all(target_os = "linux", soong))))]
    fn fmt(&self, f: &mut Formatter<'_>) -> std::result::Result<(), fmt::Error> {
        let mut debug_struct = f.debug_struct("Errno");
        debug_struct.field("code", &self.code);
        if let Ok(name) = error_name(self.code) {
            debug_struct.field("name", &name);
        }
        if let Ok(buff) = error_description(self.code)
            && let Ok(desc) = buff.as_cstr()
        {
            debug_struct.field("description", &desc);
        }
        debug_struct.finish()
    }

    #[cfg(any(target_env = "musl", all(target_os = "linux", soong)))]
    fn fmt(&self, f: &mut Formatter<'_>) -> std::result::Result<(), fmt::Error> {
        let mut debug_struct = f.debug_struct("Errno");
        debug_struct.field("code", &self.code);
        if let Ok(buff) = error_description(self.code)
            && let Ok(desc) = buff.as_cstr()
        {
            debug_struct.field("description", &desc);
        }
        debug_struct.finish()
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

/// Possible errors for the `zygote-sys` crate
#[derive(Clone, Error, Debug)]
pub enum Error {
    /// C-style string was not nul-terminated
    #[error("Invalid C string")]
    InvalidCString(#[from] FromBytesUntilNulError),

    /// An invalid wait state was provided by the caller
    #[error("Invalid wait status: {0}")]
    InvalidWaitStatus(c_int),

    /// An error returned from a `libc` call
    #[error("Libc error: {0}")]
    Libc(#[from] Errno),

    /// An OS-provided string is not a valid UTF-8 encoded string
    #[error("OS string is not a valid UTF-8 encoded string")]
    OsString(std::ffi::OsString),

    /// A file descriptor passed to `poll` produced an error
    #[error("Poll event error on FD {0}: {1}")]
    PollEventError(RawFd, c_short),

    /// A provided string is too long to fit into a buffer
    #[error("Input string of length {0} exceeds buffer size {1}")]
    StringExceedsBufferSize(usize, usize),

    /// A provided byte array or OS string is not a valid UTF-8 encoded string
    #[error("UTF-8 error: {0}")]
    Utf8(#[from] core::str::Utf8Error),

    /// A `zerocopy` conversion failed
    #[error("Error during zerocopy conversion from u8 to c_char")]
    ZerocopyCast,
}

impl<A, S, V> From<ConvertError<A, S, V>> for Error {
    fn from(_: ConvertError<A, S, V>) -> Self {
        Error::ZerocopyCast
    }
}

impl From<Error> for std::fmt::Error {
    fn from(_: Error) -> std::fmt::Error {
        std::fmt::Error
    }
}

/// A Result type covering data validation and libc errors.
pub type Result<T> = std::result::Result<T, Error>;

impl<T> From<Errno> for Result<T> {
    fn from(errno: Errno) -> Self {
        Err(Error::Libc(errno))
    }
}

impl<T> From<Error> for Result<T> {
    fn from(error: Error) -> Self {
        Err(error)
    }
}

/// A wrapper struct for [`libc::pollfd`] that ensures error codes are checked
/// before events are handled.
#[derive(Debug)]
#[repr(transparent)]
pub struct PollFd(libc::pollfd);

impl PollFd {
    /// Initialize a new [`libc::pollfd`] wrapper struct with the provided file
    /// descriptor and events.
    pub fn new(fd: RawFd, events: c_short) -> Self {
        Self(libc::pollfd { fd, events, revents: 0 })
    }

    /// Returns `Err` in the presence of errors, else `Some(PollFdChecked)`.
    pub fn check(&self) -> Result<PollFdChecked<'_>> {
        const ERROR_MASK: c_short = libc::POLLERR | libc::POLLNVAL;
        if self.0.revents & ERROR_MASK != 0 {
            Err(Error::PollEventError(self.0.fd, self.0.revents & ERROR_MASK))
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

impl Drop for LibcDir {
    fn drop(&mut self) {
        // SAFETY: The LibcDir argument can only be constructed by the `opendir`
        //         function which also checks to ensure that the pointer is
        //         non-null.
        unsafe { libc::closedir(self.inner.as_ptr()) };
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

from_bytes_c!(c_int, libc::ucred, libc::signalfd_siginfo);

// SAFETY: Passing raw byte arrays to `libc` calls is safe.  This leaves the
//         safety of casting to and from bytes up to the caller.
unsafe impl<const N: usize> LibcFromBytes for [u8; N] {}

/// Convert an integer return value from a `libc` call into a [`Result`]
/// type.  If the return value is -1 the Error type will contain the resulting
/// `errno` value.
pub fn check_failure<T: Eq + From<i8>>(retval: T) -> Result<T> {
    if retval == T::from(-1) {
        errno().into()
    } else {
        Ok(retval)
    }
}

/// Test an integer `libc` return value and returns the auxiliary value if
/// is not equal to -1 and the `errno` value if it is.  The payload thunk is
/// only evaluated when `retval` does not indicate an error.
pub fn check_failure_with_payload<T: Eq + From<i8>, P>(
    retval: T,
    payload: impl FnOnce(T) -> P,
) -> Result<P> {
    if retval == T::from(-1) {
        errno().into()
    } else {
        Ok(payload(retval))
    }
}

/// Test an integer `libc` return value and return `void` if it is not equal to
/// -1 and the `errno` value if it is.
pub fn check_failure_with_void<T: Eq + From<i8>>(retval: T) -> Result<()> {
    if retval == T::from(-1) {
        errno().into()
    } else {
        Ok(())
    }
}

/// Converts a pointer into a [`Result`] type, returning an [`Errno`] if it
/// is `null` and converting it into the desired return type if it isn't.
#[cfg(not(any(target_env = "musl", all(target_os = "linux", soong))))]
fn check_failure_with_ptr<T, U, C: Fn(NonNull<T>) -> U>(
    retval: *const T,
    constructor: C,
) -> Result<U> {
    NonNull::new(retval as *mut T).map(constructor).ok_or(errno().into())
}

/// Converts a pointer into a [`Result`] type, returning an [`Errno`] if it
/// is `null` and converting it into the desired return type if it isn't.
fn check_failure_with_mut_ptr<T, U, C: Fn(NonNull<T>) -> U>(
    retval: *mut T,
    constructor: C,
) -> Result<U> {
    NonNull::new(retval).map(constructor).ok_or(errno().into())
}

/// Convert an integer return value from a `libc` call into a [`Result`]
/// type.  If the return value is not 0 the Error type will contain the
/// resulting `errno` value.
pub fn check_success<T: Eq + From<i8>>(retval: T) -> Result<T> {
    if retval == T::from(0) {
        Ok(retval)
    } else {
        errno().into()
    }
}

/// Test an integer `libc` return value and returns the auxiliary value if
/// is is equal to 0 and the `errno` value if it isn't.  The payload thunk is
/// only evaluated when `retval` does not indicate an error.
pub fn check_success_with_payload<T: Eq + From<i8>, P>(
    retval: T,
    payload: impl FnOnce(T) -> P,
) -> Result<P> {
    if retval == T::from(0) {
        Ok(payload(retval))
    } else {
        errno().into()
    }
}

/// Test an integer `libc` return value and return `void` if it is equal to 0
/// and the `errno` value if it is.
pub fn check_success_with_void<T: Eq + From<i8>>(retval: T) -> Result<()> {
    if retval == T::from(0) {
        Ok(())
    } else {
        errno().into()
    }
}

/*
 * Libc helpers
 */

/// A macro for retrying libc calls that result in an EINTR errno.
macro_rules! retry_eintr {
    ($libc_call:expr) => {
        loop {
            match $libc_call {
                Err(Error::Libc(errno)) if errno.is(libc::EINTR) => {
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
pub fn abstract_socket_address(
    name: &str,
    family: libc::sa_family_t,
) -> Result<SocketAddr<libc::sockaddr_un>> {
    let mut socket_addr = libc::sockaddr_un { sun_family: family, sun_path: [0; 108] };
    let name_view = &name.as_bytes()[0..std::cmp::min(name.len(), socket_addr.sun_path.len() - 1)];

    // Abstract socket names begin with a null character, so start copying the
    // name at index 1.
    socket_addr.sun_path[1..name_view.len() + 1]
        .copy_from_slice(<[c_char]>::ref_from_bytes(name_view)?);

    let socklen = offset_of!(libc::sockaddr_un, sun_path) + name.len() + 1;

    Ok(SocketAddr { address: socket_addr, socklen: socklen as libc::socklen_t })
}

/// Construct a bound socket name inside a [`libc::sockaddr_un`] struct from
/// the provided name and family.
pub fn bound_socket_address(
    path: &str,
    family: libc::sa_family_t,
) -> Result<SocketAddr<libc::sockaddr_un>> {
    let mut socket_addr = libc::sockaddr_un { sun_family: family, sun_path: [0; 108] };
    let name_view = &path.as_bytes()[0..std::cmp::min(path.len(), socket_addr.sun_path.len() - 1)];

    socket_addr.sun_path[0..name_view.len()]
        .copy_from_slice(<[c_char]>::ref_from_bytes(name_view)?);

    let socklen = offset_of!(libc::sockaddr_un, sun_path) + path.len();

    Ok(SocketAddr { address: socket_addr, socklen: socklen as libc::socklen_t })
}

/// Construct a [`libc::sigset_t`] containing the provided signals.
pub fn build_sigset(signals: &[c_int]) -> Result<libc::sigset_t> {
    let mut sigset = sigemptyset()?;

    for signum in signals {
        sigaddset(&mut sigset, *signum)?;
    }

    Ok(sigset)
}

/// A wrapper for socket addresses that includes the address length.
///
/// `SockAddrType` is the type that will be casted to [`libc::sockaddr`].
pub struct SocketAddr<SockAddrType> {
    /// The socket address.
    pub address: SockAddrType,
    /// The length of the socket address.
    pub socklen: libc::socklen_t,
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

/// This function calls the `task()` thunk and checks the Result.  If the
/// thunk exited with either `EAGAIN` or `EWOULDBLOCK` the function will
/// immediately return a [`LoopExit::WouldBlock`] value.  If any other error
/// was returned by the thunk it will be returned by this function.  If the
/// thunk completed successfully the handler will be called and, based on the
/// return value, the function will either return or continue.  If the function
/// returns early the [`LoopStatus::Break`] value will be forwarded in a
/// [`LoopExit::Early`] variant.
pub fn call_until_would_block<T, U>(
    task: impl Fn() -> Result<T>,
    mut handler: impl FnMut(T) -> LoopControl<U>,
) -> Result<LoopExit<U>> {
    loop {
        match task() {
            Ok(result) => match handler(result) {
                LoopControl::Continue => continue,
                LoopControl::Break(val) => return Ok(LoopExit::Early(val)),
            },
            Err(Error::Libc(errno)) if errno.is(libc::EAGAIN) || errno.is(libc::EWOULDBLOCK) => {
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
    let socket_addr = abstract_socket_address(name, libc::AF_UNIX as libc::sa_family_t)?;

    bind(socket_fd, &socket_addr)?;

    Ok(socket_fd)
}

/// Create a new UNIX domain datagram socket and bind it to the provided file
/// system path
pub fn create_bound_socket(path: &str, protocol: c_int) -> Result<RawFd> {
    let socket_fd = socket(libc::AF_UNIX, protocol, 0)?;
    let socket_addr = bound_socket_address(path, libc::AF_UNIX as libc::sa_family_t)?;

    bind(socket_fd, &socket_addr)?;

    Ok(socket_fd)
}

/// Select the file-type bits from the `mode` value
pub fn get_file_type(stat: libc::stat) -> libc::mode_t {
    (stat.st_mode as libc::mode_t) & libc::S_IFMT
}

/// Get the socket type of a UNIX domain socket.
pub fn get_socket_type(fd: RawFd) -> Result<c_int> {
    getsockopt(fd, libc::SOL_SOCKET, libc::SO_TYPE)
}

/// Get the PID, UID, and GID for the remote end of a UNIX domain socket.
pub fn get_socket_creds(fd: RawFd) -> Result<libc::ucred> {
    getsockopt(fd, libc::SOL_SOCKET, libc::SO_PEERCRED)
}

/// Attempt to read `size_of::<T>()` bytes.  If the correct number of bytes
/// were read the function returns `Ok(T)`, otherwise an `Err(Errno)` is
/// returned with the `code` set to 0.
pub fn read_exact<T: LibcFromBytes>(fd: RawFd) -> Result<T> {
    read::<T>(fd).and_then(|(read_len, result)| {
        if read_len == std::mem::size_of::<T>() as isize {
            Ok(result)
        } else {
            Err(Error::Libc(Errno { code: 0 }))
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
        if let Err(e) = self.exit()
            && !std::thread::panicking()
        {
            panic!("Failed to restore effective permissions on drop: {e}")
        }
    }
}

/// Build a buffer-allocated NUL-terminated representation of a string
pub fn build_nul_terminated_string(str_in: &str) -> Result<CStringBuffer> {
    if str_in.len() >= BUFFER_SIZE_STRINGS {
        return Err(Error::StringExceedsBufferSize(str_in.len(), BUFFER_SIZE_STRINGS));
    }

    let mut buffer: CStringBuffer = BUFFER_INIT_CSTRING;
    buffer[..str_in.len()].copy_from_slice(str_in.as_bytes());

    Ok(buffer)
}

/// Generate a random fixed-length lowercase filename
pub fn random_filename<const N: usize>() -> [u8; N] {
    let mut buffer: [u8; N] = [0; N];
    for byte in buffer.iter_mut() {
        *byte = rand::random_range(97u8..=122u8);
    }

    buffer
}

/// Test to see if the current process can write to a path
pub fn have_write_permissions(path: &std::path::Path) -> Result<bool> {
    let test_path = if path.exists() && path.is_dir() {
        path.join(str::from_utf8(&random_filename::<16>())?)
    } else {
        path.to_path_buf()
    };

    let preexisting_path = test_path.exists();
    let path_buffer = build_nul_terminated_string(
        test_path.to_str().ok_or(Error::OsString(test_path.as_os_str().to_os_string()))?,
    )?;
    let open_result = open(path_buffer.as_cstr()?, libc::O_CREAT | libc::O_WRONLY, Some(0o600));

    match open_result {
        Ok(fd) => {
            close(fd)?;
            if !preexisting_path {
                unlink(path_buffer.as_cstr()?)?;
            }
            Ok(true)
        }
        Err(Error::Libc(Errno { code: libc::EACCES })) => Ok(false),
        Err(e) => Err(e),
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
pub fn accept(fd: RawFd) -> Result<RawFd> {
    // SAFETY: If the file descriptor is invalid `accept()` will return -1 and
    //         we will extract errno and wrap it in a Result.
    check_failure(unsafe { libc::accept(fd, std::ptr::null_mut(), std::ptr::null_mut()) })
}

/// A safe wrapper around [`libc::bind`].
///
/// See: `man bind`
pub fn bind<SockAddrType>(fd: RawFd, sockaddr: &SocketAddr<SockAddrType>) -> Result<()> {
    // SAFETY: The pointer argument to `libc::bind` is guaranteed to reference
    //         allocated memory and the return value is checked and wrapped in
    //         a Result.
    check_failure_with_void(unsafe {
        libc::bind(
            fd,
            (&sockaddr.address as *const SockAddrType) as *const libc::sockaddr,
            sockaddr.socklen,
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
pub unsafe fn clone3(args: &clone_args) -> Result<libc::pid_t> {
    // SAFETY: The pointer argument is derived from a valid reference and the
    //         result value is checked and wrapped in a Result.
    check_failure(unsafe { libc_fill::clone3(args) as libc::pid_t })
}

/// A safe wrapper around [`libc::close`].
///
/// The [`libc::EINTR`] signal is handled internally using the [`retry_eintr!`]
/// macro.
///
/// See: `man close`
pub fn close(fd: RawFd) -> Result<c_int> {
    // SAFETY: If the file descriptor is invalid `close()` will return -1 and
    //         we will extract errno and wrap it in a Result.
    retry_eintr!(check_failure(unsafe { libc::close(fd) }))
}

/// A safe wrapper around [`libc::closedir`].
///
/// See: `man closedir`
pub fn closedir(dir: LibcDir) -> Result<()> {
    // SAFETY: The LibcDir argument can only be constructed by the `opendir`
    //         function which also checks to ensure that the pointer is
    //         non-null.  If the LibcDir object *does* contain an invalid
    //         pointer then `libc` will return `-1` and a Result::Err value
    //         will be returned wrapping `errno`.
    check_failure_with_void(unsafe { libc::closedir(dir.inner.as_ptr()) })
}

/// A safe wrapper around [`libc::connect`].
///
/// See: `man connect`
pub fn connect<SockAddrType>(fd: RawFd, sockaddr: &SocketAddr<SockAddrType>) -> Result<()> {
    // SAFETY: The pointer argument to `libc::connect` is guaranteed to reference
    //         allocated memory and the return value is checked and wrapped in
    //         a Result.
    check_failure_with_void(unsafe {
        libc::connect(
            fd,
            (&sockaddr.address as *const SockAddrType) as *const libc::sockaddr,
            sockaddr.socklen,
        )
    })
}

/// A safe wrapper around [`libc::dirfd`].
///
/// See: `man dirfd`
pub fn dirfd(dir: &LibcDir) -> Result<RawFd> {
    // SAFETY: The LibcDir argument can only be constructed by the `opendir`
    //         function which also checks to ensure that the pointer is
    //         non-null.  If the LibcDir object *does* contain an invalid
    //         pointer then `libc` will return `-1` and a Result::Err value
    //         will be returned wrapping `errno`.
    check_failure(unsafe { libc::dirfd(dir.inner.as_ptr()) })
}

/// A safe wrapper around [`libc::dup3`].
///
/// See: `man dup3`
pub fn dup3(old_fd: RawFd, new_fd: RawFd, flags: c_int) -> Result<()> {
    #[cfg(not(target_os = "android"))]
    // SAFETY: Invalid arguments will result in an error code being returned
    //         that will be wrapped by a Result.
    return retry_eintr!(check_failure_with_void(unsafe { libc::dup3(old_fd, new_fd, flags) }));

    #[cfg(target_os = "android")]
    // SAFETY: Invalid arguments will result in an error code being returned
    //         that will be wrapped by a Result.
    return retry_eintr!(check_failure_with_void(unsafe {
        libc_fill::dup3(old_fd, new_fd, flags)
    }));
}

/// A safe wrapper around [`libc::strerror_r`].
///
/// This function uses `strerror_r` because `strerror` is not thread-safe.
///
/// See: `man strerror`
pub fn error_description(code: c_int) -> Result<CStringBuffer> {
    let mut buffer: CStringBuffer = BUFFER_INIT_CSTRING;

    // SAFETY: The pointer argument refers to memory allocated in this function.
    let retval = unsafe { libc::strerror_r(code, buffer.as_mut_ptr().cast(), buffer.len()) };

    if retval == 0 {
        Ok(buffer)
    } else {
        Err(Error::Libc(Errno { code: retval }))
    }
}

/// A safe wrapper around [`libc_fill::strerrorname_np`]
///
/// See: `man strerror`
#[cfg(not(any(target_env = "musl", all(target_os = "linux", soong))))]
pub fn error_name(code: c_int) -> Result<&'static CStr> {
    // SAFETY: This function takes no pointers and the return result is checked
    //         and wrapped in a Result.  The returned pointers point to
    //         statically allocated null-terminated strings and are safe to
    //         cast to `CStr`s.
    unsafe {
        check_failure_with_ptr(libc_fill::strerrorname_np(code), |non_null_ptr: NonNull<c_char>| {
            CStr::from_ptr(non_null_ptr.as_ptr() as *const c_char)
        })
    }
}

/// A safe wrapper around [`libc::fcntl`], passing [`libc::F_GETFD`] as the
/// `op`.
///
/// See: `man fcntl`
pub fn fcntl_getfd(fd: RawFd) -> Result<c_int> {
    // SAFETY: An invalid file descriptor will result in an error code being
    //         returned that will be wrapped by a Result.
    check_failure(unsafe { libc::fcntl(fd, libc::F_GETFD) })
}

/// A safe wrapper around [`libc::fcntl`], passing [`libc::F_GETFL`] as the
/// `op`.
///
/// See: `man fcntl`
pub fn fcntl_getfl(fd: RawFd) -> Result<c_int> {
    // SAFETY: An invalid file descriptor will result in an error code being
    //         returned that will be wrapped by a Result.
    check_failure(unsafe { libc::fcntl(fd, libc::F_GETFL) })
}

/// A safe wrapper around [`libc::fcntl`], passing [`libc::F_SETFD`] as the
/// `op`.
///
/// See: `man fcntl`
pub fn fcntl_setfd(fd: RawFd, flags: c_int) -> Result<()> {
    // SAFETY: Invalid arguments will result in an error code being returned
    //         that will be wrapped by a Result.
    check_failure_with_void(unsafe { libc::fcntl(fd, libc::F_SETFD, flags) })
}

/// A safe wrapper around [`libc::fcntl`], passing [`libc::F_SETFL`] as the
/// `op`.
///
/// See: `man fcntl`
pub fn fcntl_setfl(fd: RawFd, flags: c_int) -> Result<()> {
    // SAFETY: Invalid arguments will result in an error code being returned
    //         that will be wrapped by a Result.
    check_failure_with_void(unsafe { libc::fcntl(fd, libc::F_SETFL, flags) })
}

/// A safe wrapper around [`libc::fork`].
///
/// # Safety
/// This function results in undefined behavior if it is called from a process
/// with multiple threads.  The calling process *MUST* be single threaded for
/// calls to this function to be safe.
///
/// See: `man fork`
pub unsafe fn fork() -> Result<libc::pid_t> {
    // SAFETY: The `libc::fork` function takes no pointers and the return
    //         value is checked and wrapped in a Result.
    check_failure(unsafe { libc::fork() })
}

/// A safe wrapper around [`libc::fstat`].
///
/// See: `man fstat`
pub fn fstat(fd: RawFd) -> Result<libc::stat> {
    let mut buffer = MaybeUninit::<libc::stat>::uninit();

    check_failure_with_payload(
        // SAFETY: The stat buffer pointer is guaranteed to point to a valid
        //         memory address due to its allocation inside this function's
        //         stack.  The return value is checked and wrapped in a
        //         Result.
        unsafe { libc::fstat(fd, buffer.as_mut_ptr().cast()) },
        // SAFETY: The `libc::fstat` function fully initializes the stat
        //         struct.
        |_| unsafe { buffer.assume_init() },
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
pub fn getpriority(which: which_t, who: libc::id_t) -> Result<c_int> {
    errno_clear();

    // SAFETY: `errno` is cleared before calling the function and checked upon
    //         return.
    let result: c_int = unsafe { libc::getpriority(which, who) };
    if errno().code == 0 {
        Ok(result)
    } else {
        errno().into()
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
pub fn getgroups<const N: usize>() -> Result<ArrayVec<libc::gid_t, N>> {
    let mut buffer = [0; N];

    // SAFETY: The pointer argument refers to a stack-allocated buffer
    //         parameterized by N.  The result value is checked and wrapped in
    //         Result.
    check_failure_with_payload(unsafe { libc::getgroups(N as c_int, buffer.as_mut_ptr()) }, |n| {
        ArrayVec::from_iter(buffer.into_iter().take(n as usize))
    })
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
pub fn getrlimit(resource: rlimit_resource_t) -> Result<libc::rlimit> {
    let mut rlimit = libc::rlimit { rlim_cur: 0, rlim_max: 0 };

    // SAFETY: The pointer argument is derived from a local stack reference and
    //         the return value is checked and wrapped in a Result.
    check_failure_with_payload(unsafe { libc::getrlimit(resource, &mut rlimit) }, |_| rlimit)
}

/// A safe wrapper around [`libc::getsockopt`].
///
/// See: `man getsockopt`
pub fn getsockopt<T: LibcFromBytes>(fd: RawFd, level: c_int, optname: c_int) -> Result<T> {
    let mut optval = MaybeUninit::<T>::zeroed();
    let mut optlen = std::mem::size_of::<T>() as libc::socklen_t;

    check_failure_with_payload(
        // SAFETY: The `optval` and `optlen` arguments are valid pointers to
        //         memory allocated in this function.  The return value is
        //         checked and wrapped in a Result.
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
        |_| unsafe { optval.assume_init() },
    )
}

/// A safe wrapper around [`libc::getsockname`].
///
/// None will be returned if the socket family is not `AF_UNIX` or the socket is unbound.
///
/// See: `man getsockname`
pub fn getsockname(fd: RawFd) -> Result<Option<(libc::sockaddr_un, usize)>> {
    let mut addr = std::mem::MaybeUninit::<libc::sockaddr_un>::zeroed();
    let mut addr_len = std::mem::size_of::<libc::sockaddr_un>() as libc::socklen_t;

    // SAFETY: The address buffer pointer is guaranteed to reference valid
    //         memory that has been zeroed out.  This ensures that the any
    //         strings contained in the buffer will be valid null-terminated
    //         C strings.  The return value is checked and wrapped in a
    //         Result.
    check_failure(unsafe { libc::getsockname(fd, addr.as_mut_ptr().cast(), &mut addr_len) })?;

    debug_assert!(addr_len <= std::mem::size_of::<libc::sockaddr_un>() as libc::socklen_t);

    if addr_len as usize == std::mem::size_of::<libc::sa_family_t>() {
        return Ok(None);
    }

    // SAFETY: The memory had been zero initialized before `libc::getsockname`
    //         filled in any relevant data.  All strings should be valid C
    //         strings.
    let addr = unsafe { addr.assume_init() };

    if addr.sun_family != libc::AF_UNIX as u16 {
        return Ok(None);
    }

    let path_len = addr_len as usize - offset_of!(libc::sockaddr_un, sun_path);

    Ok(Some((addr, path_len)))
}

/// A safe wrapper around [`libc::getpeername`].
///
/// None will be returned if the socket family is not `AF_UNIX` or the socket is not connected.
///
/// See: `man getpeername`
pub fn getpeername(fd: RawFd) -> Result<Option<(libc::sockaddr_un, usize)>> {
    let mut addr = std::mem::MaybeUninit::<libc::sockaddr_un>::zeroed();
    let mut addr_len = std::mem::size_of::<libc::sockaddr_un>() as libc::socklen_t;

    // SAFETY: The address buffer pointer is guaranteed to reference valid
    //         memory that has been zeroed out.  This ensures that the any
    //         strings contained in the buffer will be valid null-terminated
    //         C strings.  The return value is checked and wrapped in a
    //         Result.
    check_failure(unsafe { libc::getpeername(fd, addr.as_mut_ptr().cast(), &mut addr_len) })?;

    // SAFETY: The memory had been zero initialized before `libc::getpeername`
    //         filled in any relevant data.  All strings should be valid C
    //         strings.
    let addr = unsafe { addr.assume_init() };

    debug_assert!(addr_len <= std::mem::size_of::<libc::sockaddr_un>() as libc::socklen_t);

    if addr_len as usize == std::mem::size_of::<libc::sa_family_t>() {
        return Ok(None);
    }

    if addr.sun_family != libc::AF_UNIX as u16 {
        return Ok(None);
    }

    let path_len = addr_len as usize - offset_of!(libc::sockaddr_un, sun_path);

    Ok(Some((addr, path_len)))
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
pub fn listen(fd: RawFd, backlog: c_int) -> Result<()> {
    // SAFETY: The `libc::listen` function takes no pointers and the return
    //         value is checked and wrapped in a Result.
    check_failure_with_void(unsafe { libc::listen(fd, backlog) })
}

/// A safe wrapper around [`libc::lseek64`].
///
/// See: `man lseek64`
pub fn lseek64(fd: RawFd, offset: libc::off64_t, whence: c_int) -> Result<libc::off64_t> {
    // SAFETY: The `libc::lseek64` function takes no pointers and the return
    //         value is checked and wrapped in a Result.
    check_failure(unsafe { libc::lseek64(fd, offset, whence) })
}

/// A safe wrapper around [`libc::mallopt`].
///
/// See: `man mallopt`
#[cfg(not(target_env = "musl"))]
pub fn mallopt(cmd: c_int, arg: c_int) -> Result<()> {
    // SAFETY: This function takes no pointer arguments and the return value
    //         is checked and wrapped in a Result.
    let retval = unsafe { libc::mallopt(cmd, arg) };

    // This function returns 1 on success and 0 on error.
    if retval == 0 {
        errno().into()
    } else {
        Ok(())
    }
}

/// A safe wrapper around [`libc::open`].
///
/// See: `man open`
pub fn open(path: &CStr, flags: c_int, mode: Option<libc::mode_t>) -> Result<RawFd> {
    // SAFETY: Path points to a valid, null-terminated C string and the return
    //         value is checked and wrapped in a Result.
    retry_eintr!(check_failure(unsafe {
        libc::open(path.as_ptr().cast(), flags, mode.unwrap_or(0) as c_uint)
    }))
}

/// A safe wrapper around [`libc::opendir`].
///
/// See: `man opendir`
pub fn opendir(path: &CStr) -> Result<LibcDir> {
    // SAFETY: Path points to a valid, null-terminated C string and the return
    //         value is checked and wrapped in a Result.  The pointer
    //         itself has been verified as non-null and is thus wrapped in a
    //         Result.
    check_failure_with_mut_ptr(unsafe { libc::opendir(path.as_ptr()) }, LibcDir::from_raw)
}

/// A safe wrapper around [`libc::pipe`].
///
/// See: `man pipe`
pub fn pipe() -> Result<(RawFd, RawFd)> {
    let mut pipes = [0; 2];
    check_failure_with_payload(
        // SAFETY: The file descriptor buffer is guaranteed to point to valid
        //         memory and the return value is checked before the resulting
        //         pipes are wrapped in a Result.
        unsafe { libc::pipe(pipes.as_mut_ptr()) },
        |_| (pipes[0], pipes[1]),
    )
}

/// A safe wrapper around [`libc::poll`].
///
/// See `man poll`
pub fn poll(pollfds: &mut [PollFd], timeout: c_int) -> Result<c_int> {
    // SAFETY: The `pollfds` pointer is valid because it is derived from a
    //         a valid slice reference and the length argument is obtained from
    //         the provided slice.  The return value is checked and wrapped in
    //         a Result.
    //
    //         The cast between `*mut PollFd` and `*mut libc::pollfd` is safe
    //         due to the use of `#[repr(transparent)]` on PollFd.
    check_failure(unsafe {
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
pub fn prctl_get_securebits() -> Result<c_int> {
    // SAFETY: The `libc::prctl` `PR_GET_SECUREBITS` operation takes no pointer
    //         arguments and can only return `EINVAL` if the argument is
    //         invalid.  The argument is guaranteed to be valid in this case.
    check_failure(unsafe { libc::prctl(libc::PR_GET_SECUREBITS) })
}

/// A safe wrapper around the [`libc::prctl`] `PR_SET_SECUREBITS` operation.
///
/// See: `man prctl`
/// See: `man capabilities`
pub fn prctl_set_securebits(securebits: c_int) -> Result<()> {
    // SAFETY: The `libc::prctl` `PR_SET_SECUREBITS` operation takes an integer
    //         argument and can only return `EINVAL` if the argument is
    //         invalid.  The argument is guaranteed to be valid in this case.
    check_failure_with_void(unsafe { libc::prctl(libc::PR_SET_SECUREBITS, securebits) })
}

/// A safe wrapper around the [`libc::prctl`] `PR_SET_NO_NEW_PRIVS` operation.
///
/// See: `man prctl`
pub fn prctl_set_no_new_privs() -> Result<()> {
    // SAFETY: The `libc::prctl` `PR_SET_NO_NEW_PRIVS` operation takes an integer
    //         argument and can only return `EINVAL` if the argument is
    //         invalid.  The argument is guaranteed to be valid in this case.
    check_failure_with_void(unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) })
}

/// A safe wrapper around [`libc::read`].
///
/// See: `man read`
pub fn read<T: LibcFromBytes>(fd: RawFd) -> Result<(isize, T)> {
    let mut buffer = MaybeUninit::<T>::zeroed();

    // SAFETY: The `buff` pointer is valid because it is derived from a value
    //         allocated in this function and the `count` argument is
    //         calculated using `size_of()`.  The return value is checked and
    //         wrapped in a Result.
    retry_eintr!(check_failure(unsafe {
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
    //         return value is checked and wrapped in a Result.  The
    //         pointer itself has been verified as non-null and is thus wrapped
    //         in a Result.
    check_failure_with_mut_ptr(unsafe { libc::readdir(dir.inner.as_ptr()) }, std::convert::identity)
        .ok()
}

/// A safe wrapper around [`libc::readlink`].
///
/// See: `man readlink`
pub fn readlink(path_name: &CStr) -> Result<CStringBuffer> {
    let mut link_buffer: CStringBuffer = BUFFER_INIT_CSTRING;

    check_failure_with_payload(
        // SAFETY: The path name is guaranteed to be a valid null-terminated C
        //         string.  The link buffer is guaranteed to be a
        //         zero-initialized buffer and the size_of function is used to
        //         provide the buffer length to the libc call.  The return
        //         value is checked and wrapped in a Result.
        unsafe {
            libc::readlink(
                path_name.as_ptr(),
                link_buffer.as_mut_ptr().cast(),
                std::mem::size_of::<CStringBuffer>(),
            )
        },
        |_| link_buffer,
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
pub fn recvmsg<const BUFFER_SIZE: usize>(fd: RawFd) -> Result<(isize, [u8; BUFFER_SIZE])> {
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
    //         wrapped in a Result.
    retry_eintr!(check_failure(unsafe { libc::recvmsg(fd, &mut msghdr, 0) }))
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
pub fn sendmsg(socket_fd: RawFd, buffer: &[u8]) -> Result<isize> {
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
    //         wrapped in a Result.
    retry_eintr!(check_failure(unsafe { libc::sendmsg(socket_fd, &msghdr, 0) }))
}

/// A safe wrapper around [`libc::setgroups`]
///
/// See: `man setgroups`
pub fn setgroups(groups: &[libc::gid_t]) -> Result<()> {
    // SAFETY: The length and pointer arguments are derived from a reference
    //         that outlives the call, and the return value is wrapped in a
    //         Result.
    check_failure_with_void(unsafe { libc::setgroups(groups.len(), groups.as_ptr()) })
}

/// A safe wrapper around [`libc::setpgid`].
///
/// See: `man setpgid`
pub fn setpgid(pid: libc::pid_t, pgid: libc::pid_t) -> Result<()> {
    // SAFETY: The `libc::setpgid` function takes no pointers and the return
    //         value is checked and wrapped in a Result.
    check_failure_with_void(unsafe { libc::setpgid(pid, pgid) })
}

/// A safe wrapper around [`libc::setpriority`]
///
/// See: `man setpriority`
pub fn setpriority(which: which_t, who: libc::id_t, prio: c_int) -> Result<c_int> {
    errno_clear();

    // SAFETY: `errno` is cleared before calling the function and checked upon
    //         return.
    check_success(unsafe { libc::setpriority(which, who, prio) })
}

/// A safe wrapper around [`libc::setregid`]
///
/// See: `man setregid`
pub fn setregid(rgid: libc::gid_t, egid: libc::gid_t) -> Result<()> {
    // SAFETY: The `libc::setregid` function takes no pointers and the return
    //         value is checked and wrapped in a Result.
    check_failure_with_void(unsafe { libc::setregid(rgid, egid) })
}

/// A safe wrapper around [`libc::setresgid`]
///
/// See: `man setresgid`
pub fn setresgid(rgid: libc::gid_t, egid: libc::gid_t, sgid: libc::gid_t) -> Result<()> {
    // SAFETY: The `libc::setresgid` function takes no pointers and the return
    //         value is checked and wrapped in a Result.
    check_failure_with_void(unsafe { libc::setresgid(rgid, egid, sgid) })
}

/// A safe wrapper around [`libc::setreuid`]
///
/// See : `man setreuid`
pub fn setreuid(ruid: libc::uid_t, euid: libc::uid_t) -> Result<()> {
    // SAFETY: The `libc::setreuid` function takes no pointers and the return
    //         value is checked and wrapped in a Result.
    check_failure_with_void(unsafe { libc::setreuid(ruid, euid) })
}

/// A safe wrapper around [`libc::setresuid`]
///
/// See : `man setresuid`
pub fn setresuid(ruid: libc::uid_t, euid: libc::uid_t, suid: libc::uid_t) -> Result<()> {
    // SAFETY: The `libc::setresuid` function takes no pointers and the return
    //         value is checked and wrapped in a Result.
    check_failure_with_void(unsafe { libc::setresuid(ruid, euid, suid) })
}

/// A safe wrapper around [`libc::setrlimit`]
///
/// See: `man setrlimit`
pub fn setrlimit(resource: rlimit_resource_t, rlimit: &libc::rlimit) -> Result<()> {
    // SAFETY: The pointer argument is derived from a valid reference and the
    //         return value is checked and wrapped in a Result.
    check_failure_with_void(unsafe { libc::setrlimit(resource, rlimit) })
}

/// A safe wrapper around [`libc::sigaddset`]
///
/// See: `man sigsetops`
pub fn sigaddset(sigset: &mut libc::sigset_t, signum: c_int) -> Result<()> {
    // SAFETY: The argument pointer is constructed from a valid reference.  The
    //         return value is checked and wrapped in a Result.
    check_failure_with_void(unsafe { libc::sigaddset(sigset, signum) })
}

/// A safe wrapper around [`libc::sigemptyset`]
///
/// See: `man sigsetops`
pub fn sigemptyset() -> Result<libc::sigset_t> {
    let mut sigset = MaybeUninit::<libc::sigset_t>::uninit();

    check_failure_with_payload(
        // SAFETY: The passed in pointer is valid and points to a location
        //         allocated on the previous line.
        unsafe { libc::sigemptyset(sigset.as_mut_ptr()) },
        // SAFETY: Libc guarantees that this signal set is now initialized.
        |_| unsafe { sigset.assume_init() },
    )
}

/// A safe wrapper around [`libc::signalfd`].
///
/// See: `man signalfd`
pub fn signalfd(fd: RawFd, sigset: &libc::sigset_t, flags: c_int) -> Result<RawFd> {
    // SAFETY: The sigset pointer is guaranteed to be valid because it is
    //         passed in as a reference.  Invalid argument values will
    //         result in an error being returned by `libc::signalfd`.  The
    //         return value is checked and wrapped in a Result.
    check_failure(unsafe { libc::signalfd(fd, sigset, flags) })
}

/// A safe wrapper around [`libc::sigprocmask`]
///
/// See `man sigprocmask`
pub fn sigprocmask(how: c_int, sigset: &libc::sigset_t) -> Result<libc::sigset_t> {
    let mut sigset_out = MaybeUninit::<libc::sigset_t>::uninit();

    check_failure_with_payload(
        // SAFETY: The pointer arguments are guaranteed to be valid memory
        //         addresses as they are created from references.  The return
        //         value is checked and wrapped in a Result.
        unsafe { libc::sigprocmask(how, sigset, sigset_out.as_mut_ptr()) },
        // SAFETY: Lib guarantees that this memory now contains a valid
        //         libc::sigset_t.
        |_| unsafe { sigset_out.assume_init() },
    )
}

/// A safe wrapper around [`libc::socket`].
///
/// See: `man socket`
pub fn socket(domain: c_int, ty: c_int, protocol: c_int) -> Result<RawFd> {
    // SAFETY: Invalid argument values will result in a error being returned
    //         by `libc::socket`.  The return value is checked and wrapped in
    //         a Result.
    check_failure(unsafe { libc::socket(domain, ty, protocol) })
}

/// A safe wrapper around [`libc::unlink`].
///
/// See: `man 2 unlink`
pub fn unlink(path: &CStr) -> Result<()> {
    // SAFETY: Path points to a valid, null-terminated C string and the return
    //         value is checked and wrapped in a Result.
    check_failure_with_void(unsafe { libc::unlink(path.as_ptr()) })
}

/// The status of a waited-for child process.
#[derive(Debug, Eq, PartialEq)]
pub enum WaitStatus {
    /// The child exited normally with the given exit code.
    Exited(c_int),
    /// The child was terminated by the given signal.
    Signaled(c_int),
    /// The child was stopped by the given signal.
    Stopped(c_int),
    /// The child was continued.
    Continued,
    /// The child is a job control stop and has been traced.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    PtraceEvent(c_int),
    /// The child is a job control stop and has been traced.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    PtraceSyscall,
}

impl WaitStatus {
    fn from_status(status: c_int) -> Result<Self> {
        if libc::WIFEXITED(status) {
            Ok(WaitStatus::Exited(libc::WEXITSTATUS(status)))
        } else if libc::WIFSIGNALED(status) {
            Ok(WaitStatus::Signaled(libc::WTERMSIG(status)))
        } else if libc::WIFSTOPPED(status) {
            Ok(Self::from_stop_signal(status))
        } else if libc::WIFCONTINUED(status) {
            Ok(WaitStatus::Continued)
        } else {
            Err(Error::InvalidWaitStatus(status))
        }
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    fn from_stop_signal(status: c_int) -> Self {
        let stop_signal = libc::WSTOPSIG(status);
        if stop_signal == libc::SIGTRAP | 0x80 {
            // According to `man 2 ptrace`, setting PTRACE_O_TRACESYSGOOD
            // deliveres SIGTRAP | 0x80 as the signal number for syscall stop,
            // making it easy for the tracer to distinguish normal SIGTRAPs from
            // those caused by a system call.
            WaitStatus::PtraceSyscall
        } else if stop_signal == libc::SIGTRAP && status >> 16 != 0 {
            // As per `man 2 ptrace`:
            //   PTRACE_EVENT stops are observed by the tracer as waitpid(2)
            //   returning with WIFSTOPPED(status), and WSTOPSIG(status) returns
            //   SIGTRAP (or for PTRACE_EVENT_STOP, returns the stopping signal
            //   if tracee is in a group-stop).  An additional bit is set in the
            //   higher byte of the status word: the value status>>8 will be
            //   ((PTRACE_EVENT_foo<<8) | SIGTRAP).
            WaitStatus::PtraceEvent(status >> 16)
        } else {
            WaitStatus::Stopped(stop_signal)
        }
    }

    #[cfg(not(any(target_os = "android", target_os = "linux")))]
    fn from_stop_signal(status: c_int) -> Self {
        let stop_signal = libc::WSTOPSIG(status);
        WaitStatus::Stopped(stop_signal)
    }
}

/// A safe wrapper around [`libc::waitpid`].
///
/// The [`libc::EINTR`] signal is handled internally using the [`retry_eintr!`]
/// macro.
///
/// See: `man waitpid`
pub fn waitpid(
    pid: Option<libc::pid_t>,
    options: c_int,
) -> Result<Option<(libc::pid_t, WaitStatus)>> {
    let mut status = 0;
    let pid_arg = pid.unwrap_or(-1);
    let result = retry_eintr!(check_failure(
        // SAFETY: `&mut status` is a valid mutable pointer to a stack-allocated `c_int`.
        //         The return value is immediately checked for errors.
        unsafe { libc::waitpid(pid_arg, &mut status, options) }
    ))?;

    if result == 0 {
        // This occurs when WNOHANG is specified and no child has changed state.
        return Ok(None);
    }

    Ok(Some((result, WaitStatus::from_status(status)?)))
}
