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

use crate::species::SpeciesRef;

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
    #[arg(long, short, required(true))]
    pub socket: String,

    /// Name of command to send; accepted values: exit, spawn, stat
    #[arg(required(true))]
    pub command_name: String,

    /// Additional arguments that might be used by the command
    pub command_args: Vec<String>,
}

/// Configuration values used to determine the runtime behavior of a Zygote
/// server.  Parsing implementations are derived using the `clap` crate.
#[derive(Parser)]
pub struct Server {
    /// Process name for the Zygote
    #[arg(short, long, default_value("zygote"))]
    pub name: String,

    /// Controls verbosity of logging; defaults to Warn
    #[arg(long, alias("verbose"), short_alias('v'), num_args(0..=1), default_value("2"), default_missing_value("4"), value_parser(log_level_parser))]
    pub log_level: LevelFilter,

    /// A string representing a valid server socket FD or a location to bind a
    /// new socket
    #[arg(long, default_value = "default", value_parser(socket_arg_parser))]
    pub socket: String,

    /// A runtime-defined reference to Species-specific behavior
    /// implementations.
    #[arg(long)]
    pub species: SpeciesRef,
}

fn log_level_parser(parse_arg: &str) -> Result<LevelFilter> {
    match parse_arg.parse::<usize>()? {
        0 => Ok(LevelFilter::Off),
        1 => Ok(LevelFilter::Error),
        2 => Ok(LevelFilter::Warn),
        3 => Ok(LevelFilter::Info),
        4 => Ok(LevelFilter::Debug),
        5 => Ok(LevelFilter::Trace),
        _ => bail!("Invalid log level"),
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
