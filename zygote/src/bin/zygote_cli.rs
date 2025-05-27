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

use anyhow::{anyhow, bail, Context};
use clap::Parser;

use zygote::{
    config,
    messages::{self, build_message, Message},
    sys,
};

fn main() -> anyhow::Result<()> {
    let config = config::Cli::parse();

    logger::init(
        logger::Config::default().with_tag_on_device("zygote_cli").with_max_level(config.log_level),
    );

    let finished_builder = build_message(&config.command_name, &config.command_args)
        .context("Invalid message type or arguments.")?;

    let socket_path = std::path::Path::new(&config.socket);
    let client_socket = if socket_path.exists() {
        let client_socket = sys::socket(libc::AF_UNIX, libc::SOCK_SEQPACKET, 0)?;
        let socket_addr =
            sys::bound_socket_address(&config.socket, libc::AF_UNIX as libc::sa_family_t);

        sys::connect(client_socket, &socket_addr)?;

        client_socket
    } else {
        bail!("Zygote server socket path does not exist");
    };

    sys::sendmsg(client_socket, finished_builder.finished_data())?;

    let (_, response) = sys::recvmsg::<{ messages::MESSAGE_BUFFER_SIZE }>(client_socket)?;

    let parcel = flatbuffers::root::<messages::Parcel>(&response)?;
    match parcel.message_type() {
        Message::Ack => {
            log::info!("Message acknowledged");
        }
        Message::SpawnResponse => {
            let spawn_response = parcel
                .message_as_spawn_response()
                .ok_or(anyhow!("Could not unpack SpawnResponse"))?;
            log::info!("Spawn successful; New process pid: {}", spawn_response.pid());
        }
        Message(tag) => {
            log::error!("Unexpected response message type: {}", tag);
        }
    }

    sys::close(client_socket)?;

    Ok(())
}
