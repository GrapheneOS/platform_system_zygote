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

//! Implementation of behaviors for child processes

use core::ffi::CStr;

use log::warn;

use capwrap::{self, CapabilitiesSet, Capability, CapabilityFlags};
use zygote_sys as sys;

use crate::{
    messages::SpawnParamsCommon,
    species::{ReInitWrapper, SpeciesRef},
};

const ZYGOTE_CHILD_PROCESS_INITIAL_NAME: &CStr = c"zygote-child";

/// Perform child-process initialization tasks that are available on all
/// supported platforms. All species-specific re-initialization code must
/// be called before calling [`re_init_common`].
pub(crate) fn re_initialize(
    species: SpeciesRef,
    re_init_data: ReInitWrapper,
    spawn_params: &SpawnParamsCommon,
) {
    // Perform any species-specific re-initialization before we adjust
    // capabilities and user/group IDs.
    species.re_initialize_prologue(re_init_data);

    // Set the process name
    sys::set_new_process_name(ZYGOTE_CHILD_PROCESS_INITIAL_NAME);

    // Tell the kernel that this thread should keep its capabilities after it
    // changes it UID.
    match sys::prctl_set_securebits(libc::SECBIT_KEEP_CAPS) {
        Err(errno) if errno.is(libc::EPERM) => {
            warn!("Insufficient permissions to set SECBIT_KEEP_CAPS in child process");
        }
        Err(errno) => {
            panic!("Failed to set secure bits in child process: {errno}");
        }
        _ => {}
    }

    // Temporarily add the permitted capabilities to our inherited capabilities
    // set.
    if let Some(cap_permitted) = spawn_params.cap_permitted {
        CapabilitiesSet::new(CapabilityFlags::empty(), CapabilityFlags::empty(), cap_permitted)
            .store_additive()
            .unwrap();
    }

    // Drop capabilities bounding set if requested
    if let Some(cap_bound) = spawn_params.cap_bound {
        for flag in cap_bound.complement().iter() {
            let cap = Capability::try_from(flag.bits().trailing_zeros()).unwrap();
            if capwrap::cap_within_bound(cap) {
                capwrap::cap_drop_bound(cap).unwrap();
            }
        }
    }

    // Add the process to any secondary groups if requested
    if !spawn_params.secondary_groups.is_empty() {
        sys::setgroups(spawn_params.secondary_groups.as_slice()).unwrap();
    }

    // Set rlimits
    for rlimit in &spawn_params.rlimits {
        sys::setrlimit(
            rlimit.resource,
            &libc::rlimit { rlim_cur: rlimit.soft, rlim_max: rlimit.hard },
        )
        .unwrap();
    }

    if let Some(gid) = spawn_params.gid {
        let gid = gid as libc::gid_t;
        sys::setresgid(gid, gid, gid).unwrap();
    }

    // Set SecComp filters
    // Must be called when the new process still has CAP_SYS_ADMIN, in this case,
    // before changing uid from 0, which clears capabilities.  The other
    // alternative is to call prctl(PR_SET_NO_NEW_PRIVS, 1) afterward, but that
    // breaks SELinux domain transition (see b/71859146).  As the result,
    // privileged syscalls used below still need to be accessible in app process.
    species.set_seccomp_filters(spawn_params);

    // TODO: Set the scheduling policy
    // Must be called before losing the permission to set scheduler policy.

    if let Some(uid) = spawn_params.uid {
        let uid = uid as libc::uid_t;
        sys::setresuid(uid, uid, uid).unwrap();
    }

    CapabilitiesSet::load()
        .unwrap()
        .overwrite_some(
            spawn_params.cap_effective,
            spawn_params.cap_permitted,
            spawn_params.cap_inheritable,
        )
        .store_overwrite()
        .unwrap();

    // TODO: Set SELinux context
}
