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

use core::{debug_assert, iter::Iterator};
use std::{fs::File, io::Read, os::fd::RawFd, path::Path};

use anyhow::{anyhow, Context, Result};
use arrayvec::ArrayVec;

use crate::sys;

pub const PROC_SELF_FDS_PATH_CSTR: &std::ffi::CStr = c"/proc/self/fd";
pub const PROC_SELF_EXE_PATH_STR: &str = "/proc/self/exe";

#[track_caller]
pub fn assert_single_threaded() {
    assert_eq!(ProcStat::get().unwrap().num_threads, 1);
}

// TODO: Update to return an iterator
pub(crate) fn get_open_file_descriptors() -> Result<Vec<RawFd>> {
    let proc_self_fds_path = Path::new(PROC_SELF_FDS_PATH_CSTR.to_str()?);
    assert!(proc_self_fds_path.exists());

    // The `std::fs::read_dir` function will open two file descriptors when
    // called and there is no way to gain access to their values.  Manually
    // opening and iterating over the directory allows us to avoid adding
    // transient file descriptor to the registry.
    let proc_self_fds_dir = sys::opendir(PROC_SELF_FDS_PATH_CSTR)?;
    let proc_self_fds_fd = sys::dirfd(&proc_self_fds_dir)?;

    let mut fd_vec = Vec::<RawFd>::new();
    while let Some(dir_entry) = sys::readdir(&proc_self_fds_dir) {
        // SAFETY: Libc guarantees that the dir_entry->d_name member contains
        //         a valid C string.
        let dir_entry_str =
            unsafe { std::ffi::CStr::from_ptr((*dir_entry.as_ptr()).d_name.as_ptr()) };

        if !dir_entry_str.is_empty() {
            let first_char = dir_entry_str
                .to_bytes()
                .first()
                .ok_or(anyhow!("Failed to read directory entry name"))?;

            if first_char.is_ascii_digit() {
                let open_fd = dir_entry_str
                    .to_str()?
                    .parse()
                    .context("Failed to parse proc file descriptor entry")?;

                if proc_self_fds_fd != open_fd {
                    fd_vec.push(open_fd);
                }
            }
        }
    }

    sys::closedir(proc_self_fds_dir)?;

    debug_assert!(fd_vec.is_sorted());

    Ok(fd_vec)
}

pub struct ProcStat {
    pub pid: u32,
    pub pgrp: u32,
    pub minflt: u64,
    pub cminflt: u64,
    pub majflt: u64,
    pub cmajflt: u64,
    pub utime: u64,
    pub stime: u64,
    pub num_threads: u64,
    pub vsize: u64,
    pub rss: u64,
}

// See `man proc_pid_stat` for details.
impl ProcStat {
    const PROC_STAT_BUFFER_SIZE: usize = 512;
    const PROC_STAT_PATH_STR: &str = "/proc/self/stat";
    const PROC_STAT_NUM_ENTRIES: usize = 52;

    #[rustfmt::skip]
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
            assert_eq!(super::get_open_file_descriptors().unwrap(), vec![0, 1, 2]);
        });
    }
}
