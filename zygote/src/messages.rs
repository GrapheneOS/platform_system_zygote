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

use flatbuffers::UnionWIPOffset;

// Export types from the inner module.
pub use inner::{
    Ack, AckArgs, Exit, ExitArgs, IdentityQuery, IdentityQueryArgs, IdentityQueryResponse,
    IdentityQueryResponseArgs, Message, Parcel, ParcelArgs, Spawn, SpawnAndroidNative,
    SpawnAndroidNativeArgs, SpawnArgs, SpawnLibApp, SpawnLibAppArgs, SpawnMock, SpawnMockArgs,
    SpawnPayload, SpawnResponse, SpawnResponseArgs, Stat, StatArgs,
};

/// Default size for all message parsing and passing.
pub const MESSAGE_BUFFER_SIZE: usize = 512;
/// Zero-initialized message buffer
pub const MESSAGE_BUFFER_INIT: [u8; MESSAGE_BUFFER_SIZE] = [0; MESSAGE_BUFFER_SIZE];
/// Statically allocated arrays used for receiving messages.
pub type MessageBuffer = [u8; MESSAGE_BUFFER_SIZE];

/*
 * Traits
 */

/// A trait for capturing associated types of FlatBuffers helpers
pub trait FlatBufferAssociatedType<'builder> {
    /// The `flatc` generated struct for creating tables of the associated type
    type FlatBufferType;
    /// The `flatc` generated struct for aggregating packed table constructor
    /// arguments
    type FlatBufferArgType;
}

/// A trait for helper structs that can be marshaled into a FlatBuffer
pub trait ToFlatBuffer<'builder>
where
    Self: FlatBufferAssociatedType<'builder>,
{
    /// Marshal this structure into a FlatBuffer
    fn marshal(
        &self,
        builder: &mut flatbuffers::FlatBufferBuilder<'builder>,
    ) -> flatbuffers::WIPOffset<Self::FlatBufferType>;
}

/// A trait for helper structs that can be marshaled into a FlatBuffer union
pub trait ToFlatBufferUnion<'builder, UnionType>
where
    Self: FlatBufferAssociatedType<'builder> + ToFlatBuffer<'builder>,
{
    /// The type tag for this element type of the union
    const UNION_TAG: UnionType;

    /// Marshal this structure into a FlatBuffer and then create a union value
    fn marshal_union(
        &self,
        builder: &mut flatbuffers::FlatBufferBuilder<'builder>,
    ) -> flatbuffers::WIPOffset<UnionWIPOffset> {
        self.marshal(builder).as_union_value()
    }
}

/// A trait for helper structs that can be marshaled into the FlatBuffer Parcel
/// table defined in `messages.fbs`
pub trait ToParcel<'builder>
where
    Self: ToFlatBufferUnion<'builder, Message>,
{
    /// Create and finalize a Parcel from this member of the Message FlatBuffer
    /// union
    fn to_parcel(&self) -> flatbuffers::FlatBufferBuilder<'builder> {
        let mut builder =
            flatbuffers::FlatBufferBuilder::<'builder>::with_capacity(MESSAGE_BUFFER_SIZE);

        let message = self.marshal_union(&mut builder);

        let parcel = Parcel::create(
            &mut builder,
            &ParcelArgs { message_type: Self::UNION_TAG, message: Some(message) },
        );
        builder.finish(parcel, None);
        builder
    }
}

impl<'builder, T> ToParcel<'builder> for T where T: ToFlatBufferUnion<'builder, Message> {}

trait ToPacked<'builder> {
    type PackedType: ?Sized;
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

impl<'builder> ToPacked<'builder> for &Vec<String> {
    type PackedType = flatbuffers::WIPOffset<
        flatbuffers::Vector<'builder, flatbuffers::ForwardsUOffset<&'builder str>>,
    >;

    fn to_packed(
        &self,
        builder: &mut flatbuffers::FlatBufferBuilder<'builder>,
    ) -> Self::PackedType {
        let packed_strings: Vec<_> =
            self.iter().map(|s| builder.create_string(s.as_str())).collect();
        builder.create_vector(&packed_strings)
    }
}

/*
 * Structs
 */

/// Helper struct for constructing Ack messages.
#[derive(Debug)]
pub struct AckPacker;

impl<'builder> FlatBufferAssociatedType<'builder> for AckPacker {
    type FlatBufferType = Ack<'builder>;
    type FlatBufferArgType = AckArgs;
}

