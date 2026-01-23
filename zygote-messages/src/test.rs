//
// Copyright (C) 2026 The Android Open-Source Project
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

//! Tests for the `zygote-messages` crate

use super::*;
use arrayvec::ArrayVec;

#[test]
fn test_capability_flags_marshaling() {
    let mut builder = flatbuffers::FlatBufferBuilder::new();

    // Test None
    let marshaled = marshal_capability_flags(&None, &mut builder);
    assert_eq!(marshaled, RawCap::MAX);
    assert_eq!(unmarshal_capability_flags(&marshaled), None);

    // Test Some
    let caps = CapabilityFlags::CHOWN | CapabilityFlags::SETUID;
    let marshaled = marshal_capability_flags(&Some(caps), &mut builder);
    assert_ne!(marshaled, RawCap::MAX);
    assert_eq!(unmarshal_capability_flags(&marshaled), Some(caps));
}

#[test]
fn test_message_get_spawn_data() {
    let common = SpawnParamsCommon {
        uid: Some(1000),
        gid: Some(1000),
        process_name: Some("test".to_string()),
        priority_initial: None,
        priority_final: None,
        cap_effective: None,
        cap_permitted: None,
        cap_inheritable: None,
        cap_bound: None,
        se_info: None,
        secondary_groups: ArrayVec::new(),
        rlimits: ArrayVec::new(),
    };

    let payload = SpawnPayload::Mock { name: "mock" };

    let spawn_msg = Message::Spawn { common: common.clone(), payload };

    assert!(spawn_msg.get_spawn_params().is_some());
    assert_eq!(spawn_msg.get_spawn_params().unwrap().uid, Some(1000));
    assert!(spawn_msg.get_spawn_payload().is_some());
    if let Some(SpawnPayload::Mock { name }) = spawn_msg.get_spawn_payload() {
        assert_eq!(*name, "mock");
    } else {
        panic!("Wrong payload type");
    }

    let ack_msg = Message::AckResponse;
    assert!(ack_msg.get_spawn_params().is_none());
    assert!(ack_msg.get_spawn_payload().is_none());

    let exit_msg = Message::Exit;
    assert!(exit_msg.get_spawn_params().is_none());
    assert!(exit_msg.get_spawn_payload().is_none());
}

#[test]
fn test_message_parser_to_message() {
    let parser = MessageParser::Exit;
    let msg = parser.to_message().expect("Failed to convert Exit parser");
    assert!(matches!(msg, Message::Exit));

    let common_parser = SpawnCommonParser {
        uid: Some(100),
        gid: Some(200),
        process_name: Some("name".to_string()),
        priority_initial: Some(5),
        priority_final: Some(10),
        secondary_groups: vec![300, 400],
        se_info: Some("se".to_string()),
    };
    let payload_parser = SpawnPayloadParser::Mock { name: "mock".to_string() };
    let parser = MessageParser::Spawn { common: common_parser, payload: payload_parser };

    let msg = parser.to_message().expect("Failed to convert Spawn parser");
    if let Message::Spawn { common, payload } = msg {
        assert_eq!(common.uid, Some(100));
        assert_eq!(common.gid, Some(200));
        assert_eq!(common.process_name, Some("name".to_string()));
        assert_eq!(common.priority_initial, Some(5));
        assert_eq!(common.priority_final, Some(10));
        assert_eq!(common.se_info, Some("se".to_string()));
        assert_eq!(common.secondary_groups.len(), 2);
        if let SpawnPayload::Mock { name } = payload {
            assert_eq!(name, "mock");
        } else {
            panic!("Wrong payload");
        }
    } else {
        panic!("Wrong message type");
    }
}

#[test]
fn test_message_roundtrip_ack() {
    let msg = Message::AckResponse;
    let builder = msg.to_parcel();
    let bytes = builder.finished_data();

    let decoded = Message::try_from_parcel(bytes).expect("Failed to decode AckResponse");
    assert!(matches!(decoded, Message::AckResponse));
}

#[test]
fn test_message_roundtrip_identity_query_response() {
    let msg = Message::IdentityQueryResponse {
        name: "test_name",
        species: "test_species",
        arch: "test_arch",
    };
    let builder = msg.to_parcel();
    let bytes = builder.finished_data();

    let decoded = Message::try_from_parcel(bytes).expect("Failed to decode IdentityQueryResponse");
    if let Message::IdentityQueryResponse { name, species, arch } = decoded {
        assert_eq!(name, "test_name");
        assert_eq!(species, "test_species");
        assert_eq!(arch, "test_arch");
    } else {
        panic!("Decoded message is not IdentityQueryResponse: {:?}", decoded);
    }
}

