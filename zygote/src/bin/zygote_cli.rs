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

use zygote_sys as sys;

use zygote::{
    config,
    messages::{self, FromParcel, Message, TryToParcel},
};

fn main() -> Result<()> {
    let config = config::Cli::parse();

    logger::init(
        logger::Config::default().with_tag_on_device("zygote_cli").with_max_level(config.log_level),
    );

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
