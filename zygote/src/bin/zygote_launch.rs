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

use anyhow::Result;
use clap::Parser;

use zygote::{
    config::Launch,
    messages::{SpawnMessage, TryToParcel, MESSAGE_BUFFER_INIT},
};

fn main() -> Result<()> {
    let config = Launch::parse();

    logger::init(
        logger::Config::default()
            .with_tag_on_device("zygote_launch")
            .with_max_level(config.log_level),
    );

    let builder = config.try_to_parcel()?;

    let mut message_buffer = MESSAGE_BUFFER_INIT;
    message_buffer.as_mut_slice()[0..builder.finished_data().len()]
        .copy_from_slice(builder.finished_data());

    // SAFETY: The contents of this SpawnMessage were parsed from the command
    //         line.  It is assumed that the caller of this program has
    //         permission to take any actions specified by those spawn
    //         arguments.  Any resulting actions will be taken with the
    //         permissions of the current process.
    let spawn_message = unsafe { SpawnMessage::new(message_buffer) };

    config.payload.species().gestate(spawn_message, config.priority_final)
}
