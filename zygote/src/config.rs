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

//! This module provides classes and functions for configuring a Zygote
//! process.

use std::path::Path;

use arrayvec::ArrayVec;
use clap::Parser;

use crate::species::SpeciesRef;
use zygote_core::{log_level_parser, trace_level_parser};
use zygote_messages::SpawnParamsCommon;
use zygote_sys::have_write_permissions;

const SOCKET_DIR_DEV: &str = "/dev/socket";
const SOCKET_DIR_TMP: &str = "/tmp/socket";

// The documentation string for this struct appears as the help message on the
// command line, so the programmer-facing documentation is left as a regular
// comment:
//
// Configuration values used to determine the runtime behavior of a Zygote
// server.  Parsing implementations are derived using the `clap` crate.
//
/// Zygote process server
#[derive(Debug, Parser)]
pub struct Server {
    /// Exec spawning command
    #[arg(long)]
    pub command_fd: Option<String>,

    /// Controls verbosity of logging; defaults to Warn; flag with no argument sets Debug
    #[arg(long, alias("verbose"), short_alias('v'), num_args(0..=1), default_value("2"), default_missing_value("4"), value_parser(log_level_parser))]
    pub log_level: log::LevelFilter,

    /// Controls verbosity of tracing; defaults to Info; flag with no argument sets Trace
    #[arg(long, num_args(0..=1), default_value("3"), default_missing_value("5"), value_parser(trace_level_parser))]
    pub trace_level: tracing::level_filters::LevelFilter,

    /// Process name for the Zygote
    // TODO: Add logic for computing a better default name
    #[arg(long, short, default_value("zygote"))]
    pub name: String,

    /// Effective GID to use when preloading shared libraries
    #[arg(long, value_parser(clap::value_parser!(libc::gid_t).range(0..)))]
    pub preload_gid: Option<libc::gid_t>,

    /// Libraries to be preloaded by the server
    #[arg(long("preload-library"), short('l'), action(clap::ArgAction::Append))]
    pub preload_libraries: Vec<String>,

    /// Effective UID to use when preloading shared libraries
    #[arg(long, value_parser(clap::value_parser!(libc::uid_t).range(0..)))]
    pub preload_uid: Option<libc::uid_t>,

    /// User-provided socket path for the Zygote server.  The
    /// [`Server::socket`] method can be used to access this value
    #[arg(long)]
    socket: Option<String>,

    /// A runtime-defined reference to Species-specific behavior
    /// implementations.
    #[arg(long)]
    pub species: SpeciesRef,

    /*
     * Common Spawn Parameters
     */
    /// GrapheneOS SELinux flags, see frameworks/base/core/java/com/android/internal/os/SELinuxFlags.java 
    #[arg(long)]
    selinux_flags: Option<u64>,
    /// UID to switch to
    #[arg(long)]
    pub uid: Option<i32>,

    /// Main GID to switch to
    #[arg(long)]
    pub gid: Option<i32>,

    /// Name of the new process
    #[arg(long)]
    pub process_name: Option<String>,

    /// Initial scheduling priority for child processes immediately after
    /// forking
    #[arg(long, value_parser(clap::value_parser!(i32).range(-20..20)))]
    pub priority_initial: Option<i32>,

    /// Final scheduling priority for child processes immediately before
    /// entering application code
    #[arg(long, value_parser(clap::value_parser!(i32).range(-20..20)))]
    pub priority_final: Option<i32>,

    /// Secondary group IDs for the new process
    #[arg(long)]
    pub secondary_groups: Vec<libc::gid_t>,

    /// Non-meaningful argument to expand the argument buffer
    #[arg(long)]
    pub arg_buf_padding: Option<String>,
}

impl Server {
    fn get_socket_dir(&self) -> &'static str {
        if have_write_permissions(Path::new(SOCKET_DIR_DEV)).unwrap_or(false) {
            SOCKET_DIR_DEV
        } else {
            SOCKET_DIR_TMP
        }
    }

    /// A string representing a valid server socket FD or a location to bind a
    /// new socket.
    //
    // When left unspecified, the name will be resolved as follows:
    // - environment variable named `ANDROID_SOCKET_<name>` (for AndroidNative)
    // - environment variable named `ZYGOTE_SOCKET_<name>` (for all other species)
    // - the path `/dev/socket/<name>`
    pub(crate) fn socket(&self) -> String {
        self.socket
            .clone()
            .or_else(|| self.species.get_socket_env_var(&self.name))
            .unwrap_or_else(|| format!("{}/{}", self.get_socket_dir(), self.name))
    }

    /// Generate spawn parameters from a Server configuration
    pub(crate) fn to_spawn_params(&self) -> SpawnParamsCommon {
        SpawnParamsCommon {
            selinux_flags: self.selinux_flags,
            uid: self.uid,
            gid: self.gid,
            process_name: self.process_name.clone(),
            priority_initial: self.priority_initial,
            priority_final: self.priority_final,
            cap_effective: None,
            cap_permitted: None,
            cap_inheritable: None,
            cap_bound: None,
            se_info: None,
            secondary_groups: self.secondary_groups.iter().cloned().collect(),
            rlimits: ArrayVec::new(),
        }
    }
}
