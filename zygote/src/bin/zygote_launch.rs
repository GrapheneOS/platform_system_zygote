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

use clap::Parser;

use zygote::{config, messages};

fn main() {
    let config = config::Launch::parse();

    logger::init(
        logger::Config::default()
            .with_tag_on_device("zygote_launch")
            .with_max_level(config.log_level),
    );

    let command_name = config.species.command_type_spawn().variant_name().unwrap().to_string();
    let spawn_message = messages::build_message(&command_name, &config.spawn_args).unwrap();

    let mut message_buffer = messages::MESSAGE_BUFFER_INIT;
    message_buffer.as_mut_slice()[0..spawn_message.finished_data().len()]
        .copy_from_slice(spawn_message.finished_data());

    // SAFETY: The contents of this SpawnMessage were parsed from the command
    //         line.  It is assumed that the caller of this program has
    //         permission to take any actions specified by those spawn
    //         arguments.  Any resulting actions will be taken with the
    //         permissions of the current process.
    config.species.gestate(unsafe { messages::SpawnMessage::new(message_buffer) });
}