impl ToFlatBufferUnion<'_, Message> for AckPacker {
    const UNION_TAG: Message = Message::Ack;
}

impl<'builder> ToFlatBuffer<'builder> for AckPacker {
    fn marshal(
        &self,
        builder: &mut flatbuffers::FlatBufferBuilder<'builder>,
    ) -> flatbuffers::WIPOffset<Self::FlatBufferType> {
        Self::FlatBufferType::create(builder, &Self::FlatBufferArgType {})
    }
}

/// Helper struct for constructing Exit messages.
#[derive(Debug)]
pub struct ExitPacker;

impl<'builder> FlatBufferAssociatedType<'builder> for ExitPacker {
    type FlatBufferType = Exit<'builder>;
    type FlatBufferArgType = ExitArgs;
}

impl ToFlatBufferUnion<'_, Message> for ExitPacker {
    const UNION_TAG: Message = Message::Exit;
}

impl<'builder> ToFlatBuffer<'builder> for ExitPacker {
    fn marshal(
        &self,
        builder: &mut flatbuffers::FlatBufferBuilder<'builder>,
    ) -> flatbuffers::WIPOffset<Self::FlatBufferType> {
        Self::FlatBufferType::create(builder, &Self::FlatBufferArgType {})
    }
}

/// Helper struct for constructing IdentityQuery messages.
#[derive(Debug)]
pub struct IdentityQueryPacker;

impl<'builder> FlatBufferAssociatedType<'builder> for IdentityQueryPacker {
    type FlatBufferType = IdentityQuery<'builder>;
    type FlatBufferArgType = IdentityQueryArgs;
}

impl ToFlatBufferUnion<'_, Message> for IdentityQueryPacker {
    const UNION_TAG: Message = Message::IdentityQuery;
}

impl<'builder> ToFlatBuffer<'builder> for IdentityQueryPacker {
    fn marshal(
        &self,
        builder: &mut flatbuffers::FlatBufferBuilder<'builder>,
    ) -> flatbuffers::WIPOffset<Self::FlatBufferType> {
        Self::FlatBufferType::create(builder, &Self::FlatBufferArgType {})
    }
}

/// Helper struct for constructing IdentityQueryResponse messages.
#[derive(Debug)]
pub struct IdentityQueryResponsePacker<'args> {
    /// Name of the Zygote server process
    pub name: &'args String,
    /// Name of the Zygote server's species
    pub species: &'static str,
    /// Name of the Zygote server's architecture
    pub arch: &'static str,
}

impl<'builder> FlatBufferAssociatedType<'builder> for IdentityQueryResponsePacker<'_> {
    type FlatBufferType = IdentityQueryResponse<'builder>;
    type FlatBufferArgType = IdentityQueryResponseArgs<'builder>;
}

impl ToFlatBufferUnion<'_, Message> for IdentityQueryResponsePacker<'_> {
    const UNION_TAG: Message = Message::IdentityQueryResponse;
}

impl<'builder> ToFlatBuffer<'builder> for IdentityQueryResponsePacker<'_> {
    fn marshal(
        &self,
        builder: &mut flatbuffers::FlatBufferBuilder<'builder>,
    ) -> flatbuffers::WIPOffset<Self::FlatBufferType> {
        let packed_name = self.name.to_packed(builder);
        let packed_species = self.species.to_packed(builder);
        let packed_arch = self.arch.to_packed(builder);

        Self::FlatBufferType::create(
            builder,
            &Self::FlatBufferArgType {
                name: Some(packed_name),
                species: Some(packed_species),
                arch: Some(packed_arch),
            },
        )
    }
}

union SpawnPayloadPackers {
    android_native: std::mem::ManuallyDrop<SpawnAndroidNativePacker>,
    lib_app: std::mem::ManuallyDrop<SpawnLibAppPacker>,
    mock: std::mem::ManuallyDrop<SpawnMockPacker>,
}

/// Helper struct for constructing Spawn messages
pub struct SpawnPacker {
    /// UID to assign to the newly created process
    pub uid: i32,
    /// GID to assign to the newly created process
    pub gid: i32,

    payload_type: SpawnPayload,
    payload: SpawnPayloadPackers,
}

impl SpawnPacker {
    /// Create a Spawn message with a SpawnAndroidNativePacker as the payload
    pub fn new_android_native(uid: i32, gid: i32, payload: SpawnAndroidNativePacker) -> Self {
        Self {
            uid,
            gid,
            payload_type: SpawnPayload::SpawnAndroidNative,
            payload: SpawnPayloadPackers { android_native: std::mem::ManuallyDrop::new(payload) },
        }
    }

