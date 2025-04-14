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
    ffi::{c_char, c_int, CStr},
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
const EMPTY_CSTRING_BUFFER: CStringBuffer = [0u8; STRING_BUF_SIZE];

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
pub struct Errno {
    code: c_int,
}

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

/// A Result type that uses Errno for the Error type
pub type LibcResult<T> = std::result::Result<T, Errno>;

// TODO: Consider adding [`closedir`] as a destructor.
/// Wrapper class for a `libc::DIR` pointer
pub struct LibcDir {
    inner: NonNull<libc::DIR>,
}

fn libcdir_from_nonnull(ptr: NonNull<libc::DIR>) -> LibcDir {
    LibcDir { inner: ptr }
}

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

// TODO: Put the payload into a thunk?
/// Test an integer `libc` return value and returns the auxiliary value if
/// is not equal to -1 and the `errno` value if it is.
pub fn libc_result_from_int_with_aux<T: Eq + From<i8>, P>(retval: T, payload: P) -> LibcResult<P> {
    if retval == T::from(-1) {
        Err(errno())
    } else {
        Ok(payload)
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
pub fn libc_result_from_ptr<T, U, C: Fn(NonNull<T>) -> U>(
    retval: *mut T,
    constructor: C,
) -> LibcResult<U> {
    NonNull::new(retval).map(constructor).ok_or(errno())
}

/*
 * Libc helpers
 */

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

/// Select the file-type bits from the `mode` value
pub fn get_file_type(mode: libc::mode_t) -> libc::mode_t {
    mode & libc::S_IFMT
}

/*
 * Libc wrappers
 */

// TODO: Handle EINTR

/// A safe wrapper around [`libc::bind`].
///
/// See: `man bind`
pub fn bind<SockAddrType>(fd: RawFd, sockaddr: &SockAddrType) -> LibcResult<()> {
    // SAFETY: The pointer argument to `libc::bind` is guaranteed to reference
    //         allocated memory and the return value is checked and wrapped in
    //         a LibcResult type.
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
    // SAFETY: If the file descriptor is invalid close() will return -1 and we
    //         will extract errno and wrap it in a Result type.
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
    return libc_result_from_int_with_void(unsafe { libc::dup3(old_fd, new_fd, flags) });

    #[cfg(target_os = "android")]
    // SAFETY: Invalid arguments will result in an error code being returned
    //         that will be wrapped by a LibcResult.
    return libc_result_from_int_with_void(unsafe { libc_fill::dup3(old_fd, new_fd, flags) });
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

    libc_result_from_int_with_aux(
        // SAFETY: The stat buffer pointer is guaranteed to point to a valid
        //         memory address due to its allocation inside this function's
        //         stack.  The return value is checked and wrapped in a
        //         LibcResult type.
        unsafe { libc::fstat(fd, buffer.as_mut_ptr().cast()) },
        // SAFETY: The `libc::fstat` function fully initializes the stat
        //         struct.
        unsafe { buffer.assume_init() },
    )
}

/// A safe wrapper around [`libc::getsockname`].
///
/// See: `man getsockname`
pub fn getsockname(fd: RawFd) -> LibcResult<(libc::sockaddr_un, usize)> {
    let mut addr = std::mem::MaybeUninit::<libc::sockaddr_un>::zeroed();
    let mut addr_len = std::mem::size_of::<libc::sockaddr_un>() as libc::socklen_t;

    // SAFETY: The address buffer pointer is guaranteed to reference valid
    //         memory that has been zeroed out.  This ensures that the any
    //         strings contained in the buffer will be valid null-terminated
    //         C strings.  The return value is checked and wrapped in a
    //         LibcResult type.
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

/// A safe wrapper around [`libc::lseek64`].
///
/// See: `man lseek64`
pub fn lseek64(fd: RawFd, offset: libc::off64_t, whence: c_int) -> LibcResult<libc::off64_t> {
    // SAFETY: The `libc::lseek64` function takes no pointers and the return
    //         value is checked and wrapped in a LibcResult type.  Invalid
    //         argument values will result in a None variant being returned.
    libc_result_from_int(unsafe { libc::lseek64(fd, offset, whence) })
}

/// A safe wrapper around [`libc::open`].
///
/// See: `man open`
pub fn open(path: &CStr, flags: c_int) -> LibcResult<RawFd> {
    // SAFETY: Path points to a valid, null-terminated C string and the return
    //         value is checked and wrapped in a LibcResult type.
    libc_result_from_int(unsafe { libc::open(path.as_ptr().cast(), flags) })
}

/// A safe wrapper around [`libc::opendir`].
///
/// See: `man opendir`
pub fn opendir(path: &CStr) -> LibcResult<LibcDir> {
    // SAFETY: Path points to a valid, null-terminated C string and the return
    //         value is checked and wrapped in a LibcResult type.  The pointer
    //         itself has been verified as non-null and is thus wrapped in the
    //         appropriate helper type.
    libc_result_from_ptr(unsafe { libc::opendir(path.as_ptr()) }, libcdir_from_nonnull)
}

/// A safe wrapper around [`libc::pipe`].
///
/// See: `man pipe`
pub fn pipe() -> LibcResult<(RawFd, RawFd)> {
    let mut pipes = [0; 2];
    // SAFETY: The file descriptor buffer is guaranteed to point to valid memory
    //         and the return value is checked before the resulting pipes are
    //         wrapped in a LibcResult type.
    libc_result_from_int_with_aux(unsafe { libc::pipe(pipes.as_mut_ptr()) }, (pipes[0], pipes[1]))
}

/// A safe wrapper around [`libc::readdir`].
///
/// See: `man readdir`
pub fn readdir(dir: &LibcDir) -> Option<NonNull<libc::dirent>> {
    // SAFETY: The pointer to the directory is guaranteed to be NonNull and the
    //         return value is checked and wrapped in a LibcResult type.  The
    //         pointer itself has been verified as non-null and is thus wrapped
    //         in the appropriate helper type.
    libc_result_from_ptr(unsafe { libc::readdir(dir.inner.as_ptr()) }, std::convert::identity).ok()
}

/// A safe wrapper around [`libc::readlink`].
///
/// See: `man readlink`
pub fn readlink(path_name: &CStr) -> LibcResult<CStringBuffer> {
    let mut link_buffer: CStringBuffer = EMPTY_CSTRING_BUFFER;

    libc_result_from_int_with_aux(
        // SAFETY: The path name is guaranteed to be a valid null-terminated C
        //         string.  The link buffer is guaranteed to be a
        //         zero-initialized buffer and the size_of function is used to
        //         provide the buffer length to the libc call.  The return
        //         value is checked and wrapped in a LibcResult type.
        unsafe {
            libc::readlink(
                path_name.as_ptr(),
                link_buffer.as_mut_ptr().cast(),
                std::mem::size_of::<CStringBuffer>(),
            )
        },
        link_buffer,
    )
}

/// A safe wrapper around [`libc::socket`].
///
/// See: `man socket`
pub fn socket(domain: c_int, ty: c_int, protocol: c_int) -> LibcResult<RawFd> {
    // SAFETY: Invalid argument values will result in a error being returned
    //         by `libc::socket`.  The return value is checked and wrapped in
    //         a LibcResult type.
    libc_result_from_int(unsafe { libc::socket(domain, ty, protocol) })
}