#[test]
fn test_message_roundtrip_spawn() {
    let common = SpawnParamsCommon {
        uid: Some(1234),
        gid: Some(5678),
        process_name: Some("spawn_test".to_string()),
        priority_initial: Some(-10),
        priority_final: Some(5),
        cap_effective: Some(CapabilityFlags::CHOWN),
        cap_permitted: Some(CapabilityFlags::SETUID),
        cap_inheritable: None,
        cap_bound: None,
        se_info: Some("se_info_test".to_string()),
        secondary_groups: ArrayVec::from_iter([10, 20]),
        rlimits: ArrayVec::new(),
    };

    let payload = SpawnPayload::Mock { name: "spawn_mock" };

    let msg = Message::Spawn { common: common.clone(), payload };
    let builder = msg.to_parcel();
    let bytes = builder.finished_data();

    let decoded = Message::try_from_parcel(bytes).expect("Failed to decode Spawn message");
    if let Message::Spawn { common: decoded_common, payload: decoded_payload } = decoded {
        assert_eq!(decoded_common.uid, common.uid);
        assert_eq!(decoded_common.gid, common.gid);
        assert_eq!(decoded_common.process_name, common.process_name);
        assert_eq!(decoded_common.priority_initial, common.priority_initial);
        assert_eq!(decoded_common.priority_final, common.priority_final);
        assert_eq!(decoded_common.cap_effective, common.cap_effective);
        assert_eq!(decoded_common.cap_permitted, common.cap_permitted);
        assert_eq!(decoded_common.se_info, common.se_info);
        assert_eq!(decoded_common.secondary_groups, common.secondary_groups);

        if let SpawnPayload::Mock { name } = decoded_payload {
            assert_eq!(name, "spawn_mock");
        } else {
            panic!("Decoded payload is not Mock: {:?}", decoded_payload);
        }
    } else {
        panic!("Decoded message is not Spawn: {:?}", decoded);
    }
}

#[test]
fn test_message_roundtrip_stat_response() {
    let msg = Message::StatResponse {
        pid: 123,
        pgrp: 456,
        minflt: 1000,
        cminflt: 2000,
        majflt: 3000,
        cmajflt: 4000,
        utime: 5000,
        stime: 6000,
        num_threads: 10,
        vsize: 1000000,
        rss: 100,
    };
    let builder = msg.to_parcel();
    let bytes = builder.finished_data();

    let decoded = Message::try_from_parcel(bytes).expect("Failed to decode StatResponse");
    if let Message::StatResponse {
        pid,
        pgrp,
        minflt,
        cminflt,
        majflt,
        cmajflt,
        utime,
        stime,
        num_threads,
        vsize,
        rss,
    } = decoded
    {
        assert_eq!(pid, 123);
        assert_eq!(pgrp, 456);
        assert_eq!(minflt, 1000);
        assert_eq!(cminflt, 2000);
        assert_eq!(majflt, 3000);
        assert_eq!(cmajflt, 4000);
        assert_eq!(utime, 5000);
        assert_eq!(stime, 6000);
        assert_eq!(num_threads, 10);
        assert_eq!(vsize, 1000000);
        assert_eq!(rss, 100);
    } else {
        panic!("Decoded message is not StatResponse: {:?}", decoded);
    }
}

#[test]
fn test_message_roundtrip_spawn_subspecies() {
    let common = SpawnParamsCommon {
        uid: Some(999),
        gid: Some(888),
        process_name: Some("subspecies_test".to_string()),
        priority_initial: None,
        priority_final: None,
        cap_effective: None,
        cap_permitted: None,
        cap_inheritable: None,
        cap_bound: None,
        se_info: None,
        secondary_groups: ArrayVec::new(),
        rlimits: ArrayVec::new(),
    };

    let payload = SpawnPayload::Mock { name: "subspecies_mock" };

    let msg =
        Message::SpawnSubspecies { common: common.clone(), socket_path: "/tmp/sub.sock", payload };
    let builder = msg.to_parcel();
    let bytes = builder.finished_data();

    let decoded =
        Message::try_from_parcel(bytes).expect("Failed to decode SpawnSubspecies message");
    if let Message::SpawnSubspecies {
        common: decoded_common,
        socket_path: decoded_socket_path,
        payload: decoded_payload,
    } = decoded
    {
        assert_eq!(decoded_common.uid, common.uid);
        assert_eq!(decoded_socket_path, "/tmp/sub.sock");
        if let SpawnPayload::Mock { name } = decoded_payload {
            assert_eq!(name, "subspecies_mock");
        } else {
            panic!("Decoded payload is not Mock: {:?}", decoded_payload);
        }
    } else {
        panic!("Decoded message is not SpawnSubspecies: {:?}", decoded);
    }
}

