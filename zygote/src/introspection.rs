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

//! This module provides support for querying the `/proc/` file system for
//! information about the current process.

use core::{ffi::CStr, iter::Iterator};
use std::{
    fs::File,
    io::{Read, Write},
    os::fd::RawFd,
    path::Path,
};

use anyhow::{anyhow, Context, Result};
use arrayvec::ArrayVec;

use zygote_sys as sys;

/// Prefix to the /proc/ directory containing information about open file
/// descriptors.
///
/// See: `man proc_pid_fd`
const PROC_SELF_FD_DIR_STR: &str = "/proc/self/fd";
const PROC_SELF_FD_DIR_CSTR: &std::ffi::CStr = c"/proc/self/fd";

#[cfg(test)]
const PROC_SELF_EXE: &str = "/proc/self/exe";

/// Panic if the [`Result`] is [`Err`].
#[macro_export]
macro_rules! assert_ok {
    ($result:expr) => {
        match $result {
            Ok(_) => {}
            Err(err_val) => {
                panic!("Unexpected error: {:?}", err_val);
            }
        }
    };
}

/// Panic if the [`Result`] is [`Err`].
#[macro_export]
macro_rules! debug_assert_ok {
    ($result:expr) => {
        if cfg!(debug_assertions) {
            assert_ok!($result);
        }
    };
}

/// Panic if there is more than one thread in the current process.
#[track_caller]
pub fn assert_single_threaded() {
    assert_eq!(ProcStat::get().unwrap().num_threads, 1);
}

/// Panic if there is more than one thread in the current process.
#[track_caller]
pub fn debug_assert_single_threaded() {
    debug_assert_eq!(ProcStat::get().unwrap().num_threads, 1);
}

/// Panic if a given file descriptor *IS NOT* open
#[cfg(feature = "test")]
#[track_caller]
pub fn assert_fd_open(fd: RawFd) {
    assert!(get_proc_fd_path(fd).exists());
}

/// Panic if a given file descriptor *IS* open
#[cfg(feature = "test")]
#[track_caller]
pub fn assert_fd_closed(fd: RawFd) {
    assert!(!get_proc_fd_path(fd).exists());
}

/// Query procfs for the path of the current executable
#[cfg(test)]
pub(crate) fn get_executable_path() -> std::io::Result<std::path::PathBuf> {
    std::fs::read_link(PROC_SELF_EXE)
}

/// Read the contents of the /proc/self/fd directory to get a list of open
/// file descriptors.
pub(crate) struct ProcFdIterator {
    dir: sys::LibcDir,
    dir_fd: RawFd,
}

impl ProcFdIterator {
    pub(crate) fn new() -> Result<Self> {
        assert!(Path::new(PROC_SELF_FD_DIR_CSTR.to_str()?).exists());
        // The `std::fs::read_dir` function will open two file descriptors when
        // called and there is no way to gain access to their values.  Manually
        // opening and iterating over the directory allows us to avoid adding
        // transient file descriptor to the registry.
        let dir = sys::opendir(PROC_SELF_FD_DIR_CSTR)?;
        let dir_fd = sys::dirfd(&dir)?;
        Ok(Self { dir, dir_fd })
    }

    fn next_fd(&self, dir_entry: &CStr) -> Result<Option<RawFd>> {
        if !dir_entry.to_bytes().first().is_some_and(|c| c.is_ascii_digit()) {
            return Ok(None);
        }
        let open_fd =
            dir_entry.to_str()?.parse().context("Failed to parse proc file descriptor entry")?;
        Ok((self.dir_fd != open_fd).then_some(open_fd))
    }
}

impl Iterator for ProcFdIterator {
    type Item = Result<RawFd>;

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(dir_entry) = sys::readdir(&self.dir) {
            // TODO: Provide a safe abstraction via the sys module
            // SAFETY: Libc guarantees that the dir_entry->d_name member contains a valid C string.
            let dir_entry = unsafe { CStr::from_ptr((*dir_entry.as_ptr()).d_name.as_ptr()) };
            if let Some(r) = self.next_fd(dir_entry).transpose() {
                return Some(r);
            }
        }
        None
    }
}