    /// Create a Spawn message with a SpawnLibAppPacker as the payload
    pub fn new_lib_app(uid: i32, gid: i32, payload: SpawnLibAppPacker) -> Self {
        Self {
            uid,
            gid,
            payload_type: SpawnPayload::SpawnLibApp,
            payload: SpawnPayloadPackers { lib_app: std::mem::ManuallyDrop::new(payload) },
        }
    }

    /// Create a Spawn message with a SpawnMockPacker as a payload
    pub fn new_mock(uid: i32, gid: i32, payload: SpawnMockPacker) -> Self {
        Self {
            uid,
            gid,
            payload_type: SpawnPayload::SpawnMock,
            payload: SpawnPayloadPackers { mock: std::mem::ManuallyDrop::new(payload) },
        }
    }
}

impl std::ops::Drop for SpawnPacker {
    fn drop(&mut self) {
        // SAFETY: All reads/writes to this union are moderated by the
        //         `payload_type` value.
        unsafe {
            match self.payload_type {
                SpawnPayload::SpawnAndroidNative => {
                    std::mem::ManuallyDrop::drop(&mut self.payload.android_native)
                }
                SpawnPayload::SpawnLibApp => {
                    std::mem::ManuallyDrop::drop(&mut self.payload.lib_app)
                }
                SpawnPayload::SpawnMock => std::mem::ManuallyDrop::drop(&mut self.payload.mock),
                _ => {}
            }
        }
    }
}

impl std::fmt::Debug for SpawnPacker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut debug_builder = f.debug_struct("SpawnPacker");

        debug_builder
            .field("uid", &self.uid)
            .field("gid", &self.gid)
            .field("payload_type", &self.payload_type);

        // SAFETY: All reads/writes to this union are moderated by the
        //         `payload_type` value.
        unsafe {
            match self.payload_type {
                SpawnPayload::SpawnAndroidNative => {
                    debug_builder.field("payload", &self.payload.android_native)
                }
                SpawnPayload::SpawnLibApp => debug_builder.field("payload", &self.payload.lib_app),
                SpawnPayload::SpawnMock => debug_builder.field("payload", &self.payload.mock),
                _ => debug_builder.field("payload", &"UNKNOWN"),
            };
        }

        debug_builder.finish()
    }
}

impl<'builder> FlatBufferAssociatedType<'builder> for SpawnPacker {
    type FlatBufferType = Spawn<'builder>;
    type FlatBufferArgType = SpawnArgs;
}

impl ToFlatBufferUnion<'_, Message> for SpawnPacker {
    const UNION_TAG: Message = Message::Spawn;
}

impl<'builder> ToFlatBuffer<'builder> for SpawnPacker {
    fn marshal(
        &self,
        builder: &mut flatbuffers::FlatBufferBuilder<'builder>,
    ) -> flatbuffers::WIPOffset<Self::FlatBufferType> {
        // SAFETY: All reads/writes to this union are moderated by the
        //         `payload_type` value.
        let payload_offset = unsafe {
            match self.payload_type {
                SpawnPayload::SpawnAndroidNative => {
                    self.payload.android_native.marshal(builder).as_union_value()
                }
                SpawnPayload::SpawnLibApp => self.payload.lib_app.marshal(builder).as_union_value(),
                SpawnPayload::SpawnMock => self.payload.mock.marshal(builder).as_union_value(),
                _ => panic!("Attempted to marshal unknown SpawnPayload type"),
            }
        };

        Self::FlatBufferType::create(
            builder,
            &Self::FlatBufferArgType {
                uid: self.uid,
                gid: self.gid,
                payload_type: self.payload_type,
                payload: Some(payload_offset),
            },
        )
    }
}

/// Helper struct for constructing SpawnAndroidNative spawn payloads
#[derive(Debug)]
pub struct SpawnAndroidNativePacker {
    /// Name of the NativeAndroidApplication package
    pub package: String,
}

impl<'builder> FlatBufferAssociatedType<'builder> for SpawnAndroidNativePacker {
    type FlatBufferType = SpawnAndroidNative<'builder>;
    type FlatBufferArgType = SpawnAndroidNativeArgs<'builder>;
}

impl ToFlatBufferUnion<'_, SpawnPayload> for SpawnAndroidNativePacker {
    const UNION_TAG: SpawnPayload = SpawnPayload::SpawnAndroidNative;
}

