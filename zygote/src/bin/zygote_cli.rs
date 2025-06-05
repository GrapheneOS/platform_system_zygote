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

use anyhow::{anyhow, bail, Result};
use clap::{Arg, Command};

use flatbuffers::FlatBufferBuilder;
use zygote::{
    config::log_level_parser,
    messages::{self, Message, ToParcel},
    sys,
};

fn main() -> Result<()> {
    let cli_matches = build_cli_parser().get_matches();

    logger::init(
        logger::Config::default()
            .with_tag_on_device("zygote_cli")
            .with_max_level(*cli_matches.get_one::<log::LevelFilter>("log-level").unwrap()),
    );

    let builder = build_command_parcel(&cli_matches)?;

    let client_socket = get_client_socket(cli_matches.get_one::<String>("socket").unwrap())?;
    handle_command_transaction(builder, client_socket)?;
    sys::close(client_socket)?;

    Ok(())
}

fn build_cli_parser() -> Command {
    Command::new("zygote_cli")
        .about("A command line interface for interacting with a Zygote server.")
        .arg(Arg::new("socket").help("Path to the Zygote server socket").required(true))
        .arg(
            Arg::new("log-level")
                .long("log-level")
                .help("Set the logging level")
                .num_args(0..=1)
                .default_value("2")
                .default_missing_value("4")
                .value_parser(log_level_parser)
                .alias("verbose")
                .short_alias('v'),
        )
        .subcommand(Command::new("Exit"))
        .subcommand(Command::new("IdentityQuery"))
        .subcommand(
            Command::new("Spawn")
                .arg(
                    Arg::new("uid")
                        .long("uid")
                        .short('u')
                        .help("UID for the new process")
                        .default_value("-1")
                        .value_parser(clap::value_parser!(i32)),
                )
                .arg(
                    Arg::new("gid")
                        .long("gid")
                        .short('g')
                        .help("GID for the new process")
                        .default_value("-1")
                        .value_parser(clap::value_parser!(i32)),
                )
                .subcommand(Command::new("SpawnAndroidNative").arg(
                    Arg::new("package").help("Package name for the new process").required(true),
                ))
                .subcommand(
                    Command::new("SpawnLibApp")
                        .arg(Arg::new("path").help("Path to the library").required(true))
                        .arg(
                            Arg::new("args")
                                .help("Arguments for the library")
                                .num_args(clap::builder::ValueRange::new(0..))
                                .trailing_var_arg(true),
                        ),
                )
                .subcommand(
                    Command::new("SpawnMock")
                        .arg(Arg::new("name").help("Name for the mock process").required(true)),
                ),
        )
        .subcommand(Command::new("Stat"))
}

fn build_command_parcel<'builder>(
    cli_matches: &clap::ArgMatches,
) -> Result<flatbuffers::FlatBufferBuilder<'builder>> {
    match cli_matches.subcommand() {
        Some(("Exit", _)) => Ok(messages::ExitPacker {}.to_parcel()),
        Some(("IdentityQuery", _)) => Ok(messages::IdentityQueryPacker {}.to_parcel()),
        Some(("Spawn", args)) => {
            let uid = args.get_one::<i32>("uid").unwrap_or(&-1);
            let gid = args.get_one::<i32>("gid").unwrap_or(&-1);

            match args.subcommand() {
                Some(("SpawnAndroidNative", args)) => {
                    let package = args.get_one::<String>("package").unwrap();
                    Ok(messages::SpawnPacker::new_android_native(
                        *uid,
                        *gid,
                        messages::SpawnAndroidNativePacker { package: package.to_string() },
                    )
                    .to_parcel())
                }
                Some(("SpawnLibApp", args)) => {
                    let path = args.get_one::<String>("path").unwrap();
                    let args_vec = args.get_many::<String>("args").unwrap().cloned().collect();
                    Ok(messages::SpawnPacker::new_lib_app(
                        *uid,
                        *gid,
                        messages::SpawnLibAppPacker { path: path.to_string(), args: args_vec },
                    )
                    .to_parcel())
                }
                Some(("SpawnMock", args)) => {
                    let name = args.get_one::<String>("name").unwrap();
                    Ok(messages::SpawnPacker::new_mock(
                        *uid,
                        *gid,
                        messages::SpawnMockPacker { name: name.to_string() },
                    )
                    .to_parcel())
                }
                Some((name, _)) => {
                    bail!("Unrecognized Spawn payload: {name}");
                }
                None => {
                    bail!("No payload specified for Spawn message");
                }
            }
        }
        Some(("Stat", _)) => Ok(messages::StatPacker {}.to_parcel()),
        _ => {
            bail!("Unrecognized Zygote server command");
        }
    }
}

fn get_client_socket(path_str: &String) -> Result<RawFd> {
    let socket_path = std::path::Path::new(path_str);
    if socket_path.exists() {
        let client_socket = sys::socket(libc::AF_UNIX, libc::SOCK_SEQPACKET, 0)?;
        let socket_addr = sys::bound_socket_address(path_str, libc::AF_UNIX as libc::sa_family_t);

        sys::connect(client_socket, &socket_addr)?;

        Ok(client_socket)
    } else {
        bail!("Zygote server socket path does not exist")
    }
}

fn handle_command_transaction(builder: FlatBufferBuilder, client_socket: RawFd) -> Result<()> {
    sys::sendmsg(client_socket, builder.finished_data())?;

    let (_, response) = sys::recvmsg::<{ messages::MESSAGE_BUFFER_SIZE }>(client_socket)?;

    let parcel = flatbuffers::root::<messages::Parcel>(&response)?;
    match parcel.message_type() {
        Message::Ack => {
            log::info!("Message acknowledged");
        }
        Message::IdentityQueryResponse => {
            let identity_query_response = parcel
                .message_as_identity_query_response()
                .ok_or(anyhow!("Could not unpack IdentityQueryResponse"))?;

            log::info!("Identity query successful: {:?}", identity_query_response);
            println!("{:?}", identity_query_response);
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

    Ok(())
}
