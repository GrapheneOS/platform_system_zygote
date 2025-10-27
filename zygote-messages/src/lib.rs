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

use anyhow::Result;
use arrayvec::ArrayVec;
use clap::{Args, Subcommand};
use itertools::Itertools;

use cap::{CapabilityFlags, RawCap};
use zygote_proc_macros::{MarshalParcel, UnmarshalParcel};
use zygote_sys as sys;

/// Default size for GID vectors
pub const GID_VECTOR_SIZE: usize = 32;
/// Default size for rlimit vectors
pub const RLIMIT_VECTOR_SIZE: usize = 16;

/// Default size for all message parsing and passing.
pub const MESSAGE_BUFFER_SIZE: usize = 2048;
/// Zero-initialized message buffer
pub const MESSAGE_BUFFER_INIT: [u8; MESSAGE_BUFFER_SIZE] = [0; MESSAGE_BUFFER_SIZE];
/// Statically allocated arrays used for receiving messages.
pub type MessageBuffer = [u8; MESSAGE_BUFFER_SIZE];

/// The maximum size for buffers holding message arguments.
#[allow(dead_code)]
const MESSAGE_ARG_BUFFER_MAX: usize = 32;

/// A trait for helper structs that can be marshaled into a FlatBuffer
trait MarshalParcel<'builder> {
    /// The inner type for union variant (e.g. inner::Spawn)
    type Inner;
    /// The `flatc` generated type this is a wrapper for
    type Output;

    /// The `flatc` generated union type
    fn inner_type(&self) -> Self::Inner;

    /// Marshal this structure into a FlatBuffer
    fn marshal(
        &self,
        builder: &mut flatbuffers::FlatBufferBuilder<'builder>,
    ) -> flatbuffers::WIPOffset<Self::Output>;
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

fn marshal_capability_flags(
    cap: &Option<CapabilityFlags>,
    _builder: &mut flatbuffers::FlatBufferBuilder<'_>,
) -> RawCap {
    cap.map(|cap| cap.bits()).unwrap_or(RawCap::MAX)
}

fn marshal_string<'builder>(
    s: &Option<String>,
    builder: &mut flatbuffers::FlatBufferBuilder<'builder>,
) -> Option<flatbuffers::WIPOffset<&'builder str>> {
    match s.as_ref() {
        Some(name) => Some(name.to_packed(builder)),
        None => Some("".to_packed(builder)),
    }
}

fn unmarshal_capability_flags(cap: &RawCap) -> Option<CapabilityFlags> {
    if *cap == RawCap::MAX {
        None
    } else {
        Some(CapabilityFlags::from_bits_truncate(*cap))
    }
}

#[cfg(feature = "libapp")]
fn unmarshal_libapp_args<'a>(
    args: &flatbuffers::Vector<'a, flatbuffers::ForwardsUOffset<&'a str>>,
) -> ArrayVec<&'a str, { MESSAGE_ARG_BUFFER_MAX }> {
    args.iter().collect()
}

fn unmarshal_rlimits(
    rlimits: &flatbuffers::Vector<'_, inner::RLimitData>,
) -> ArrayVec<RLimitData, RLIMIT_VECTOR_SIZE> {
    rlimits
        .iter()
        .map(|rlimit| RLimitData {
            resource: rlimit.resource() as _,
            soft: rlimit.soft() as _,
            hard: rlimit.hard() as _,
        })
        .collect()
}

fn unmarshal_secondary_groups(
    groups: &Option<flatbuffers::Vector<'_, libc::gid_t>>,
) -> ArrayVec<libc::gid_t, GID_VECTOR_SIZE> {
    groups.map(|group| group.iter().collect()).unwrap_or_default()
}

fn unmarshal_string(s: &Option<&str>) -> Option<String> {
    s.map(|s| s.to_string())
}