impl<'builder> ToFlatBuffer<'builder> for SpawnAndroidNativePacker {
    fn marshal(
        &self,
        builder: &mut flatbuffers::FlatBufferBuilder<'builder>,
    ) -> flatbuffers::WIPOffset<Self::FlatBufferType> {
        let packed_package = (&self.package).to_packed(builder);
        Self::FlatBufferType::create(
            builder,
            &Self::FlatBufferArgType { package: Some(packed_package) },
        )
    }
}

/// Helper struct for constructing SpawnLibApp spawn payloads.
#[derive(Debug)]
pub struct SpawnLibAppPacker {
    /// Path to the LibApp shared library
    pub path: String,
    /// Arguments for the LibApp
    pub args: Vec<String>,
}

impl<'builder> FlatBufferAssociatedType<'builder> for SpawnLibAppPacker {
    type FlatBufferType = SpawnLibApp<'builder>;
    type FlatBufferArgType = SpawnLibAppArgs<'builder>;
}

impl ToFlatBufferUnion<'_, SpawnPayload> for SpawnLibAppPacker {
    const UNION_TAG: SpawnPayload = SpawnPayload::SpawnLibApp;
}

impl<'builder> ToFlatBuffer<'builder> for SpawnLibAppPacker {
    fn marshal(
        &self,
        builder: &mut flatbuffers::FlatBufferBuilder<'builder>,
    ) -> flatbuffers::WIPOffset<Self::FlatBufferType> {
        let packed_path = (&self.path).to_packed(builder);
        let packed_args = (&self.args).to_packed(builder);
        Self::FlatBufferType::create(
            builder,
            &Self::FlatBufferArgType { path: Some(packed_path), args: Some(packed_args) },
        )
    }
}

/// Helper struct for constructing SpawnMock spawn payloads.
#[derive(Debug)]
pub struct SpawnMockPacker {
    /// The name to print in the new process
    pub name: String,
}

impl<'builder> FlatBufferAssociatedType<'builder> for SpawnMockPacker {
    type FlatBufferType = SpawnMock<'builder>;
    type FlatBufferArgType = SpawnMockArgs<'builder>;
}

impl ToFlatBufferUnion<'_, SpawnPayload> for SpawnMockPacker {
    const UNION_TAG: SpawnPayload = SpawnPayload::SpawnMock;
}

impl<'builder> ToFlatBuffer<'builder> for SpawnMockPacker {
    fn marshal(
        &self,
        builder: &mut flatbuffers::FlatBufferBuilder<'builder>,
    ) -> flatbuffers::WIPOffset<Self::FlatBufferType> {
        let packed_name = (&self.name).to_packed(builder);
        Self::FlatBufferType::create(builder, &Self::FlatBufferArgType { name: Some(packed_name) })
    }
}

/// Helper struct for constructing SpawnResponse messages.
#[derive(Debug)]
pub struct SpawnResponsePacker {
    /// The pid of the spawned process.
    pub pid: i32,
}

impl<'builder> FlatBufferAssociatedType<'builder> for SpawnResponsePacker {
    type FlatBufferType = SpawnResponse<'builder>;
    type FlatBufferArgType = SpawnResponseArgs;
}

impl ToFlatBufferUnion<'_, Message> for SpawnResponsePacker {
    const UNION_TAG: Message = Message::SpawnResponse;
}

impl<'builder> ToFlatBuffer<'builder> for SpawnResponsePacker {
    fn marshal(
        &self,
        builder: &mut flatbuffers::FlatBufferBuilder<'builder>,
    ) -> flatbuffers::WIPOffset<Self::FlatBufferType> {
        Self::FlatBufferType::create(builder, &Self::FlatBufferArgType { pid: self.pid })
    }
}

/// Helper struct for constructing Stat messages.
#[derive(Debug)]
pub struct StatPacker;

impl<'builder> FlatBufferAssociatedType<'builder> for StatPacker {
    type FlatBufferType = Stat<'builder>;
    type FlatBufferArgType = StatArgs;
}

impl ToFlatBufferUnion<'_, Message> for StatPacker {
    const UNION_TAG: Message = Message::Stat;
}

impl<'builder> ToFlatBuffer<'builder> for StatPacker {
    fn marshal(
        &self,
        builder: &mut flatbuffers::FlatBufferBuilder<'builder>,
    ) -> flatbuffers::WIPOffset<Self::FlatBufferType> {
        Self::FlatBufferType::create(builder, &Self::FlatBufferArgType {})
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
