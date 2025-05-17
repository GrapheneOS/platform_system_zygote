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

use anyhow::Result;
use clap::Parser;
use flatbuffers::UnionWIPOffset;

pub const DEFAULT_BUFFER_SIZE: usize = 1024;

pub trait ToFlatbuffer {
    fn build_command(
        &self,
        builder: &mut flatbuffers::FlatBufferBuilder,
    ) -> (Command, flatbuffers::WIPOffset<UnionWIPOffset>);

    fn build<'a>(&self) -> flatbuffers::FlatBufferBuilder<'a> {
        let mut builder = flatbuffers::FlatBufferBuilder::<'a>::with_capacity(DEFAULT_BUFFER_SIZE);

        let (command_type, command) = self.build_command(&mut builder);

        let message =
            Message::create(&mut builder, &MessageArgs { command_type, command: Some(command) });

        builder.finish(message, None);

        builder
    }
}

#[derive(Debug, Parser)]
pub struct ExitParser;

impl ToFlatbuffer for ExitParser {
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

impl ToFlatbuffer for SpawnAndroidNativeParser {
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

impl ToFlatbuffer for SpawnLibAppParser {
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

impl ToFlatbuffer for SpawnMockParser {
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

impl ToFlatbuffer for StatParser {
    fn build_command(
        &self,
        builder: &mut flatbuffers::FlatBufferBuilder,
    ) -> (Command, flatbuffers::WIPOffset<UnionWIPOffset>) {
        (Command::Stat, Stat::create(builder, &StatArgs {}).as_union_value())
    }
}
