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
    mem::offset_of,
};
use std::{
    fmt::{self, Debug, Display, Formatter},
    mem::MaybeUninit,
    os::fd::RawFd,
    ptr::NonNull,
};

use anyhow::Result;
use static_assertions::const_assert;
use zerocopy::FromBytes;

#[cfg(target_os = "android")]
use crate::libc_fill;

/// Number of bytes to allocate for string buffers.  The value 512 was selected
/// because it was large enough to fit all strings that were observed during
/// testing.  Testing should be performed to see if a smaller value can be
/// used.
pub const STRING_BUF_SIZE: usize = 512;
const_assert!(
    STRING_BUF_SIZE
        >= std::mem::size_of::<libc::sockaddr_un>() - offset_of!(libc::sockaddr_un, sun_path)
);

// TODO: Consider making this an ArrayVec
/// Buffers used for static string allocations
pub type CStringBuffer = [u8; STRING_BUF_SIZE];
const CSTRING_BUFFER_INIT: CStringBuffer = [0u8; STRING_BUF_SIZE];

const MSGHDR_ZERO_INIT: libc::msghdr = libc::msghdr {
    msg_name: std::ptr::null_mut(),
    msg_namelen: 0,
    msg_iov: std::ptr::null_mut(),
    msg_iovlen: 0,
    msg_control: std::ptr::null_mut(),
    msg_controllen: 0,
    msg_flags: 0,
};

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
}

// TODO: use strerror_r to retrieve the error code
impl Display for Errno {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), fmt::Error> {
        write!(f, "Errno: {}", self.code)
    }
}

// TODO: use strerror_r to retrieve the error code
impl Debug for Errno {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), fmt::Error> {
        f.debug_struct("Errno").field("code", &self.code).finish()
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

/// A Result type that uses Errno for the Error type
pub type LibcResult<T> = std::result::Result<T, Errno>;

/// This trait provides additional functionality for the [`libc::pollfd`] type.
pub trait PollFdExt {
    /// Initialize a new [`libc::pollfd`] struct with the provided file
    /// descriptor and events.
    fn new(fd: RawFd, events: c_short) -> Self;
    /// This function will:
    ///   1. Return Err if any of POLLHUP, POLLERR, or POLLNVAL are set
    ///   2. Return Ok(None) if `event` is not set
    ///   3. Return Ok(Some(handler(fd)))
    fn handle_event<T>(
        &self,
        event: c_short,
        handler: impl FnMut(RawFd) -> T,
    ) -> Result<Option<T>, c_short>;
}

impl PollFdExt for libc::pollfd {
    fn new(fd: RawFd, events: c_short) -> Self {
        libc::pollfd { fd, events, revents: 0 }
    }

