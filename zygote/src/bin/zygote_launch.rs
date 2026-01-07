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
    config::{self, Launch},
    species::ToSpecies,
};
use zygote_messages::{FromParcel, Message, TryToParcel, MESSAGE_BUFFER_INIT};

fn main() -> Result<()> {
    let config = Launch::parse();
    let _trace_guard =
        config::init_reporting("zygote_launch", config.log_level, config.trace_level);

    let builder = config.try_to_parcel()?;

    let mut message_buffer = MESSAGE_BUFFER_INIT;
    message_buffer.as_mut_slice()[0..builder.finished_data().len()]
        .copy_from_slice(builder.finished_data());

    let spawn_message = Message::try_from_parcel(&message_buffer)?;

    config.payload.to_species().gestate(
        spawn_message.get_spawn_params().unwrap(),
        spawn_message.get_spawn_payload().unwrap(),
    )
}
