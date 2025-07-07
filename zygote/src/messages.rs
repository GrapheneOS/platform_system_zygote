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

//! Generated Rust bindings for the FlatBuffer schema defined in `schemas/messages.fbs`

#[allow(dead_code, missing_docs, unsafe_op_in_unsafe_fn, unused_imports, clippy::all)]
mod inner {
    include!(concat!(env!("OUT_DIR"), "/messages.rs"));
}

use anyhow::{bail, Result};
use arrayvec::ArrayVec;
use clap::Subcommand;
use flatbuffers::UnionWIPOffset;
use itertools::Itertools;

use crate::species::{self, SpeciesRef};
use capwrap::{CapabilityFlags, RawCap};
use zygote_sys as sys;

/// Default size for GID vectors
pub const GID_VECTOR_SIZE: usize = 32;
/// Default size for rlimit vectors
pub const RLIMIT_VECTOR_SIZE: usize = 16;

/// Default size for all message parsing and passing.
pub const MESSAGE_BUFFER_SIZE: usize = 512;
/// Zero-initialized message buffer
pub const MESSAGE_BUFFER_INIT: [u8; MESSAGE_BUFFER_SIZE] = [0; MESSAGE_BUFFER_SIZE];
/// Statically allocated arrays used for receiving messages.
pub type MessageBuffer = [u8; MESSAGE_BUFFER_SIZE];

/// A trait for helper structs that can be marshaled into a FlatBuffer
trait EnumToFlatBufferUnion<InnerType> {
    /// The `flatc` generated union this is a wrapper for
    fn inner_type(&self) -> InnerType;

    /// Marshal this structure into a FlatBuffer
    fn marshal(
        &self,
        builder: &mut flatbuffers::FlatBufferBuilder<'_>,
    ) -> flatbuffers::WIPOffset<UnionWIPOffset>;
}

/// Traits for things that can be marshalled into a FlatBuffers Parcel as
/// defined in `schemas/messages.fbs` without fail.
pub trait ToParcel {
    /// Convert the struct into a FlatBufferBuilder containing a Parcel
    fn to_parcel<'a>(&self) -> flatbuffers::FlatBufferBuilder<'a>;
}

/// Traits for things that can be marshalled into a FlatBuffers Parcel as
/// defined in `schemas/messages.fbs`.
pub trait TryToParcel {
    /// Attempt to convert the struct into a FlatBufferBuilder containing a
    /// Parcel
    fn try_to_parcel<'a>(&self) -> Result<flatbuffers::FlatBufferBuilder<'a>>;
}