    fn handle_event<T>(
        &self,
        event: c_short,
        mut handler: impl FnMut(RawFd) -> T,
    ) -> Result<Option<T>, c_short> {
        const ERROR_MASK: c_short = libc::POLLHUP | libc::POLLERR | libc::POLLNVAL;
        if self.revents & ERROR_MASK != 0 {
            Err(self.revents)
        } else if self.revents & event != event {
            Ok(None)
        } else {
            Ok(Some(handler(self.fd)))
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

/// Convert an integer return value from a `libc` call into a [`LibcResult`]
/// type.  If the return value is -1 the Error type will contain the resulting
/// `errno` value.
fn libc_result_from_int<T: Eq + From<i8>>(retval: T) -> LibcResult<T> {
    if retval == T::from(-1) {
        Err(errno())
    } else {
        Ok(retval)
    }
}

/// Test an integer `libc` return value and returns the auxiliary value if
/// is not equal to -1 and the `errno` value if it is.  The payload thunk is
/// only evaluated when `retval` does not indicate an error.
fn libc_result_from_int_with_payload<T: Eq + From<i8>, P>(
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
fn libc_result_from_int_with_void<T: Eq + From<i8>>(retval: T) -> LibcResult<()> {
    if retval == T::from(-1) {
        Err(errno())
    } else {
        Ok(())
    }
}

/// Converts a pointer into a [`LibcResult`] type, returning an [`Errno`] if it
/// is `null` and converting it into the desired return type if it isn't.
fn libc_result_from_ptr<T, U, C: Fn(NonNull<T>) -> U>(
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
pub enum LoopStatus<T> {
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
    mut handler: impl FnMut(T) -> LoopStatus<U>,
) -> LibcResult<LoopExit<U>> {
    loop {
        match task() {
            Ok(result) => match handler(result) {
                LoopStatus::Continue => continue,
                LoopStatus::Break(val) => return Ok(LoopExit::Early(val)),
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

/// A safe wrapper around [`libc::close`].
///
/// See: `man close`
pub fn close(fd: RawFd) -> LibcResult<c_int> {
    // SAFETY: If the file descriptor is invalid `close()` will return -1 and
    //         we will extract errno and wrap it in a LibcResult.
    libc_result_from_int(unsafe { libc::close(fd) })
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

/// A safe wrapper round [`libc::listen`].
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
    libc_result_from_ptr(unsafe { libc::opendir(path.as_ptr()) }, LibcDir::from_raw)
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
pub fn poll(pollfds: &mut [libc::pollfd], timeout: c_int) -> LibcResult<c_int> {
    // SAFETY: The `pollfds` pointer is valid because it is derived from a
    //         a valid slice reference and the length argument is obtained from
    //         the provided slice.  The return value is checked and wrapped in
    //         a LibcResult.
    libc_result_from_int(unsafe {
        libc::poll(pollfds.as_mut_ptr(), pollfds.len() as libc::nfds_t, timeout)
    })
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
    libc_result_from_ptr(unsafe { libc::readdir(dir.inner.as_ptr()) }, std::convert::identity).ok()
}

/// A safe wrapper around [`libc::readlink`].
///
/// See: `man readlink`
pub fn readlink(path_name: &CStr) -> LibcResult<CStringBuffer> {
    let mut link_buffer: CStringBuffer = CSTRING_BUFFER_INIT;

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
/// See `man recvmsg` and `man readv`
pub fn recvmsg<const BUFFER_SIZE: usize>(fd: RawFd) -> LibcResult<(usize, [u8; BUFFER_SIZE])> {
    let mut buffer = [0; BUFFER_SIZE];

    let mut msghdr = MSGHDR_ZERO_INIT;

    let mut io_vec =
        libc::iovec { iov_base: buffer.as_mut_ptr() as *mut c_void, iov_len: buffer.len() };

    msghdr.msg_iov = &mut io_vec;
    msghdr.msg_iovlen = 1;

    // SAFETY: All of the pointers passed to `libc::recvmsg` point to regions
    //         allocated inside this function.  The return value is checked and
    //         wrapped in a LibcResult.
    retry_eintr!(libc_result_from_int(unsafe { libc::recvmsg(fd, &mut msghdr, 0) }))
        .and_then(|retval| Ok((retval as usize, buffer)))
}

/// A limited wrapper around [`libc::sendmsg`].
///
/// This wrapper only provides functionality for sending a single buffer over a
/// UNIX-domain datagram session socket.
///
/// See: `man sendmsg`
pub fn sendmsg(socket_fd: RawFd, buffer: &[u8]) -> LibcResult<isize> {
    let mut msghdr = MSGHDR_ZERO_INIT;

    let mut io_vec =
        libc::iovec { iov_base: buffer.as_ptr() as *mut c_void, iov_len: buffer.len() };

    msghdr.msg_iov = &mut io_vec;
    msghdr.msg_iovlen = 1;

    // SAFETY: All of the pointers passed to `libc::sendmsg` point to regions
    //         allocated inside this function.  The return value is checked and
    //         wrapped in a LibcResult.
    libc_result_from_int(unsafe { libc::sendmsg(socket_fd, &msghdr, 0) })
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
