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

use std::{path::Path, str::FromStr, sync::atomic::AtomicBool};

use anyhow::{bail, Result};
use arrayvec::ArrayVec;
#[cfg(target_os = "android")]
use atrace_tracing_subscriber::AtraceSubscriber;
use clap::Parser;
use tracing_subscriber::{
    layer::{Layer, SubscriberExt},
    util::SubscriberInitExt,
};

use crate::species::SpeciesRef;
use zygote_messages::{
    Message, MessageParser, SpawnParamsCommon, SpawnPayloadParser, ToParcel, TryToParcel,
};
use zygote_sys::have_write_permissions;

const SOCKET_DIR_DEV: &str = "/dev/socket";
const SOCKET_DIR_TMP: &str = "/tmp/socket";

static REPORTING_INITIALIZED: AtomicBool = AtomicBool::new(false);

// The documentation string for this struct appears as the help message on the
// command line, so the programmer-facing documentation is left as a regular
// comment:
//
// Configuration values used by the Zygote command line interface.  This API
// is temporary as the message types evolve.
//
/// Send messages to a Zygote server
#[derive(Debug, Parser)]
pub struct Cli {
    /// Controls verbosity of logging; defaults to Warn; flag with no argument sets Debug
    #[arg(long, alias("verbose"), short_alias('v'), num_args(0..=1), default_value("2"), default_missing_value("4"), value_parser(log_level_parser))]
    pub log_level: log::LevelFilter,

    /// Controls verbosity of tracing; defaults to Info; flag with no argument sets Trace
    #[arg(long, num_args(0..=1), default_value("3"), default_missing_value("5"), value_parser(trace_level_parser))]
    pub trace_level: tracing::level_filters::LevelFilter,

    /// A path to the target Zygote's server socket; Abstract sockets are not
    /// currently supported.
    #[arg(required(true))]
    pub socket: String,

    /// Name of command and arguments to send
    #[command(subcommand)]
    pub command: MessageParser,
}

// The documentation string for this struct appears as the help message on the
// command line, so the programmer-facing documentation is left as a regular
// comment:
//
// Configuration values used by the Zygote launch utility.  This API is
// temporary as the message types evolve.
//
/// Launch a new process using Zygote initialization logic
#[derive(Debug, Parser)]
pub struct Launch {
    /// Controls verbosity of logging; defaults to Warn; flag with no argument sets Debug
    #[arg(long, alias("verbose"), short_alias('v'), num_args(0..=1), default_value("2"), default_missing_value("4"), value_parser(log_level_parser))]
    pub log_level: log::LevelFilter,

    /// Controls verbosity of tracing; defaults to Info; flag with no argument sets Trace
    #[arg(long, num_args(0..=1), default_value("3"), default_missing_value("5"), value_parser(trace_level_parser))]
    pub trace_level: tracing::level_filters::LevelFilter,

    /// The species and species specific arguments
    #[command(subcommand)]
    pub payload: SpawnPayloadParser,

    /*
     * Common Spawn Parameters
     */
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

    /// SELinux context to switch to
    #[arg(long)]
    pub se_info: Option<String>,

    /// Secondary group IDs for the new process
    #[arg(long)]
    pub secondary_groups: Vec<libc::gid_t>,
}

impl Launch {
    fn to_spawn_message(&self) -> Result<Message<'_, '_>> {
        Ok(Message::Spawn {
            common: self.to_spawn_params(),
            payload: self.payload.to_spawn_payload()?,
        })
    }

    fn to_spawn_params(&self) -> SpawnParamsCommon {
        SpawnParamsCommon {
            uid: self.uid,
            gid: self.gid,
            process_name: self.process_name.clone(),
            priority_initial: self.priority_initial,
            priority_final: self.priority_final,
            cap_effective: None,
            cap_permitted: None,
            cap_inheritable: None,
            cap_bound: None,
            se_info: self.se_info.clone(),
            secondary_groups: self.secondary_groups.iter().cloned().collect(),
            rlimits: ArrayVec::new(),
        }
    }
}

