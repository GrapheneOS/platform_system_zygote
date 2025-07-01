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

use crate::messages::SpawnParamsCommon;

const ZYGOTE_CHILD_PROCESS_INITIAL_NAME: &CStr = c"zygote-child";

/// Perform child-process initialization tasks that are available on all
/// supported platforms. All species-specific re-initialization code must
/// be called before calling [`re_init_common`].
pub(crate) fn re_init_common(spawn_params: &SpawnParamsCommon) {
    // Set the process name
    sys::prctl_set_name(&ZYGOTE_CHILD_PROCESS_INITIAL_NAME.to_bytes());

    // Tell the kernel that this thread should keep its capabilities after it
    // changes it UID.
    match sys::prctl_set_securebits(libc::SECBIT_KEEP_CAPS) {
        Err(errno) if errno.is(libc::EPERM) => {
            warn!("Insufficient permissions to set SECBIT_KEEP_CAPS in child process");
        }
        Err(errno) => {
            panic!("Failed to set secure bits in child process: {}", errno);
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

    // TODO: Set SecComp filters
    // TODO: Set the scheduling policy
    // TODO: Set new real and effective uid and gid

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

/// Perform child-process initialization tasks that are specific to Android.
#[cfg(target_os = "android")]
#[inline(always)]
pub(crate) fn re_init_android(fds_error_level: sys::android::FDSanErrorLevel) {
    crate::introspection::debug_assert_single_threaded();

    sys::android::reset_stack_guards();

    // SAFETY: This is called in a single-threaded context
    unsafe {
        sys::android::fdsan_set_error_level(fds_error_level);
    }

    if sys::android::set_zygote_child().is_err() {
        log::error!("Failed to android_mallopt(M_SET_ZYGOTE_CHILD)");
    }

    if let Err(errno) = sys::mallopt(libc::M_DECAY_TIME, 1) {
        log::error!("Failed to mallopt(M_DECAY_TIME): {}", errno);
    }
}
