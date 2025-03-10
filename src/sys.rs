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
use arrayvec::ArrayVec;
use static_assertions::const_assert;
use zerocopy::FromBytes;

#[cfg(target_os = "android")]
use crate::libc_fill;

pub const STRING_BUF_SIZE: usize = 512;
const_assert!(
    STRING_BUF_SIZE
        >= std::mem::size_of::<libc::sockaddr_un>() - offset_of!(libc::sockaddr_un, sun_path)
);

// TODO: Consider making this an ArrayVec
pub type CStringBuffer = [u8; STRING_BUF_SIZE];
const EMPTY_CSTRING_BUFFER: CStringBuffer = [0u8; STRING_BUF_SIZE];

pub trait AsCStr {
    fn as_cstr(&self) -> &CStr;
}

impl AsCStr for CStringBuffer {
    fn as_cstr(&self) -> &CStr {
        CStr::from_bytes_until_nul(self).unwrap()
    }
}

pub struct Errno {
    code: c_int,
}

pub(crate) fn errno() -> Errno {
    Errno {
        // SAFETY: Reads from the thread's errno address should never fail
        #[cfg(not(target_os = "android"))]
        code: unsafe { *libc::__errno_location() },

        #[cfg(target_os = "android")]
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

pub type LibcResult<T> = std::result::Result<T, Errno>;

pub fn libc_result_from_int<T: Eq + From<i8>>(retval: T) -> LibcResult<T> {
    if retval == T::from(-1) {
        Err(errno())
    } else {
        Ok(retval)
    }
}

// TODO: Put the payload into a thunk?
pub fn libc_result_from_int_with_aux<T: Eq + From<i8>, P>(retval: T, payload: P) -> LibcResult<P> {
    if retval == T::from(-1) {
        Err(errno())
    } else {
        Ok(payload)
    }
}

pub fn libc_result_from_ptr<T>(retval: *mut T) -> LibcResult<NonNull<T>> {
    NonNull::new(retval).ok_or(errno())
}

pub fn libc_result_from_ptr_with_constructor<T, P>(
    retval: *mut T,
    constructor: impl Fn(*mut T) -> P,
) -> LibcResult<P> {
    if retval.is_null() {
        Err(errno())
    } else {
        Ok(constructor(retval))
    }
}

pub fn libc_result_from_int_with_void<T: Eq + From<i8>>(retval: T) -> LibcResult<()> {
    if retval == T::from(-1) {
        Err(errno())
    } else {
        Ok(())
    }
}

/*
 * Libc Helpers
 */

pub fn abstract_socket_address(name: &str, family: libc::sa_family_t) -> libc::sockaddr_un {
    let mut socket_addr = libc::sockaddr_un { sun_family: family, sun_path: [0; 108] };
    let name_view = &name.as_bytes()[0..std::cmp::min(name.len(), socket_addr.sun_path.len() - 1)];

    // Abstract socket names begin with a null character, so start copying the
    // name at index 1.
    socket_addr.sun_path[1..name_view.len() + 1]
        .copy_from_slice(<[c_char]>::ref_from_bytes(name_view).unwrap());

    socket_addr
}

pub fn bound_socket_address(path: &str, family: libc::sa_family_t) -> libc::sockaddr_un {
    let mut socket_addr = libc::sockaddr_un { sun_family: family, sun_path: [0; 108] };
    let name_view = &path.as_bytes()[0..std::cmp::min(path.len(), socket_addr.sun_path.len() - 1)];

    socket_addr.sun_path[0..name_view.len()]
        .copy_from_slice(<[c_char]>::ref_from_bytes(name_view).unwrap());

    socket_addr
}

/*
 * Libc wrappers
 */

// TODO: Handle EINTR

pub fn bind<SockAddrType>(fd: RawFd, sockaddr: &SockAddrType) -> LibcResult<()> {
    libc_result_from_int_with_void(unsafe {
        libc::bind(
            fd,
            (sockaddr as *const SockAddrType) as *const libc::sockaddr,
            std::mem::size_of::<SockAddrType>() as libc::socklen_t,
        )
    })
}

pub fn close(fd: RawFd) -> LibcResult<c_int> {
    // SAFETY: If the file descriptor is invalid close() will return -1 and we
    //         will extract errno and wrap it in a Result type.
    libc_result_from_int(unsafe { libc::close(fd) })
}

pub fn closedir(dir: NonNull<libc::DIR>) -> LibcResult<c_int> {
    libc_result_from_int(unsafe { libc::closedir(dir.as_ptr()) })
}

pub fn dirfd(dir: &NonNull<libc::DIR>) -> LibcResult<RawFd> {
    libc_result_from_int(unsafe { libc::dirfd(dir.as_ptr()) })
}

pub fn dup3(oldfd: RawFd, newfd: RawFd, flags: c_int) -> LibcResult<c_int> {
    #[cfg(not(target_os = "android"))]
    return libc_result_from_int(unsafe { libc::dup3(oldfd, newfd, flags) });

    #[cfg(target_os = "android")]
    return libc_result_from_int(unsafe { libc_fill::dup3(oldfd, newfd, flags) });
}

pub fn fcntl_getfd(fd: RawFd) -> LibcResult<c_int> {
    libc_result_from_int(unsafe { libc::fcntl(fd, libc::F_GETFD) })
}

pub fn fcntl_getfl(fd: RawFd) -> LibcResult<c_int> {
    libc_result_from_int(unsafe { libc::fcntl(fd, libc::F_GETFL) })
}

pub fn fcntl_setfd(fd: RawFd, flags: c_int) -> LibcResult<()> {
    libc_result_from_int_with_void(unsafe { libc::fcntl(fd, libc::F_SETFD, flags) })
}

pub fn fcntl_setfl(fd: RawFd, flags: c_int) -> LibcResult<()> {
    libc_result_from_int_with_void(unsafe { libc::fcntl(fd, libc::F_SETFL, flags) })
}

pub fn fstat(fd: RawFd) -> LibcResult<libc::stat> {
    let mut buffer = MaybeUninit::<libc::stat>::uninit();

    libc_result_from_int_with_aux(unsafe { libc::fstat(fd, buffer.as_mut_ptr().cast()) }, unsafe {
        buffer.assume_init()
    })
}

pub fn getsockname(fd: RawFd) -> LibcResult<(libc::sockaddr_un, usize)> {
    let mut addr = std::mem::MaybeUninit::<libc::sockaddr_un>::zeroed();
    let mut addr_len = std::mem::size_of::<libc::sockaddr_un>() as libc::socklen_t;

    libc_result_from_int(unsafe {
        libc::getsockname(fd, addr.as_mut_ptr().cast(), &mut addr_len)
    })?;

    debug_assert!(addr_len <= std::mem::size_of::<libc::sockaddr_un>() as libc::socklen_t);

    if addr_len as usize == std::mem::size_of::<libc::sa_family_t>() {
        return Err(Errno { code: 0 });
    }

    let addr = unsafe { addr.assume_init() };

    if addr.sun_family != libc::AF_UNIX as u16 {
        return Err(Errno { code: 0 });
    }

    let path_len = addr_len as usize - offset_of!(libc::sockaddr_un, sun_path);

    Ok((addr, path_len))
}

pub fn lseek64(fd: RawFd, offset: libc::off64_t, whence: c_int) -> LibcResult<libc::off64_t> {
    libc_result_from_int(unsafe { libc::lseek64(fd, offset, whence) })
}

pub fn open(path: &[u8], flags: c_int) -> LibcResult<RawFd> {
    libc_result_from_int(unsafe { libc::open(path.as_ptr().cast(), flags) })
}

pub fn opendir(path: &CStr) -> LibcResult<NonNull<libc::DIR>> {
    libc_result_from_ptr(unsafe { libc::opendir(path.as_ptr()) })
}

pub fn pipe() -> LibcResult<(RawFd, RawFd)> {
    let mut pipes = [0; 2];
    libc_result_from_int_with_aux(unsafe { libc::pipe(pipes.as_mut_ptr()) }, (pipes[0], pipes[1]))
}

pub fn readdir(dir: &NonNull<libc::DIR>) -> Option<NonNull<libc::dirent>> {
    libc_result_from_ptr(unsafe { libc::readdir(dir.as_ptr()) }).ok()
}

pub fn readlink(path_name: &mut ArrayVec<u8, STRING_BUF_SIZE>) -> LibcResult<CStringBuffer> {
    let mut link_buffer: CStringBuffer = EMPTY_CSTRING_BUFFER;

    libc_result_from_int_with_aux(
        unsafe {
            libc::readlink(
                path_name.as_slice().as_ptr().cast(),
                link_buffer.as_mut_ptr().cast(),
                std::mem::size_of::<CStringBuffer>(),
            )
        },
        link_buffer,
    )
}

pub fn socket(domain: c_int, ty: c_int, protocol: c_int) -> LibcResult<RawFd> {
    libc_result_from_int(unsafe { libc::socket(domain, ty, protocol) })
}