impl TryToParcel for Launch {
    fn try_to_parcel<'a>(&self) -> Result<flatbuffers::FlatBufferBuilder<'a>> {
        Ok(self.to_spawn_message()?.to_parcel())
    }
}

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
    /// Controls verbosity of logging; defaults to Warn; flag with no argument sets Debug
    #[arg(long, alias("verbose"), short_alias('v'), num_args(0..=1), default_value("2"), default_missing_value("4"), value_parser(log_level_parser))]
    pub log_level: log::LevelFilter,

    /// Controls verbosity of tracing; defaults to Info; flag with no argument sets Trace
    #[arg(long, num_args(0..=1), default_value("3"), default_missing_value("5"), value_parser(trace_level_parser))]
    pub trace_level: tracing::level_filters::LevelFilter,

    /// Process name for the Zygote
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

    /// A string representing a valid server socket FD or a location to bind a
    /// new socket.
    //
    // When left unspecified, the name will be resolved as follows:
    // - environment variable named `ANDROID_SOCKET_<name>` (for AndroidNative)
    // - environment variable named `ZYGOTE_SOCKET_<name>` (for all other species)
    // - the path `/dev/socket/<name>`
    #[arg(long)]
    pub socket: Option<String>,

    /// A runtime-defined reference to Species-specific behavior
    /// implementations.
    #[arg(long)]
    pub species: SpeciesRef,

    /*
     * Common Spawn Parameters
     */
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
}

impl Server {
    fn get_socket_dir(&self) -> &'static str {
        if have_write_permissions(Path::new(SOCKET_DIR_DEV)).unwrap() {
            SOCKET_DIR_DEV
        } else {
            SOCKET_DIR_TMP
        }
    }

    /// Possibly use configuration and environment data to synthesize a socket path.
    pub fn resolve_socket(&self) -> Option<String> {
        self.socket
            .clone()
            .or_else(|| self.species.get_socket_env_var(&self.name))
            .or_else(|| Some(format!("{}/{}", self.get_socket_dir(), self.name)))
    }

    /// Generate spawn parameters from a Server configuration
    pub fn to_spawn_params(&self) -> SpawnParamsCommon {
        SpawnParamsCommon {
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

/// Parse a string into a [`log::LevelFilter`]
fn log_level_parser(parse_arg: &str) -> Result<log::LevelFilter> {
    log::LevelFilter::from_str(parse_arg).or_else(|_| match parse_arg {
        "0" => Ok(log::LevelFilter::Off),
        "1" => Ok(log::LevelFilter::Error),
        "2" => Ok(log::LevelFilter::Warn),
        "3" => Ok(log::LevelFilter::Info),
        "4" => Ok(log::LevelFilter::Debug),
        "5" => Ok(log::LevelFilter::Trace),
        level => bail!("Invalid log level: {}", level),
    })
}

/// Parse a string into a [`tracing::level_filters::LevelFilter`]
fn trace_level_parser(parse_arg: &str) -> Result<tracing::level_filters::LevelFilter> {
    tracing::level_filters::LevelFilter::from_str(parse_arg).or_else(|_| match parse_arg {
        "0" => Ok(tracing::level_filters::LevelFilter::OFF),
        "1" => Ok(tracing::level_filters::LevelFilter::ERROR),
        "2" => Ok(tracing::level_filters::LevelFilter::WARN),
        "3" => Ok(tracing::level_filters::LevelFilter::INFO),
        "4" => Ok(tracing::level_filters::LevelFilter::DEBUG),
        "5" => Ok(tracing::level_filters::LevelFilter::TRACE),
        level => bail!("Invalid trace level: {}", level),
    })
}

/// Initialize the `log` and `tracing` crates.  This should be called before
/// any log messages or tracing spans are emitted.  May only be called once.
pub fn init_reporting<T: Into<Vec<u8>>>(
    name: T,
    log_level: log::LevelFilter,
    trace_level: tracing::level_filters::LevelFilter,
) -> tracing::subscriber::DefaultGuard {
    if REPORTING_INITIALIZED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        // Given the current design of the Zygote server there is no legitimate
        // reason to call this twice, so we panic to help with debugging.
        panic!("Reporting initialization invoked multiple times");
    } else {
        logger::init(logger::Config::default().with_tag_on_device(name).with_max_level(log_level));

        let fmt_registry = tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .with_span_events(tracing_subscriber::fmt::format::FmtSpan::ACTIVE)
                .with_filter(trace_level),
        );

        // While the Native Zygote has nothing to do with the Dalvik runtime
        // there are a limited number of values in the AtraceTag bitfield and
        // this tag has become a catch-all for system service and runtime
        // related tracing.
        #[cfg(target_os = "android")]
        let fmt_registry = fmt_registry.with(AtraceSubscriber::new(atrace::AtraceTag::Dalvik));

        fmt_registry.set_default()
    }
}

/// Initialize the `log` crate for testing.  May be called multiple times.
pub fn init_reporting_for_testing() {
    if !REPORTING_INITIALIZED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        logger::init(logger::Config::default().with_tag_on_device("zygote_next_test"));
    }
}
