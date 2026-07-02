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
use arrayvec::ArrayVec;
use clap::Parser;

use zygote::species::ToSpecies;
use zygote_core::{init_reporting, log_level_parser, trace_level_parser};
use zygote_messages::{
    FromParcel, Message, SpawnParamsCommon, SpawnPayloadParser, ToParcel, TryToParcel,
    MESSAGE_BUFFER_INIT,
};

// The documentation string for this struct appears as the help message on the
// command line, so the programmer-facing documentation is left as a regular
// comment:
//
// Configuration values used by the Zygote launch utility.  This API is
// temporary as the message types evolve.
//
/// Launch a new process using Zygote initialization logic
#[derive(Debug, Parser)]
pub struct Launch {
    /// GrapheneOS SELinux flags, see frameworks/base/core/java/com/android/internal/os/SELinuxFlags.java
    #[arg(long)]
    pub selinux_flags: Option<u64>,

    /// Controls verbosity of logging; defaults to Warn; flag with no argument sets Debug
    #[arg(long, alias("verbose"), short_alias('v'), num_args(0..=1), default_value("2"), default_missing_value("4"), value_parser(log_level_parser))]
    pub log_level: log::LevelFilter,

    /// Controls verbosity of tracing; defaults to Info; flag with no argument sets Trace
    #[arg(long, num_args(0..=1), default_value("3"), default_missing_value("5"), value_parser(trace_level_parser))]
    pub trace_level: tracing::level_filters::LevelFilter,

    /// The species and species specific arguments
    #[command(subcommand)]
    pub payload: SpawnPayloadParser,

    /*
     * Common Spawn Parameters
     */
    /// UID to switch to
    #[arg(long)]
    pub uid: Option<i32>,

    /// Main GID to switch to
    #[arg(long)]
    pub gid: Option<i32>,

    /// Name of the new process
    #[arg(long)]
    pub process_name: Option<String>,

    /// Initial scheduling priority for child processes immediately after
    /// forking
    #[arg(long, value_parser(clap::value_parser!(i32).range(-20..20)))]
    pub priority_initial: Option<i32>,

    /// Final scheduling priority for child processes immediately before
    /// entering application code
    #[arg(long, value_parser(clap::value_parser!(i32).range(-20..20)))]
    pub priority_final: Option<i32>,

    /// SELinux context to switch to
    #[arg(long)]
    pub se_info: Option<String>,

    /// Secondary group IDs for the new process
    #[arg(long)]
    pub secondary_groups: Vec<libc::gid_t>,
}

impl Launch {
    fn to_spawn_message(&self) -> zygote_messages::Result<Message<'_, '_>> {
        Ok(Message::Spawn {
            common: self.to_spawn_params(),
            payload: self.payload.to_spawn_payload()?,
        })
    }

    fn to_spawn_params(&self) -> SpawnParamsCommon {
        SpawnParamsCommon {
            selinux_flags: self.selinux_flags,
            uid: self.uid,
            gid: self.gid,
            process_name: self.process_name.clone(),
            priority_initial: self.priority_initial,
            priority_final: self.priority_final,
            cap_effective: None,
            cap_permitted: None,
            cap_inheritable: None,
            cap_bound: None,
            se_info: self.se_info.clone(),
            secondary_groups: self.secondary_groups.iter().cloned().collect(),
            rlimits: ArrayVec::new(),
        }
    }
}

impl TryToParcel for Launch {
    fn try_to_parcel<'a>(&self) -> zygote_messages::Result<flatbuffers::FlatBufferBuilder<'a>> {
        Ok(self.to_spawn_message()?.to_parcel())
    }
}

fn main() -> Result<()> {
    let config = Launch::parse();
    let _trace_guard = init_reporting("zygote_launch", config.log_level, config.trace_level);

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