/// Parameters common to all spawn operations
#[derive(Debug, Clone, MarshalParcel, UnmarshalParcel)]
#[inner_type_name = "SpawnCommon"]
pub struct SpawnParamsCommon {
    /// UID for the new process
    #[marshal(default = -1)]
    #[unmarshal(valid_range = (1..))]
    pub uid: Option<i32>,
    /// Primary GID for the new process
    #[marshal(default = -1)]
    #[unmarshal(valid_range = (1..))]
    pub gid: Option<i32>,
    /// Name of the new process
    #[marshal(map = marshal_string)]
    #[unmarshal(map = unmarshal_string)]
    pub process_name: Option<String>,
    /// Initial scheduling priority for child processes immediately after
    /// forking
    #[marshal(default = i32::MAX)]
    #[unmarshal(valid_range = (-20..20))]
    pub priority_initial: Option<i32>,
    /// Final scheduling priority for child processes immediately before
    /// entering application code
    #[marshal(default = i32::MAX)]
    #[unmarshal(valid_range = (-20..20))]
    pub priority_final: Option<i32>,
    /// Effective capabilities for the child process
    #[marshal(map = marshal_capability_flags)]
    #[unmarshal(map = unmarshal_capability_flags)]
    pub cap_effective: Option<CapabilityFlags>,
    /// Permitted capabilities for the child process
    #[marshal(map = marshal_capability_flags)]
    #[unmarshal(map = unmarshal_capability_flags)]
    pub cap_permitted: Option<CapabilityFlags>,
    /// Inheritable capabilities for the child process
    #[marshal(map = marshal_capability_flags)]
    #[unmarshal(map = unmarshal_capability_flags)]
    pub cap_inheritable: Option<CapabilityFlags>,
    /// Bounding capabilities for the child process
    #[marshal(map = marshal_capability_flags)]
    #[unmarshal(map = unmarshal_capability_flags)]
    pub cap_bound: Option<CapabilityFlags>,
    /// SELinux labels for the new process
    #[marshal(map = marshal_string)]
    #[unmarshal(map = unmarshal_string)]
    pub se_info: Option<String>,
    /// Secondary groups for the child process
    #[marshal(packed)]
    #[unmarshal(map = unmarshal_secondary_groups)]
    pub secondary_groups: ArrayVec<libc::gid_t, GID_VECTOR_SIZE>,
    /// Resource limits for the child process
    #[marshal(packed)]
    #[unmarshal(map = unmarshal_rlimits)]
    pub rlimits: ArrayVec<RLimitData, RLIMIT_VECTOR_SIZE>,
}

