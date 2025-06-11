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

//! Command line utility for launching processes using Zygote species.

use anyhow::{bail, Result};
use clap::{Arg, Command};

use zygote::{
    config::log_level_parser,
    messages::{self, SpawnMessage, ToParcel, MESSAGE_BUFFER_INIT},
    species::{self, SpeciesRef},
};

fn main() -> Result<()> {
    let launch_matches = build_launch_parser().get_matches();

    logger::init(
        logger::Config::default()
            .with_tag_on_device("zygote_launch")
            .with_max_level(*launch_matches.get_one::<log::LevelFilter>("log-level").unwrap()),
    );

    let (species, builder) = build_spawn_parcel(&launch_matches)?;

    let mut message_buffer = MESSAGE_BUFFER_INIT;
    message_buffer.as_mut_slice()[0..builder.finished_data().len()]
        .copy_from_slice(builder.finished_data());

    // SAFETY: The contents of this SpawnMessage were parsed from the command
    //         line.  It is assumed that the caller of this program has
    //         permission to take any actions specified by those spawn
    //         arguments.  Any resulting actions will be taken with the
    //         permissions of the current process.
    let spawn_message = unsafe { SpawnMessage::new(message_buffer) };

    species.gestate(spawn_message, launch_matches.get_one::<i32>("priority-final").copied());
}

fn build_launch_parser() -> Command {
    Command::new("zygote_launch")
        .about("A command line interface for launching processes using Zygote species.")
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
        .arg(
            Arg::new("priority-final")
                .long("priority-final")
                .help("Final scheduling priority for child processes immediately before entering application code")
                .value_parser(clap::value_parser!(i32).range(-20..20))
        )
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
        .subcommand(
            Command::new("AndroidNative").arg(
                Arg::new("package").help("Package name for the new process").required(true),
            ),
        )
        .subcommand(
            Command::new("LibApp")
                .arg(Arg::new("path").help("Path to the library").required(true))
                .arg(Arg::new("args").help("Arguments for the library").num_args(clap::builder::ValueRange::new(0..)).trailing_var_arg(true)),
        )
        .subcommand(
            Command::new("Mock")
                .arg(Arg::new("name").help("Name for the mock process").required(true)),
        )
}

fn build_spawn_parcel<'builder>(
    cli_matches: &clap::ArgMatches,
) -> Result<(SpeciesRef, flatbuffers::FlatBufferBuilder<'builder>)> {
    let uid = cli_matches.get_one::<i32>("uid").unwrap_or(&-1);
    let gid = cli_matches.get_one::<i32>("gid").unwrap_or(&-1);

    match cli_matches.subcommand() {
        Some(("AndroidNative", args)) => {
            let package = args.get_one::<String>("package").unwrap();
            Ok((
                &species::android_native::App,
                messages::SpawnPacker::new_android_native(
                    *uid,
                    *gid,
                    messages::SpawnAndroidNativePacker { package: package.to_string() },
                )
                .to_parcel(),
            ))
        }
        Some(("LibApp", args)) => {
            let path = args.get_one::<String>("path").unwrap();
            let args_vec = args.get_many::<String>("args").unwrap().cloned().collect();
            Ok((
                &species::lib_app::App,
                messages::SpawnPacker::new_lib_app(
                    *uid,
                    *gid,
                    messages::SpawnLibAppPacker { path: path.to_string(), args: args_vec },
                )
                .to_parcel(),
            ))
        }
        #[cfg(any(test, feature = "test"))]
        Some(("Mock", args)) => {
            let name = args.get_one::<String>("name").unwrap();
            Ok((
                &species::mock::Turtle,
                messages::SpawnPacker::new_mock(
                    *uid,
                    *gid,
                    messages::SpawnMockPacker { name: name.to_string() },
                )
                .to_parcel(),
            ))
        }
        Some((name, _)) => {
            bail!("Unrecognized Spawn payload: {name}");
        }
        None => {
            bail!("No payload specified for Spawn message");
        }
    }
}