/// Trait for things that can be built from a FlatBuffers Parcel as defined
/// in `schemas/messages.fbs`.
pub trait FromParcel<'a>
where
    Self: Sized,
{
    /// Attempt to build the structure from a FlatBuffer
    fn try_from_parcel(buffer: &'a [u8]) -> Result<Self>;
}

/// Data necessary to call [`zygote_sys::setrlimits`]
#[derive(Debug, Clone, Copy)]
pub struct RLimitData {
    /// Resource ID
    pub resource: sys::rlimit_resource_t,
    /// The current resource limit
    pub soft: libc::rlim_t,
    /// The maximum resource limit
    pub hard: libc::rlim_t,
}

/// Parameters common to all spawn operations
#[derive(Debug, Clone)]
pub struct SpawnParamsCommon {
    /// UID for the new process
    pub uid: Option<i32>,
    /// Primary GID for the new process
    pub gid: Option<i32>,
    /// Initial scheduling priority for child processes immediately after
    /// forking
    pub priority_initial: Option<i32>,
    /// Final scheduling priority for child processes immediately before
    /// entering application code
    pub priority_final: Option<i32>,
    /// Effective capabilities for the child process
    pub cap_effective: Option<CapabilityFlags>,
    /// Permitted capabilities for the child process
    pub cap_permitted: Option<CapabilityFlags>,
    /// Inheritable capabilities for the child process
    pub cap_inheritable: Option<CapabilityFlags>,
    /// Bounding capabilities for the child process
    pub cap_bound: Option<CapabilityFlags>,
    /// Secondary groups for the child process
    pub secondary_groups: ArrayVec<libc::gid_t, GID_VECTOR_SIZE>,
    /// Resource limits for the child process
    pub rlimits: ArrayVec<RLimitData, RLIMIT_VECTOR_SIZE>,
}

impl SpawnParamsCommon {
    /// Take optional arguments from `other` if the are missing from `self`
    pub fn or(&self, other: &Self) -> SpawnParamsCommon {
        SpawnParamsCommon {
            uid: self.uid.or(other.uid),
            gid: self.gid.or(other.gid),
            priority_initial: self.priority_initial.or(other.priority_initial),
            priority_final: self.priority_final.or(other.priority_final),
            cap_effective: self.cap_effective.or(other.cap_effective),
            cap_permitted: self.cap_permitted.or(other.cap_permitted),
            cap_inheritable: self.cap_inheritable.or(other.cap_inheritable),
            cap_bound: self.cap_bound.or(other.cap_bound),
            secondary_groups: self
                .secondary_groups
                .iter()
                .cloned()
                .chain(other.secondary_groups.iter().cloned())
                .unique()
                .collect(),
            rlimits: self.rlimits.iter().cloned().chain(other.rlimits.iter().cloned()).collect(),
        }
    }
}

/// Messages used in the Zygote server protocol
#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum Message<'a, 'b> {
    /// Acknowledge that a command was received
    AckResponse,
    /// Request the server cleanly shut down
    Exit,
    /// Request the server identify itself
    IdentityQuery,
    /// Response to an [`Message::IdentityQuery`]
    IdentityQueryResponse {
        /// Name of the server
        name: &'a str,
        /// Server species
        species: &'b str,
        /// Server binary architecture
        arch: &'b str,
    },
    /// Request the server spawn a new process
    Spawn {
        /// Parameters common to all spawn operations
        params: SpawnParamsCommon,
        /// Species-specific spawn data
        payload: SpawnPayload<'a>,
    },
    /// Response to a [`Message::Spawn`]
    SpawnResponse {
        /// PID of the new process
        pid: i32,
    },
    /// Request the server provide runtime statistics
    Stat,
}

impl Message<'_, '_> {
    /// Return a [`Message::Spawn`] variant's parameters
    pub fn get_spawn_params(&self) -> Option<&SpawnParamsCommon> {
        match self {
            Message::Spawn { params, .. } => Some(params),
            _ => None,
        }
    }

    /// Return a [`Message::Spawn`] variant's payload
    pub fn get_spawn_payload(&self) -> Option<&SpawnPayload<'_>> {
        match self {
            Message::Spawn { payload, .. } => Some(payload),
            _ => None,
        }
    }
}

impl EnumToFlatBufferUnion<inner::Message> for Message<'_, '_> {
    fn inner_type(&self) -> inner::Message {
        match self {
            Message::AckResponse => inner::Message::Ack,
            Message::Exit => inner::Message::Exit,
            Message::IdentityQuery => inner::Message::IdentityQuery,
            Message::IdentityQueryResponse { .. } => inner::Message::IdentityQueryResponse,
            Message::Spawn { .. } => inner::Message::Spawn,
            Message::SpawnResponse { .. } => inner::Message::SpawnResponse,
            Message::Stat => inner::Message::Stat,
        }
    }

