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

use anyhow::Result;
use clap::{ArgAction, Parser};

use crate::species::SpeciesRef;

const ENV_VAR_SOCKET: &str = "ZYGOTE_SOCKET";

/// Configuration values used to determine the runtime behavior of a Zygote.
/// Parsing implementations are derived using the `clap` crate.
#[derive(Parser)]
pub struct Config {
    /// Process name for the Zygote
    #[arg(short, long, default_value = "zygote")]
    pub name: String,

    /// A string representing a valid server socket FD or a location to bind a
    /// new socket
    #[arg(long, default_value = "default", value_parser = socket_arg_parser)]
    pub socket: String,

    /// A runtime-defined reference to Species-specific behavior
    /// implementations.
    #[arg(long)]
    pub species: SpeciesRef,

    /// A flag controlling logging levels.
    #[arg(short, long, action = ArgAction::SetTrue)]
    pub verbose: bool,
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
