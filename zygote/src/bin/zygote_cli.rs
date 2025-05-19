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

use anyhow::{bail, Context, Result};
use clap::Parser;
use flatbuffers::FlatBufferBuilder;

use zygote::{
    config,
    messages::{self, ToFlatBuffer},
    sys,
};

fn main() -> anyhow::Result<()> {
    let config = config::Cli::parse();

    logger::init(
        logger::Config::default().with_tag_on_device("zygote_cli").with_max_level(config.log_level),
    );

    let finished_builder = build_message(&config).context("Invalid message type or arguments.")?;

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
    sys::close(client_socket)?;

    Ok(())
}

fn build_message(config: &config::Cli) -> Result<FlatBufferBuilder> {
    let extra_args_iter = std::iter::once(&config.command_name).chain(config.command_args.iter());

    match config.command_name.as_str() {
        "Exit" => Ok(messages::ExitParser::try_parse_from(extra_args_iter)?.build()),
        "SpawnAndroidNative" => {
            Ok(messages::SpawnAndroidNativeParser::try_parse_from(extra_args_iter)?.build())
        }
        "SpawnLibApp" => Ok(messages::SpawnLibAppParser::try_parse_from(extra_args_iter)?.build()),
        "SpawnMock" => Ok(messages::SpawnMockParser::try_parse_from(extra_args_iter)?.build()),
        "Stat" => Ok(messages::StatParser::try_parse_from(extra_args_iter)?.build()),
        _ => {
            bail!("Invalid command type: {}", config.command_name)
        }
    }
}
