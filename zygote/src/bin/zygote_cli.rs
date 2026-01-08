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

//! Command line interface tool for issuing commands to a Zygote

use std::os::fd::RawFd;

use anyhow::{bail, Result};
use clap::Parser;
use flatbuffers::FlatBufferBuilder;
use log::{error, info};

use zygote_core::{init_reporting, log_level_parser, trace_level_parser};
use zygote_messages::{self as messages, FromParcel, Message, MessageParser, TryToParcel};
use zygote_sys as sys;

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

fn main() -> Result<()> {
    let config = Cli::parse();
    let _trace_guard = init_reporting("zygote_cli", config.log_level, config.trace_level);

    let builder = config.command.try_to_parcel()?;

    let client_socket = get_client_socket(&config.socket)?;
    handle_command_transaction(builder, client_socket)?;
    sys::close(client_socket)?;

    Ok(())
}

fn get_client_socket(path_str: &String) -> Result<RawFd> {
    let socket_path = std::path::Path::new(path_str);
    if socket_path.exists() {
        let client_socket = sys::socket(libc::AF_UNIX, libc::SOCK_SEQPACKET, 0)?;
        let socket_addr = if let Some(abs_socket_addr) = path_str.strip_prefix("@") {
            sys::abstract_socket_address(abs_socket_addr, libc::AF_UNIX as libc::sa_family_t)
        } else {
            sys::bound_socket_address(path_str, libc::AF_UNIX as libc::sa_family_t)
        };

        sys::connect(client_socket, &socket_addr)?;

        Ok(client_socket)
    } else {
        bail!("Zygote server socket path does not exist")
    }
}

fn handle_command_transaction(builder: FlatBufferBuilder, client_socket: RawFd) -> Result<()> {
    sys::sendmsg(client_socket, builder.finished_data())?;

    let (_, response_buffer) = sys::recvmsg::<{ messages::MESSAGE_BUFFER_SIZE }>(client_socket)?;

    let response = Message::try_from_parcel(&response_buffer)?;
    match response {
        Message::AckResponse => {
            info!("Message acknowledged");
        }
        Message::IdentityQueryResponse { .. } => {
            info!("Identity query successful: {response:?}");
            println!("{response:?}");
        }
        Message::SpawnResponse { pid } => {
            info!("Spawn successful; New process pid: {pid}");
        }
        Message::StatResponse { .. } => {
            info!("Stat response: {response:?}");
            println!("{response:?}");
        }
        msg => {
            error!("Unexpected response message: {msg:?}");
        }
    }

    Ok(())
}