    fn marshal(
        &self,
        builder: &mut flatbuffers::FlatBufferBuilder<'_>,
    ) -> flatbuffers::WIPOffset<UnionWIPOffset> {
        match self {
            Message::AckResponse => {
                inner::Ack::create(builder, &inner::AckArgs {}).as_union_value()
            }
            Message::Exit => inner::Exit::create(builder, &inner::ExitArgs {}).as_union_value(),
            Message::IdentityQuery => {
                inner::IdentityQuery::create(builder, &inner::IdentityQueryArgs {}).as_union_value()
            }
            Message::IdentityQueryResponse { name, species, arch } => {
                let packed_name = name.to_packed(builder);
                let packed_species = species.to_packed(builder);
                let packed_arch = arch.to_packed(builder);
                inner::IdentityQueryResponse::create(
                    builder,
                    &inner::IdentityQueryResponseArgs {
                        name: Some(packed_name),
                        species: Some(packed_species),
                        arch: Some(packed_arch),
                    },
                )
                .as_union_value()
            }
            Message::Spawn { params, payload } => {
                let packed_groups = params.secondary_groups.to_packed(builder);
                let packed_rlimits = params.rlimits.to_packed(builder);
                let packed_payload = payload.marshal(builder);
                inner::Spawn::create(
                    builder,
                    &inner::SpawnArgs {
                        uid: params.uid.unwrap_or(-1),
                        gid: params.gid.unwrap_or(-1),
                        priority_initial: params.priority_initial.unwrap_or(<i32>::MAX),
                        priority_final: params.priority_final.unwrap_or(<i32>::MAX),
                        cap_effective: params
                            .cap_effective
                            .map(|cap| cap.bits())
                            .unwrap_or(RawCap::MAX),
                        cap_permitted: params
                            .cap_permitted
                            .map(|cap| cap.bits())
                            .unwrap_or(RawCap::MAX),
                        cap_inheritable: params
                            .cap_inheritable
                            .map(|cap| cap.bits())
                            .unwrap_or(RawCap::MAX),
                        cap_bound: params.cap_bound.map(|cap| cap.bits()).unwrap_or(RawCap::MAX),
                        secondary_groups: Some(packed_groups),
                        rlimits: Some(packed_rlimits),
                        payload_type: payload.inner_type(),
                        payload: Some(packed_payload),
                    },
                )
                .as_union_value()
            }
            Message::SpawnResponse { pid } => {
                inner::SpawnResponse::create(builder, &inner::SpawnResponseArgs { pid: *pid })
                    .as_union_value()
            }
            Message::Stat => inner::Stat::create(builder, &inner::StatArgs {}).as_union_value(),
        }
    }
}

impl ToParcel for Message<'_, '_> {
    fn to_parcel<'a>(&self) -> flatbuffers::FlatBufferBuilder<'a> {
        let mut builder = flatbuffers::FlatBufferBuilder::<'a>::with_capacity(MESSAGE_BUFFER_SIZE);

        let packed_message = self.marshal(&mut builder);
        let parcel = inner::Parcel::create(
            &mut builder,
            &inner::ParcelArgs { message_type: self.inner_type(), message: Some(packed_message) },
        );

        builder.finish(parcel, None);
        builder
    }
}