impl SpawnParamsCommon {
    /// Take optional arguments from `other` if the are missing from `self`
    pub fn or(&self, other: &Self) -> SpawnParamsCommon {
        SpawnParamsCommon {
            uid: self.uid.or(other.uid),
            gid: self.gid.or(other.gid),
            process_name: self.process_name.clone().or(other.process_name.clone()),
            priority_initial: self.priority_initial.or(other.priority_initial),
            priority_final: self.priority_final.or(other.priority_final),
            cap_effective: self.cap_effective.or(other.cap_effective),
            cap_permitted: self.cap_permitted.or(other.cap_permitted),
            cap_inheritable: self.cap_inheritable.or(other.cap_inheritable),
            cap_bound: self.cap_bound.or(other.cap_bound),
            se_info: self.se_info.clone().or(other.se_info.clone()),
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
#[derive(Debug, MarshalParcel, UnmarshalParcel)]
pub enum Message<'a, 'b> {
    /// Acknowledge that a command was received
    #[inner_type_name = "Ack"]
    AckResponse,
    /// Request the server cleanly shut down
    Exit,
    /// Request the server identify itself
    IdentityQuery,
    /// Response to an [`Message::IdentityQuery`]
    IdentityQueryResponse {
        /// Name of the server
        #[marshal(packed)]
        name: &'a str,
        /// Server species
        #[marshal(packed)]
        species: &'b str,
        /// Server binary architecture
        #[marshal(packed)]
        arch: &'b str,
    },
    /// Request the server spawn a new process
    Spawn {
        /// Parameters common to all spawn operations
        #[table]
        common: SpawnParamsCommon,
        /// Species-specific spawn data
        #[union(inner::Spawn<'a>)]
        payload: SpawnPayload<'a>,
    },
    /// Request the server spawn a new subspecies zygote process
    SpawnSubspecies {
        /// Parameters common to all spawn operations
        #[table]
        common: SpawnParamsCommon,
        /// Absolute path to the socket that the subspecies zygote should listen
        /// on
        #[marshal(packed)]
        socket_path: &'a str,
        /// Species-specific spawn data
        #[union(inner::SpawnSubspecies<'a>)]
        payload: SpawnPayload<'a>,
    },
    /// Response to a [`Message::Spawn`]
    SpawnResponse {
        /// PID of the new process
        pid: i32,
    },
    /// Request the server provide runtime statistics
    Stat,
    /// Response to a [`Message::Stat`]
    StatResponse {
        /// Process ID
        pid: u32,
        /// Process group ID
        pgrp: u32,
        /// Number of minor faults
        minflt: u64,
        /// Number of minor faults in waited-for children
        cminflt: u64,
        /// Number of major faults
        majflt: u64,
        /// Number of major faults in waited-for children
        cmajflt: u64,
        /// User time
        utime: u64,
        /// System time
        stime: u64,
        /// Number of threads in the process
        num_threads: u64,
        /// Virtual memory size in bytes
        vsize: u64,
        /// Resident set size in number of pages
        rss: u64,
    },
}

impl Message<'_, '_> {
    /// Return a [`Message::Spawn`] variant's parameters
    pub fn get_spawn_params(&self) -> Option<&SpawnParamsCommon> {
        match self {
            Message::Spawn { common, .. } | Message::SpawnSubspecies { common, .. } => Some(common),
            _ => None,
        }
    }

    /// Return a [`Message::Spawn`] variant's payload
    pub fn get_spawn_payload(&self) -> Option<&SpawnPayload<'_>> {
        match self {
            Message::Spawn { payload, .. } | Message::SpawnSubspecies { payload, .. } => {
                Some(payload)
            }
            _ => None,
        }
    }
}

impl ToParcel for Message<'_, '_> {
    fn to_parcel<'builder>(&self) -> flatbuffers::FlatBufferBuilder<'builder> {
        let mut builder =
            flatbuffers::FlatBufferBuilder::<'builder>::with_capacity(MESSAGE_BUFFER_SIZE);

        let packed_message = self.marshal(&mut builder);
        let parcel = inner::Parcel::create(
            &mut builder,
            &inner::ParcelArgs { message_type: self.inner_type(), message: Some(packed_message) },
        );

        builder.finish(parcel, None);
        builder
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
        /// Data common to all spawn commands
        #[command(flatten)]
        common: SpawnCommonParser,
        /// Species-specific spawn data
        #[command(subcommand)]
        payload: SpawnPayloadParser,
    },
    /// Request the server spawn a new subspecies zygote process
    SpawnSubspecies {
        /// Data common to all spawn commands
        #[command(flatten)]
        common: SpawnCommonParser,
        /// Absolute path to the socket that the subspecies zygote should listen
        /// on
        #[arg(long, required(true))]
        socket_path: String,
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
            MessageParser::Spawn { common, payload } => Ok(Message::Spawn {
                common: common.to_spawn_common(),
                payload: payload.to_spawn_payload()?,
            }),
            MessageParser::SpawnSubspecies { common, socket_path, payload } => {
                Ok(Message::SpawnSubspecies {
                    common: common.to_spawn_common(),
                    socket_path,
                    payload: payload.to_spawn_payload()?,
                })
            }
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
#[derive(Debug, MarshalParcel, UnmarshalParcel)]
#[unmarshal_from(source_type = inner::Spawn<'a>, field = payload)]
#[unmarshal_from(source_type = inner::SpawnSubspecies<'a>, field = payload)]
pub enum SpawnPayload<'a> {
    /// Spawn data for [`species::android_native::App`]
    #[inner_type_name = "SpawnAndroidNative"]
    AndroidNative {
        /// Name of the package to start
        #[marshal(packed)]
        package: &'a str,
        /// Id of the spawn request
        start_seq: i64,
        /// The target SDK version for the app.
        target_sdk_version: i32,
        /// Additional flags for the runtime.
        runtime_flags: u32,
    },
    /// Spawn data for the subspecies Zygote
    #[inner_type_name = "SpawnSubspeciesAndroidNative"]
    AndroidNativeSubspecies {
        /// The target SDK version for the app.
        target_sdk_version: i32,
        /// Additional flags for the runtime.
        runtime_flags: u32,
        /// Path to the library to load
        #[marshal(packed)]
        library_path: &'a str,
        /// Directories to search for libraries
        #[marshal(packed)]
        library_dirs: &'a str,
        /// Permitted library paths
        #[marshal(packed)]
        permitted_library_paths: &'a str,
        /// Preload function
        #[marshal(packed)]
        preload_func: Option<&'a str>,
        /// Minimum UID/GID for the subspecies zygote's UID/GID range
        uid_gid_min: u32,
        /// Maximum UID/GID for the subspecies zygote's UID/GID range
        uid_gid_max: u32,
    },
    /// Spawn data for [`species::lib_app::App`]
    #[cfg(feature = "libapp")]
    #[inner_type_name = "SpawnLibApp"]
    LibApp {
        /// Path to the shared library to load
        #[marshal(packed)]
        path: &'a str,
        /// Arguments to pass to the entry function
        #[marshal(packed)]
        #[unmarshal(map = unmarshal_libapp_args)]
        args: ArrayVec<&'a str, { MESSAGE_ARG_BUFFER_MAX }>,
    },
    /// Spawn data for [`species::mock::Turtle`]
    #[inner_type_name = "SpawnMock"]
    Mock {
        /// Name to print in the new process
        #[marshal(packed)]
        name: &'a str,
    },
}

/// Command line parser for building SpawnCommon
#[derive(Debug, Args)]
#[command(rename_all = "verbatim")]
pub struct SpawnCommonParser {
    /// UID for the new process
    #[arg(long)]
    uid: Option<i32>,
    /// Primary GID for the new process
    #[arg(long)]
    gid: Option<i32>,
    /// Name of the new process
    process_name: Option<String>,
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
    /// SELinux context to switch to
    se_info: Option<String>,
}

impl SpawnCommonParser {
    /// Construct a [`SpawnCommon`] from this enum
    pub fn to_spawn_common(&self) -> SpawnParamsCommon {
        SpawnParamsCommon {
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

/// Command line parser for building Spawn [`Message`]es
#[derive(Debug, Subcommand)]
#[command(rename_all = "verbatim")]
pub enum SpawnPayloadParser {
    /// Request the creation of an AndroidNative process
    #[cfg(all(target_os = "android", feature = "android-native"))]
    AndroidNative {
        /// The package to execute
        #[arg(required(true))]
        package: String,
        /// Id of the spawn request
        #[arg(required(true))]
        start_seq: i64,
        /// The target SDK version for the app.
        #[arg(required(true))]
        target_sdk_version: i32,
        /// Additional flags for the runtime.
        #[arg(required(true))]
        runtime_flags: u32,
    },
    /// Request the creation of a subspecies Zygote process
    #[cfg(all(target_os = "android", feature = "android-native"))]
    AndroidNativeSubspecies {
        /// The target SDK version for the app.
        #[arg(required(true))]
        target_sdk_version: i32,
        /// Additional flags for the runtime.
        #[arg(required(true))]
        runtime_flags: u32,
        /// Path to the library to load
        #[arg(long, required(true))]
        library_path: String,
        /// Directories to search for libraries
        #[arg(long)]
        library_dirs: String,
        /// Permitted library paths
        #[arg(long, required(true))]
        permitted_library_paths: String,
        /// Preload function
        #[arg(long)]
        preload_func: Option<String>,
        /// Minimum UID/GID for the subspecies zygote's UID/GID range
        #[arg(long)]
        uid_gid_min: u32,
        /// Maximum UID/GID for the subspecies zygote's UID/GID range
        #[arg(long)]
        uid_gid_max: u32,
    },
    /// Request the creation of a LibApp process
    #[cfg(feature = "libapp")]
    LibApp {
        /// Path to the library to load
        #[arg(required(true))]
        path: String,
        /// Arguments to pass to the entry function
        #[arg(trailing_var_arg(true))]
        args: Vec<String>,
    },
    /// Request the creation of a Mock process
    #[cfg(any(test, feature = "mock"))]
    Mock {
        /// The name to print in the new process
        #[arg(required(true))]
        name: String,
    },
}

impl SpawnPayloadParser {
    /// Construct a [`SpawnPayload`] from this enum
    pub fn to_spawn_payload(&self) -> Result<SpawnPayload<'_>> {
        match self {
            #[cfg(all(target_os = "android", feature = "android-native"))]
            SpawnPayloadParser::AndroidNative {
                package,
                start_seq,
                target_sdk_version,
                runtime_flags,
            } => Ok(SpawnPayload::AndroidNative {
                package: package.as_str(),
                start_seq: *start_seq,
                target_sdk_version: *target_sdk_version,
                runtime_flags: *runtime_flags,
            }),
            #[cfg(all(target_os = "android", feature = "android-native"))]
            SpawnPayloadParser::AndroidNativeSubspecies {
                target_sdk_version,
                runtime_flags,
                library_path,
                library_dirs,
                permitted_library_paths,
                preload_func,
                uid_gid_min,
                uid_gid_max,
            } => Ok(SpawnPayload::AndroidNativeSubspecies {
                target_sdk_version: *target_sdk_version,
                runtime_flags: *runtime_flags,
                library_path: library_path.as_str(),
                library_dirs: library_dirs.as_str(),
                permitted_library_paths: permitted_library_paths.as_str(),
                preload_func: preload_func.as_deref(),
                uid_gid_min: *uid_gid_min,
                uid_gid_max: *uid_gid_max,
            }),
            #[cfg(feature = "libapp")]
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

impl<'builder> ToPacked<'builder> for Option<&str> {
    type PackedType = flatbuffers::WIPOffset<&'builder str>;

    fn to_packed(
        &self,
        builder: &mut flatbuffers::FlatBufferBuilder<'builder>,
    ) -> Self::PackedType {
        builder.create_string(self.unwrap_or_default())
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