/// Read file descriptor information from procfs into a CStringBuffer.
pub(crate) fn get_proc_fd_link_info(fd: RawFd) -> Result<sys::CStringBuffer> {
    let mut path_cstr_buff = ArrayVec::<u8, { sys::BUFFER_SIZE_STRINGS }>::new();
    write!(path_cstr_buff, "{PROC_SELF_FD_DIR_STR}/{fd}\0")?;
    let path_cstr = CStr::from_bytes_until_nul(path_cstr_buff.as_slice()).unwrap();

    sys::readlink(path_cstr).with_context(|| format!("Unable to read procfs symlink for fd {fd}"))
}

/// Construct PathBuf pointing to an entry in /proc/self/fd.  The entry may or
/// may not exist.
pub fn get_proc_fd_path(fd: RawFd) -> std::path::PathBuf {
    std::path::Path::new(PROC_SELF_FD_DIR_STR).join(fd.to_string())
}

/// Information gathered from /proc/self/stat.
///
/// See: `man proc_pid_stat`
pub struct ProcStat {
    /// Process ID
    pub pid: u32,
    /// Process group ID
    pub pgrp: u32,
    /// Number of minor faults
    pub minflt: u64,
    /// Number of minor faults in waited-for children
    pub cminflt: u64,
    /// Number of major faults
    pub majflt: u64,
    /// Number of major faults in waited-for children
    pub cmajflt: u64,
    /// User time
    pub utime: u64,
    /// System time
    pub stime: u64,
    /// Number of threads in the process
    pub num_threads: u64,
    /// Virtual memory size in bytes
    pub vsize: u64,
    /// Resident set size in number of pages
    pub rss: u64,
}

impl ProcStat {
    const PROC_STAT_BUFFER_SIZE: usize = 512;
    const PROC_STAT_PATH_STR: &str = "/proc/self/stat";
    const PROC_STAT_NUM_ENTRIES: usize = 52;

    /// Query procfs for statistics on the current process.
    #[rustfmt::skip]
    #[tracing::instrument(level = "trace")]
    pub fn get() -> Result<Self> {
        let mut proc_file = File::open(Self::PROC_STAT_PATH_STR)?;
        let mut proc_buf: [u8; Self::PROC_STAT_BUFFER_SIZE] = [0; Self::PROC_STAT_BUFFER_SIZE];

        let read_len = proc_file.read(&mut proc_buf)?;
        let text = String::from_utf8_lossy(&proc_buf[0..read_len]);

        let (text_left, text_right) = text.rsplit_once(") ").ok_or(anyhow!("Malformed /proc/self/stat output"))?;
        let (pid_str, _) = text_left.split_once(" (").ok_or(anyhow!("Malformed /proc/self/stat output"))?;
        let rest = text_right
            .split_whitespace()
            .collect::<ArrayVec<&str, { Self::PROC_STAT_NUM_ENTRIES }>>();

        assert_eq!(rest.len(), Self::PROC_STAT_NUM_ENTRIES - 2);

        Ok(Self {
            pid:         pid_str.parse()?,
            pgrp:        rest[ 2].parse()?,
            minflt:      rest[ 7].parse()?,
            cminflt:     rest[ 8].parse()?,
            majflt:      rest[ 9].parse()?,
            cmajflt:     rest[10].parse()?,
            utime:       rest[11].parse()?,
            stime:       rest[12].parse()?,
            num_threads: rest[17].parse()?,
            vsize:       rest[20].parse()?,
            rss:         rest[21].parse()?,
        })
    }
}

#[cfg(test)]
mod test {
    use crate::test::manage_test;

    #[test]
    fn test_open_file_descriptors() {
        manage_test(|| {
            let mut it = super::ProcFdIterator::new().unwrap();
            assert!(matches!(it.next(), Some(Ok(0))));
            assert!(matches!(it.next(), Some(Ok(1))));
            assert!(matches!(it.next(), Some(Ok(2))));
            assert!(it.next().is_none());
        });
    }
}