impl<'a> FromParcel<'a> for Message<'a, 'a> {
    fn try_from_parcel(buffer: &'a [u8]) -> Result<Self> {
        let parcel: inner::Parcel<'a> = flatbuffers::root::<inner::Parcel>(buffer)?;

        match parcel.message_type() {
            inner::Message::Ack => Ok(Message::AckResponse),
            inner::Message::Exit => Ok(Message::Exit),
            inner::Message::IdentityQuery => Ok(Message::IdentityQuery),
            inner::Message::IdentityQueryResponse => {
                let id_query_response = parcel.message_as_identity_query_response().unwrap();
                Ok(Message::IdentityQueryResponse {
                    name: id_query_response.name(),
                    species: id_query_response.species(),
                    arch: id_query_response.arch(),
                })
            }
            inner::Message::Spawn => {
                let spawn: inner::Spawn<'a> = parcel.message_as_spawn().unwrap();

                let uid = if spawn.uid() > 0 { Some(spawn.uid()) } else { None };
                let gid = if spawn.gid() > 0 { Some(spawn.gid()) } else { None };

                let priority_initial = if (-20..20).contains(&spawn.priority_initial()) {
                    Some(spawn.priority_initial())
                } else {
                    None
                };
                let priority_final = if (-20..20).contains(&spawn.priority_final()) {
                    Some(spawn.priority_final())
                } else {
                    None
                };

                let cap_effective = (spawn.cap_effective() != RawCap::MAX)
                    .then(|| CapabilityFlags::from_bits_truncate(spawn.cap_effective()));
                let cap_permitted = (spawn.cap_permitted() != RawCap::MAX)
                    .then(|| CapabilityFlags::from_bits_truncate(spawn.cap_permitted()));
                let cap_inheritable = (spawn.cap_inheritable() != RawCap::MAX)
                    .then(|| CapabilityFlags::from_bits_truncate(spawn.cap_inheritable()));
                let cap_bound = (spawn.cap_bound() != RawCap::MAX)
                    .then(|| CapabilityFlags::from_bits_truncate(spawn.cap_bound()));

                let secondary_groups = spawn
                    .secondary_groups()
                    .map(|groups| groups.iter().collect())
                    .unwrap_or_default();

                let rlimits = spawn
                    .rlimits()
                    .iter()
                    .map(|rlimit| RLimitData {
                        resource: rlimit.resource() as _,
                        soft: rlimit.soft() as _,
                        hard: rlimit.hard() as _,
                    })
                    .collect();

                let payload = SpawnPayload::<'a>::from_spawn(&spawn).unwrap();

                Ok(Message::Spawn {
                    params: SpawnParamsCommon {
                        uid,
                        gid,
                        priority_initial,
                        priority_final,
                        cap_effective,
                        cap_permitted,
                        cap_inheritable,
                        cap_bound,
                        secondary_groups,
                        rlimits,
                    },
                    payload,
                })
            }
            inner::Message::SpawnResponse => {
                let spawn_response = parcel.message_as_spawn_response().unwrap();
                Ok(Message::SpawnResponse { pid: spawn_response.pid() })
            }
            inner::Message::Stat => Ok(Message::Stat),
            inner::Message(tag) => {
                bail!("Unknown Message type: {tag}")
            }
        }
    }
}

/// Command line parser for building FlatBuffer Parcels
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Subcommand)]
#[command(rename_all = "verbatim")]
pub enum MessageParser {
    /// Request the server cleanly exit
    Exit,
    /// Request the server identify itself
    IdentityQuery,
    /// Request the server spawn a new process
    Spawn {
        /// UID for the new process
        #[arg(long)]
        uid: Option<i32>,
        /// Primary GID for the new process
        #[arg(long)]
        gid: Option<i32>,
        /// Initial scheduling priority for child processes immediately after
        /// forking
        #[arg(long, value_parser(clap::value_parser!(i32).range(-20..20)))]
        priority_initial: Option<i32>,
        /// Final scheduling priority for child processes immediately before
        /// entering application code
        #[arg(long, value_parser(clap::value_parser!(i32).range(-20..20)))]
        priority_final: Option<i32>,
        /// Secondary group IDs for the new process
        #[arg(long)]
        secondary_groups: Vec<libc::gid_t>,
        /// Species-specific spawn data
        #[command(subcommand)]
        payload: SpawnPayloadParser,
    },
    /// Request the server provide runtime statistics
    Stat,
}

impl MessageParser {
    fn to_message(&self) -> Result<Message<'_, '_>> {
        match self {
            MessageParser::Exit => Ok(Message::Exit),
            MessageParser::IdentityQuery => Ok(Message::IdentityQuery),
            MessageParser::Spawn {
                uid,
                gid,
                priority_initial,
                priority_final,
                secondary_groups,
                payload,
            } => Ok(Message::Spawn {
                params: SpawnParamsCommon {
                    uid: *uid,
                    gid: *gid,
                    priority_initial: *priority_initial,
                    priority_final: *priority_final,
                    cap_effective: None,
                    cap_permitted: None,
                    cap_inheritable: None,
                    cap_bound: None,
                    secondary_groups: secondary_groups.iter().cloned().collect(),
                    rlimits: ArrayVec::new(),
                },
                payload: payload.to_spawn_payload()?,
            }),
            MessageParser::Stat => Ok(Message::Stat),
        }
    }
}