#[test]
fn test_rlimits_roundtrip() {
    let mut rlimits = ArrayVec::<RLimitData, RLIMIT_VECTOR_SIZE>::new();
    rlimits.push(RLimitData { resource: libc::RLIMIT_NOFILE as _, soft: 1024, hard: 2048 });

    let common = SpawnParamsCommon {
        uid: None,
        gid: None,
        process_name: None,
        priority_initial: None,
        priority_final: None,
        cap_effective: None,
        cap_permitted: None,
        cap_inheritable: None,
        cap_bound: None,
        se_info: None,
        secondary_groups: ArrayVec::new(),
        rlimits: rlimits.clone(),
    };

    let msg = Message::Spawn { common, payload: SpawnPayload::Mock { name: "test" } };
    let builder = msg.to_parcel();
    let bytes = builder.finished_data();

    let decoded = Message::try_from_parcel(bytes).expect("Failed to decode Spawn message");
    if let Message::Spawn { common: decoded_common, .. } = decoded {
        assert_eq!(decoded_common.rlimits.len(), 1);
        assert_eq!(decoded_common.rlimits[0].resource, libc::RLIMIT_NOFILE as _);
        assert_eq!(decoded_common.rlimits[0].soft, 1024);
        assert_eq!(decoded_common.rlimits[0].hard, 2048);
    } else {
        panic!("Wrong message type");
    }
}

#[test]
fn test_spawn_params_common_or() {
    let params1 = SpawnParamsCommon {
        uid: Some(1000),
        gid: None,
        process_name: Some("process1".to_string()),
        priority_initial: Some(10),
        priority_final: None,
        cap_effective: None,
        cap_permitted: None,
        cap_inheritable: None,
        cap_bound: None,
        se_info: None,
        secondary_groups: ArrayVec::from_iter([100, 200]),
        rlimits: ArrayVec::new(),
    };

    let params2 = SpawnParamsCommon {
        uid: None,
        gid: Some(2000),
        process_name: Some("process2".to_string()),
        priority_initial: None,
        priority_final: Some(20),
        cap_effective: None,
        cap_permitted: None,
        cap_inheritable: None,
        cap_bound: None,
        se_info: Some("se_info2".to_string()),
        secondary_groups: ArrayVec::from_iter([200, 300]),
        rlimits: ArrayVec::new(),
    };

    let combined = params1.or(&params2);

    assert_eq!(combined.uid, Some(1000));
    assert_eq!(combined.gid, Some(2000));
    assert_eq!(combined.process_name, Some("process1".to_string()));
    assert_eq!(combined.priority_initial, Some(10));
    assert_eq!(combined.priority_final, Some(20));
    assert_eq!(combined.se_info, Some("se_info2".to_string()));

    let mut actual_groups: Vec<_> = combined.secondary_groups.iter().cloned().collect();
    actual_groups.sort();
    assert_eq!(actual_groups, vec![100, 200, 300]);
}

#[test]
fn test_spawn_params_common_or_rlimits() {
    let mut rlimits1 = ArrayVec::<RLimitData, RLIMIT_VECTOR_SIZE>::new();
    rlimits1.push(RLimitData { resource: libc::RLIMIT_NOFILE as _, soft: 1024, hard: 2048 });

    let mut rlimits2 = ArrayVec::<RLimitData, RLIMIT_VECTOR_SIZE>::new();
    rlimits2.push(RLimitData { resource: libc::RLIMIT_CPU as _, soft: 60, hard: 120 });

    let p1 = SpawnParamsCommon {
        uid: None,
        gid: None,
        process_name: None,
        priority_initial: None,
        priority_final: None,
        cap_effective: None,
        cap_permitted: None,
        cap_inheritable: None,
        cap_bound: None,
        se_info: None,
        secondary_groups: ArrayVec::new(),
        rlimits: rlimits1,
    };

    let p2 = SpawnParamsCommon {
        uid: None,
        gid: None,
        process_name: None,
        priority_initial: None,
        priority_final: None,
        cap_effective: None,
        cap_permitted: None,
        cap_inheritable: None,
        cap_bound: None,
        se_info: None,
        secondary_groups: ArrayVec::new(),
        rlimits: rlimits2,
    };

    let combined = p1.or(&p2);
    assert_eq!(combined.rlimits.len(), 2);
    assert_eq!(combined.rlimits[0].resource, libc::RLIMIT_NOFILE as _);
    assert_eq!(combined.rlimits[1].resource, libc::RLIMIT_CPU as _);
}

#[test]
fn test_spawn_params_common_string_none_roundtrip() {
    let common = SpawnParamsCommon {
        uid: None,
        gid: None,
        process_name: None,
        priority_initial: None,
        priority_final: None,
        cap_effective: None,
        cap_permitted: None,
        cap_inheritable: None,
        cap_bound: None,
        se_info: None,
        secondary_groups: ArrayVec::new(),
        rlimits: ArrayVec::new(),
    };

    let msg =
        Message::Spawn { common: common.clone(), payload: SpawnPayload::Mock { name: "test" } };
    let builder = msg.to_parcel();
    let bytes = builder.finished_data();

    let decoded = Message::try_from_parcel(bytes).expect("Failed to decode Spawn message");
    if let Message::Spawn { common: decoded_common, .. } = decoded {
        // Based on marshal_string/unmarshal_string implementation:
        // marshal_string returns Some("") for None.
        // unmarshal_string returns Some("".to_string()) for Some("").
        assert_eq!(decoded_common.process_name, Some("".to_string()));
        assert_eq!(decoded_common.se_info, Some("".to_string()));
    } else {
        panic!("Wrong message type");
    }
}
