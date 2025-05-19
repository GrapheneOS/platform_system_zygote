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

//! Generated Rust bindings for the Flatbuffer schema defined in `schemas/messages.fbs`

#![allow(dead_code, missing_docs, unsafe_op_in_unsafe_fn, unused_imports, clippy::all)]

include!(concat!(env!("OUT_DIR"), "/messages.rs"));

use anyhow::{bail, Result};
use clap::Parser;
use flatbuffers::UnionWIPOffset;

/// Default size for all message parsing and passing.
pub const MESSAGE_BUFFER_SIZE: usize = 512;
/// Zero-initialized message buffer
pub const MESSAGE_BUFFER_INIT: [u8; MESSAGE_BUFFER_SIZE] = [0; MESSAGE_BUFFER_SIZE];
/// Statically allocated arrays used for receiving messages.
pub type MessageBuffer = [u8; MESSAGE_BUFFER_SIZE];

pub trait ToFlatBuffer {
    fn build_command(
        &self,
        builder: &mut flatbuffers::FlatBufferBuilder,
    ) -> (Command, flatbuffers::WIPOffset<UnionWIPOffset>);

    fn build<'a>(&self) -> flatbuffers::FlatBufferBuilder<'a> {
        let mut builder = flatbuffers::FlatBufferBuilder::<'a>::with_capacity(MESSAGE_BUFFER_SIZE);

        let (command_type, command) = self.build_command(&mut builder);

        let message =
            Message::create(&mut builder, &MessageArgs { command_type, command: Some(command) });

        builder.finish(message, None);

        builder
    }
}

#[derive(Debug, Parser)]
pub struct ExitParser;

impl ToFlatBuffer for ExitParser {
    fn build_command(
        &self,
        builder: &mut flatbuffers::FlatBufferBuilder,
    ) -> (Command, flatbuffers::WIPOffset<UnionWIPOffset>) {
        (Command::Exit, Exit::create(builder, &ExitArgs {}).as_union_value())
    }
}

#[derive(Debug, Parser)]
pub struct SpawnAndroidNativeParser {
    #[arg(required(true))]
    package: String,
}

impl ToFlatBuffer for SpawnAndroidNativeParser {
    fn build_command(
        &self,
        builder: &mut flatbuffers::FlatBufferBuilder,
    ) -> (Command, flatbuffers::WIPOffset<UnionWIPOffset>) {
        let packed_package = builder.create_string(self.package.as_str());

        (
            Command::SpawnAndroidNative,
            SpawnAndroidNative::create(
                builder,
                &SpawnAndroidNativeArgs { package: Some(packed_package) },
            )
            .as_union_value(),
        )
    }
}

#[derive(Debug, Parser)]
pub struct SpawnLibAppParser {
    #[arg(required(true))]
    path: String,
}

impl ToFlatBuffer for SpawnLibAppParser {
    fn build_command(
        &self,
        builder: &mut flatbuffers::FlatBufferBuilder,
    ) -> (Command, flatbuffers::WIPOffset<UnionWIPOffset>) {
        let packed_path = builder.create_string(self.path.as_str());

        (
            Command::SpawnLibApp,
            SpawnLibApp::create(builder, &SpawnLibAppArgs { path: Some(packed_path) })
                .as_union_value(),
        )
    }
}

#[derive(Debug, Parser)]
pub struct SpawnMockParser {
    #[arg(required(true))]
    name: String,
}

impl ToFlatBuffer for SpawnMockParser {
    fn build_command(
        &self,
        builder: &mut flatbuffers::FlatBufferBuilder,
    ) -> (Command, flatbuffers::WIPOffset<UnionWIPOffset>) {
        let packed_name = builder.create_string(self.name.as_str());

        (
            Command::SpawnMock,
            SpawnMock::create(builder, &SpawnMockArgs { name: Some(packed_name) }).as_union_value(),
        )
    }
}

#[derive(Debug, Parser)]
pub struct StatParser;

impl ToFlatBuffer for StatParser {
    fn build_command(
        &self,
        builder: &mut flatbuffers::FlatBufferBuilder,
    ) -> (Command, flatbuffers::WIPOffset<UnionWIPOffset>) {
        (Command::Stat, Stat::create(builder, &StatArgs {}).as_union_value())
    }
}

/// A wrapper class used to ensure that the process server and species code
/// document the invariants and safety checks that they rely upon when handling
/// spawn messages.
#[repr(transparent)]
pub struct SpawnMessage {
    buffer: MessageBuffer,
}

impl SpawnMessage {
    /// Constructor
    ///
    /// # Safety
    /// Once consumed, the contents of the MessageBuffer can be used to access
    /// file system resources and execute code.  For this reason, the contents
    /// of the MessageBuffer must come from a trusted source that is authorized
    /// to execute commands in the current process's context.
    pub unsafe fn new(message_buffer: MessageBuffer) -> SpawnMessage {
        SpawnMessage { buffer: message_buffer }
    }
}

impl AsRef<MessageBuffer> for SpawnMessage {
    fn as_ref(&self) -> &MessageBuffer {
        &self.buffer
    }
}

pub fn build_message<'a>(
    command_name: &'a String,
    command_args: &'a [String],
) -> Result<flatbuffers::FlatBufferBuilder<'a>> {
    let extra_args_iter = std::iter::once(command_name).chain(command_args.iter());

    match command_name.as_str() {
        "Exit" => Ok(ExitParser::try_parse_from(extra_args_iter)?.build()),
        "SpawnAndroidNative" => {
            Ok(SpawnAndroidNativeParser::try_parse_from(extra_args_iter)?.build())
        }
        "SpawnLibApp" => Ok(SpawnLibAppParser::try_parse_from(extra_args_iter)?.build()),
        "SpawnMock" => Ok(SpawnMockParser::try_parse_from(extra_args_iter)?.build()),
        "Stat" => Ok(StatParser::try_parse_from(extra_args_iter)?.build()),
        _ => {
            bail!("Invalid command type: {}", command_name)
        }
    }
}