impl TryToParcel for MessageParser {
    fn try_to_parcel<'a>(&self) -> Result<flatbuffers::FlatBufferBuilder<'a>> {
        Ok(self.to_message()?.to_parcel())
    }
}

/// Species-specific spawn data
#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum SpawnPayload<'a> {
    /// Spawn data for [`species::android_native::App`]
    AndroidNative {
        /// Name of the package to start
        package: &'a str,
    },
    /// Spawn data for [`species::lib_app::App`]
    LibApp {
        /// Path to the shared library to load
        path: &'a str,
        /// Arguments to pass to the entry function
        args: ArrayVec<&'a str, { crate::species::lib_app::MAX_ARGS }>,
    },
    /// Spawn data for [`species::mock::Turtle`]
    Mock {
        /// Name to print in the new process
        name: &'a str,
    },
}

impl<'a> SpawnPayload<'a> {
    fn from_spawn(spawn: &inner::Spawn<'a>) -> Result<Self> {
        match spawn.payload_type() {
            inner::SpawnPayload::SpawnAndroidNative => {
                let payload: inner::SpawnAndroidNative<'a> =
                    spawn.payload_as_spawn_android_native().unwrap();
                Ok(SpawnPayload::AndroidNative { package: payload.package() })
            }
            inner::SpawnPayload::SpawnLibApp => {
                let payload: inner::SpawnLibApp<'a> = spawn.payload_as_spawn_lib_app().unwrap();
                Ok(SpawnPayload::LibApp {
                    path: payload.path(),
                    args: payload.args().iter().collect(),
                })
            }
            inner::SpawnPayload::SpawnMock => {
                let payload: inner::SpawnMock<'a> = spawn.payload_as_spawn_mock().unwrap();
                Ok(SpawnPayload::Mock { name: payload.name() })
            }
            inner::SpawnPayload(tag) => {
                bail!("Unknown SpawnPayload type: {tag}")
            }
        }
    }
}

impl EnumToFlatBufferUnion<inner::SpawnPayload> for SpawnPayload<'_> {
    fn inner_type(&self) -> inner::SpawnPayload {
        match self {
            SpawnPayload::AndroidNative { .. } => inner::SpawnPayload::SpawnAndroidNative,
            SpawnPayload::LibApp { .. } => inner::SpawnPayload::SpawnLibApp,
            SpawnPayload::Mock { .. } => inner::SpawnPayload::SpawnMock,
        }
    }

    fn marshal(
        &self,
        builder: &mut flatbuffers::FlatBufferBuilder<'_>,
    ) -> flatbuffers::WIPOffset<UnionWIPOffset> {
        match self {
            SpawnPayload::AndroidNative { package } => {
                let packed_package = package.to_packed(builder);
                inner::SpawnAndroidNative::create(
                    builder,
                    &inner::SpawnAndroidNativeArgs { package: Some(packed_package) },
                )
                .as_union_value()
            }
            SpawnPayload::LibApp { path, args } => {
                let packed_path = path.to_packed(builder);
                let packed_args = args.to_packed(builder);
                inner::SpawnLibApp::create(
                    builder,
                    &inner::SpawnLibAppArgs { path: Some(packed_path), args: Some(packed_args) },
                )
                .as_union_value()
            }
            SpawnPayload::Mock { name } => {
                let packed_name = name.to_packed(builder);
                inner::SpawnMock::create(builder, &inner::SpawnMockArgs { name: Some(packed_name) })
                    .as_union_value()
            }
        }
    }
}

/// Command line parser for building Spawn [`Message`]es
#[derive(Debug, Subcommand)]
#[command(rename_all = "verbatim")]
pub enum SpawnPayloadParser {
    /// Request the creation of an AndroidNative process
    #[cfg(target_os = "android")]
    AndroidNative {
        /// The package to execute
        #[arg(required(true))]
        package: String,
    },
    /// Request the creation of a LibApp process
    LibApp {
        /// Path to the library to load
        #[arg(required(true))]
        path: String,
        /// Arguments to pass to the entry function
        #[arg(trailing_var_arg(true))]
        args: Vec<String>,
    },
    /// Request the creation of a Mock process
    #[cfg(any(test, feature = "test"))]
    Mock {
        /// The name to print in the new process
        #[arg(required(true))]
        name: String,
    },
}

