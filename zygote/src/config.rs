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

use anyhow::{bail, Result};
use clap::Parser;
use log::LevelFilter;

use crate::{
    messages::{
        Message, MessageParser, SpawnParamsCommon, SpawnPayloadParser, ToParcel, TryToParcel,
    },
    species::SpeciesRef,
};

const ENV_VAR_SOCKET: &str = "ZYGOTE_SOCKET";

/// Configuration values used by the Zygote command line interface.  This API
/// is temporary as the message types evolve.
#[derive(Parser)]
pub struct Cli {
    /// Controls verbosity of logging; defaults to Warn
    #[arg(long, alias("verbose"), short_alias('v'), num_args(0..=1), default_value("2"), default_missing_value("4"), value_parser(log_level_parser))]
    pub log_level: LevelFilter,

    /// A path to the target Zygote's server socket; Abstract sockets are not
    /// currently supported.
    #[arg(required(true))]
    pub socket: String,

    /// Name of command and arguments to send
    #[command(subcommand)]
    pub command: MessageParser,
}

/// Configuration values used by the Zygote launch utility.  This API is
/// temporary as the message types evolve.
#[derive(Parser)]
pub struct Launch {
    /// Controls verbosity of logging; defaults to Warn
    #[arg(long, alias("verbose"), short_alias('v'), num_args(0..=1), default_value("2"), default_missing_value("4"), value_parser(log_level_parser))]
    pub log_level: LevelFilter,

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

    /// Initial scheduling priority for child processes immediately after
    /// forking
    #[arg(long, value_parser(clap::value_parser!(i32).range(-20..20)))]
    pub priority_initial: Option<i32>,

    /// Final scheduling priority for child processes immediately before
    /// entering application code
    #[arg(long, value_parser(clap::value_parser!(i32).range(-20..20)))]
    pub priority_final: Option<i32>,
}

impl Launch {
    fn to_spawn_message(&self) -> Result<Message<'_, '_>> {
        Ok(Message::Spawn {
            params: self.to_spawn_params(),
            payload: self.payload.to_spawn_payload()?,
        })
    }

    fn to_spawn_params(&self) -> SpawnParamsCommon {
        SpawnParamsCommon {
            uid: self.uid,
            gid: self.gid,
            priority_initial: self.priority_initial,
            priority_final: self.priority_final,
            cap_effective: None,
            cap_permitted: None,
            cap_inheritable: None,
            cap_bound: None,
        }
    }
}

impl TryToParcel for Launch {
    fn try_to_parcel<'a>(&self) -> Result<flatbuffers::FlatBufferBuilder<'a>> {
        Ok(self.to_spawn_message()?.to_parcel())
    }
}

/// Configuration values used to determine the runtime behavior of a Zygote
/// server.  Parsing implementations are derived using the `clap` crate.
#[derive(Parser)]
pub struct Server {
    /// Controls verbosity of logging; defaults to Warn
    #[arg(long, alias("verbose"), short_alias('v'), num_args(0..=1), default_value("2"), default_missing_value("4"), value_parser(log_level_parser))]
    pub log_level: LevelFilter,

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
    /// new socket
    #[arg(long, default_value = "default", value_parser(socket_arg_parser))]
    pub socket: String,

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

    /// Initial scheduling priority for child processes immediately after
    /// forking
    #[arg(long, value_parser(clap::value_parser!(i32).range(-20..20)))]
    pub priority_initial: Option<i32>,

    /// Final scheduling priority for child processes immediately before
    /// entering application code
    #[arg(long, value_parser(clap::value_parser!(i32).range(-20..20)))]
    pub priority_final: Option<i32>,
}

impl Server {
    /// Generate spawn parameters from a Server configuration
    pub fn to_spawn_params(&self) -> SpawnParamsCommon {
        SpawnParamsCommon {
            uid: self.uid,
            gid: self.gid,
            priority_initial: self.priority_initial,
            priority_final: self.priority_final,
            cap_effective: None,
            cap_permitted: None,
            cap_inheritable: None,
            cap_bound: None,
        }
    }
}

/// Parse a string into a [`log::LevelFilter`]
pub fn log_level_parser(parse_arg: &str) -> Result<LevelFilter> {
    match parse_arg {
        "0" => Ok(LevelFilter::Off),
        "1" => Ok(LevelFilter::Error),
        "2" => Ok(LevelFilter::Warn),
        "3" => Ok(LevelFilter::Info),
        "4" => Ok(LevelFilter::Debug),
        "5" => Ok(LevelFilter::Trace),
        level => bail!("Invalid log level: {}", level),
    }
}

fn socket_arg_parser(parse_arg: &str) -> Result<String> {
    if parse_arg == "default" {
        if let Ok(env_arg) = std::env::var(ENV_VAR_SOCKET) {
            Ok(env_arg)
        } else {
            Ok("".to_owned())
        }
    } else {
        Ok(parse_arg.to_owned())
    }
}
