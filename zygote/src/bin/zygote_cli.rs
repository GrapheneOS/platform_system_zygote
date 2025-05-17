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

use anyhow::bail;
use clap::Parser;
use flatbuffers::FlatBufferBuilder;
use log::error;

use zygote::{config, messages, sys};

fn main() -> anyhow::Result<()> {
    let config = config::Cli::parse();

    logger::init(
        logger::Config::default().with_tag_on_device("zyogte_cli").with_max_level(config.log_level),
    );

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

    let mut builder = FlatBufferBuilder::with_capacity(1024);

    if let Some(msg_args) = build_message_args(config, &mut builder) {
        let message = messages::Message::create(&mut builder, &msg_args);
        builder.finish(message, None);

        sys::sendmsg(client_socket, builder.finished_data())?;
        sys::close(client_socket)?;

        Ok(())
    } else {
        bail!("Invalid message type or arguments.")
    }
}

fn build_message_args(
    config: config::Cli,
    builder: &mut FlatBufferBuilder<'_>,
) -> Option<messages::MessageArgs> {
    match config.command_name.as_str() {
        "exit" => Some(messages::MessageArgs {
            command: Some(messages::Exit::create(builder, &messages::ExitArgs {}).as_union_value()),
            command_type: messages::Command::Exit,
        }),
        "spawn" => {
            if config.command_args.len() == 1 {
                let packed_name = builder.create_string(config.command_args[0].as_str());

                Some(messages::MessageArgs {
                    command: Some(
                        messages::Spawn::create(
                            builder,
                            &messages::SpawnArgs { name: Some(packed_name) },
                        )
                        .as_union_value(),
                    ),
                    command_type: messages::Command::Spawn,
                })
            } else {
                error!(
                    "Invalid number of arguments for spawn command: {}",
                    config.command_args.len()
                );
                None
            }
        }
        "stat" => Some(messages::MessageArgs {
            command: Some(messages::Stat::create(builder, &messages::StatArgs {}).as_union_value()),
            command_type: messages::Command::Stat,
        }),
        invalid => {
            println!("Invalid command received: {}", invalid);
            None
        }
    }
}