impl SpawnPayloadParser {
    /// Fetch a reference to the species associated with this payload type
    pub fn species(&self) -> SpeciesRef {
        match self {
            #[cfg(target_os = "android")]
            SpawnPayloadParser::AndroidNative { .. } => &species::android_native::App,
            SpawnPayloadParser::LibApp { .. } => &species::lib_app::App,
            #[cfg(any(test, feature = "test"))]
            SpawnPayloadParser::Mock { .. } => &species::mock::Turtle,
        }
    }

    /// Construct a [`SpawnPayload`] from this enum
    pub fn to_spawn_payload(&self) -> Result<SpawnPayload<'_>> {
        match self {
            #[cfg(target_os = "android")]
            SpawnPayloadParser::AndroidNative { package } => {
                Ok(SpawnPayload::AndroidNative { package: package.as_str() })
            }
            SpawnPayloadParser::LibApp { path, args } => Ok(SpawnPayload::LibApp {
                path: path.as_str(),
                args: args.iter().map(|s| s.as_str()).collect(),
            }),
            #[cfg(any(test, feature = "test"))]
            SpawnPayloadParser::Mock { name } => Ok(SpawnPayload::Mock { name: name.as_str() }),
        }
    }
}

trait ToPacked<'builder> {
    type PackedType: flatbuffers::Push;
    fn to_packed(&self, builder: &mut flatbuffers::FlatBufferBuilder<'builder>)
        -> Self::PackedType;
}

impl<'builder> ToPacked<'builder> for &String {
    type PackedType = flatbuffers::WIPOffset<&'builder str>;

    fn to_packed(
        &self,
        builder: &mut flatbuffers::FlatBufferBuilder<'builder>,
    ) -> Self::PackedType {
        builder.create_string(self.as_str())
    }
}

impl<'builder> ToPacked<'builder> for &str {
    type PackedType = flatbuffers::WIPOffset<&'builder str>;

    fn to_packed(
        &self,
        builder: &mut flatbuffers::FlatBufferBuilder<'builder>,
    ) -> Self::PackedType {
        builder.create_string(self)
    }
}

impl<'builder, const N: usize> ToPacked<'builder> for ArrayVec<&str, N> {
    type PackedType = flatbuffers::WIPOffset<
        flatbuffers::Vector<'builder, flatbuffers::ForwardsUOffset<&'builder str>>,
    >;

    fn to_packed(
        &self,
        builder: &mut flatbuffers::FlatBufferBuilder<'builder>,
    ) -> Self::PackedType {
        let packed_strings: Vec<_> = self.iter().map(|s| builder.create_string(s)).collect();
        builder.create_vector(&packed_strings)
    }
}

impl<'builder, const N: usize> ToPacked<'builder> for ArrayVec<u32, N> {
    type PackedType =
        flatbuffers::WIPOffset<flatbuffers::Vector<'builder, <u32 as flatbuffers::Push>::Output>>;

    fn to_packed(
        &self,
        builder: &mut flatbuffers::FlatBufferBuilder<'builder>,
    ) -> Self::PackedType {
        builder.create_vector_from_iter(self.iter())
    }
}

impl<'builder, const N: usize> ToPacked<'builder> for ArrayVec<RLimitData, N> {
    type PackedType = flatbuffers::WIPOffset<
        flatbuffers::Vector<'builder, <inner::RLimitData as flatbuffers::Push>::Output>,
    >;

    fn to_packed(
        &self,
        builder: &mut flatbuffers::FlatBufferBuilder<'builder>,
    ) -> Self::PackedType {
        let packed_rlimit_data = self.iter().map(|rlimit_data| {
            inner::RLimitData::new(
                rlimit_data.resource as _,
                rlimit_data.soft as _,
                rlimit_data.hard as _,
            )
        });
        builder.create_vector_from_iter(packed_rlimit_data)
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
