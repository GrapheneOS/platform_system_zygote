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

//! Safe wrappers around functions exported by libcap
//!
//! To build with cargo, you need to install libcap-dev

// The `bitflags!` macro generate an associated constant that we can't
// document.
#![allow(missing_docs)]

use bitflags::bitflags;

use zygote_sys::{libc_result_from_int_with_payload, libc_result_from_int_with_void, LibcResult};

mod sys;

use sys::{__user_cap_data_struct, __user_cap_header_struct, cap_value_t};

type CapDataPair = [__user_cap_data_struct; 2];

const CAP_HEADER_V3: __user_cap_header_struct = cap_header(CapabilitiesVersion::V3, 0);

bitflags! {
    /// Bitflags representing Linux capabilities
    ///
    /// See: `man capabilities`
    #[repr(transparent)]
    #[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct CapabilityFlags: u64 {
        const CHOWN = 1 << 0;
        const DAC_OVERRIDE = 1 << 1;
        const DAC_READ_SEARCH = 1 << 2;
        const FOWNER = 1 << 3;
        const FSETID = 1 << 4;
        const KILL = 1 << 5;
        const SETGID = 1 << 6;
        const SETUID = 1 << 7;
        const SETPCAP = 1 << 8;
        const LINUX_IMMUTABLE = 1 << 9;
        const NET_BIND_SERVICE = 1 << 10;
        const NET_BROADCAST = 1 << 11;
        const NET_ADMIN = 1 << 12;
        const NET_RAW = 1 << 13;
        const IPC_LOCK = 1 << 14;
        const IPC_OWNER = 1 << 15;
        const SYS_MODULE = 1 << 16;
        const SYS_RAWIO = 1 << 17;
        const SYS_CHROOT = 1 << 18;
        const SYS_PTRACE = 1 << 19;
        const SYS_PACCT = 1 << 20;
        const SYS_ADMIN = 1 << 21;
        const SYS_BOOT = 1 << 22;
        const SYS_NICE = 1 << 23;
        const SYS_RESOURCE = 1 << 24;
        const SYS_TIME = 1 << 25;
        const SYS_TTY_CONFIG = 1 << 26;
        const MKNOD = 1 << 27;
        const LEASE = 1 << 28;
        const AUDIT_WRITE = 1 << 29;
        const AUDIT_CONTROL = 1 << 30;
        const SETFCAP = 1 << 31;
        const MAC_OVERRIDE = 1 << 32;
        const MAC_ADMIN = 1 << 33;
        const SYSLOG = 1 << 34;
        const WAKE_ALARM = 1 << 35;
        const BLOCK_SUSPEND = 1 << 36;
        const AUDIT_READ = 1 << 37;
        const PERFMON = 1 << 38;
        const BPF = 1 << 39;
        const CHECKPOINT_RESTORE = 1 << 40;

    }
}

/// A struct containing effective, permitted, and
/// inheritable permissions
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CapabilitiesSet {
    effective: CapabilityFlags,
    permitted: CapabilityFlags,
    inheritable: CapabilityFlags,
}

impl CapabilitiesSet {
    /*
     * Constructors
     */

    /// Creates a new [`CapabilitiesSet`] with all flags set to zero
    pub const fn const_default() -> Self {
        Self {
            effective: CapabilityFlags::empty(),
            permitted: CapabilityFlags::empty(),
            inheritable: CapabilityFlags::empty(),
        }
    }

    /// Construct a [`CapabilitiesSet`] from a pair of `libcap` structs
    fn from_raw(data: &CapDataPair) -> Self {
        Self {
            effective: CapabilityFlags::from_bits_truncate(
                data[0].effective as u64 | (data[1].effective as u64) << 32,
            ),
            permitted: CapabilityFlags::from_bits_truncate(
                data[0].permitted as u64 | (data[1].permitted as u64) << 32,
            ),
            inheritable: CapabilityFlags::from_bits_truncate(
                data[0].inheritable as u64 | (data[1].inheritable as u64) << 32,
            ),
        }
    }

    /// A safe wrapper around the `libcap` implementation of `capget`
    ///
    /// See: `man capget`
    pub fn load() -> LibcResult<Self> {
        let mut header = CAP_HEADER_V3;
        let mut data = sys::CAP_DATA_EMPTY;

        libc_result_from_int_with_payload(
            // SAFETY: The pointers passed to this function are guaranteed to
            //         be valid as they refer to regions allocated on the local
            //         stack frame. The return value is checked and wrapped in
            //         a `LibcResult`.
            unsafe { sys::capget(std::ptr::addr_of_mut!(header), data.as_mut_ptr()) },
            || Self::from_raw(&data),
        )
    }

    /// Creates a new [`CapabilitiesSet`] from the raw bitmasks
    pub fn new(effective: u64, permitted: u64, inheritable: u64) -> Self {
        Self {
            effective: CapabilityFlags::from_bits_truncate(effective),
            permitted: CapabilityFlags::from_bits_truncate(permitted),
            inheritable: CapabilityFlags::from_bits_truncate(inheritable),
        }
    }

    /// Creates a new [`CapabilitiesSet`] by combining the flags of `self` and
    /// `other`.
    pub fn with(&self, other: &Self) -> Self {
        Self {
            effective: self.effective | other.effective,
            permitted: self.permitted | other.permitted,
            inheritable: self.inheritable | other.inheritable,
        }
    }

    /// Return a new [`CapabilitiesSet`] with the provided flags added to the
    /// effective set
    pub fn with_effective(&self, flags: CapabilityFlags) -> Self {
        Self { effective: self.effective | flags, ..*self }
    }

    /// Return a new [`CapabilitiesSet`] with the provided flags added to the
    /// permitted set
    pub fn with_permitted(&self, flags: CapabilityFlags) -> Self {
        Self { permitted: self.permitted | flags, ..*self }
    }

    /// Return a new [`CapabilitiesSet`] with the provided flags added to the
    /// inheritable set
    pub fn with_inheritable(&self, flags: CapabilityFlags) -> Self {
        Self { inheritable: self.inheritable | flags, ..*self }
    }

    /// Return a new [`CapabilitiesSet`] without the provided flags added to
    /// wrapped capabilities sets
    pub fn without(&self, other: &Self) -> Self {
        Self {
            effective: self.effective & !other.effective,
            permitted: self.permitted & !other.permitted,
            inheritable: self.inheritable & !other.inheritable,
        }
    }

    /// Return a new [`CapabilitiesSet`] without the provided flags included in
    /// the effective set
    pub fn without_effective(&self, flags: CapabilityFlags) -> Self {
        Self { effective: self.effective & !flags, ..*self }
    }

    /// Return a new [`CapabilitiesSet`] without the provided flags included in
    /// the permitted set
    pub fn without_permitted(&self, flags: CapabilityFlags) -> Self {
        Self { permitted: self.permitted & !flags, ..*self }
    }

    /// Return a new [`CapabilitiesSet`] without the provided flags included in
    /// the permitted set
    pub fn without_inheritable(&self, flags: CapabilityFlags) -> Self {
        Self { inheritable: self.inheritable & !flags, ..*self }
    }

    /*
     * Kernel Stores
     */

    /// A safe wrapper around the `libcap` implementation of `capset`.  This
    /// function will read the existing capabilities, add this struct's
    /// capabilities, and pass the resulting data to the kernel.
    ///
    /// See: `man capset`
    pub fn store_additive(&self) -> LibcResult<()> {
        Self::load()?.add(self).store_overwrite()
    }

    /// A safe wrapper around the `libcap` implementation of `capset`.  This
    /// function will pass the struct's data to the kernel and overwrite the
    /// existing capabilities.
    ///
    /// See: `man capset`
    pub fn store_overwrite(&self) -> LibcResult<()> {
        let mut header = CAP_HEADER_V3;
        let data = self.as_user_cap_data();

        // SAFETY: The pointers passed to this function are guaranteed to be
        //         valid as they refer to regions allocated on the local stack
        //         frame. The return value is checked and wrapped in a
        //         `LibcResult`.
        libc_result_from_int_with_void(unsafe {
            sys::capset(std::ptr::addr_of_mut!(header), data.as_ptr())
        })
    }

    /*
     * Getters and Setters
     */

    /// Returns the effective capabilities
    pub fn effective(&self) -> CapabilityFlags {
        self.effective
    }

    /// Returns the permitted capabilities
    pub fn permitted(&self) -> CapabilityFlags {
        self.permitted
    }

    /// Returns the inheritable capabilities
    pub fn inheritable(&self) -> CapabilityFlags {
        self.inheritable
    }

    /// Adds the flags of `other` to this struct's bitmasks
    pub fn add(&mut self, other: &Self) -> &Self {
        self.effective |= other.effective;
        self.permitted |= other.permitted;
        self.inheritable |= other.inheritable;

        self
    }

    /// Add the provided flags to the effective set
    pub fn add_effective(&mut self, flags: CapabilityFlags) -> &Self {
        self.effective |= flags;

        self
    }

    /// Add the provided flags to the permitted set
    pub fn add_permitted(&mut self, flags: CapabilityFlags) -> &Self {
        self.permitted |= flags;

        self
    }

    /// Add the provided flags to the inheritable set
    pub fn add_inheritable(&mut self, flags: CapabilityFlags) -> &Self {
        self.inheritable |= flags;

        self
    }

    /// Marshal the data into `libcap` data structures
    fn as_user_cap_data(&self) -> CapDataPair {
        [
            __user_cap_data_struct {
                effective: self.effective.bits() as u32,
                permitted: self.permitted.bits() as u32,
                inheritable: self.inheritable.bits() as u32,
            },
            __user_cap_data_struct {
                effective: (self.effective.bits() >> 32) as u32,
                permitted: (self.permitted.bits() >> 32) as u32,
                inheritable: (self.inheritable.bits() >> 32) as u32,
            },
        ]
    }

    /// Test to see if the provided flags are in the effective capabilities
    pub fn has_effective(&self, flags: CapabilityFlags) -> bool {
        self.effective.contains(flags)
    }

    /// Test to see if the provided flags are in the permitted capabilities
    pub fn has_permitted(&self, flags: CapabilityFlags) -> bool {
        self.permitted.contains(flags)
    }

    /// Test to see if the provided flags are in the inheritable capabilities
    pub fn has_inheritable(&self, flags: CapabilityFlags) -> bool {
        self.inheritable.contains(flags)
    }

    /// Removes the flags of `other` to this struct's bitmasks
    pub fn remove(&mut self, other: &Self) -> &Self {
        self.effective &= !other.effective;
        self.permitted &= !other.permitted;
        self.inheritable &= !other.inheritable;

        self
    }

    /// Remove the provided flags from the effective set
    pub fn remove_effective(&mut self, flags: CapabilityFlags) -> &Self {
        self.effective &= !flags;

        self
    }

    /// Remove the provided flags from the permitted set
    pub fn remove_permitted(&mut self, flags: CapabilityFlags) -> &Self {
        self.permitted &= !flags;

        self
    }

    /// Remove the provided flags from the inheritable set
    pub fn remove_inheritable(&mut self, flags: CapabilityFlags) -> &Self {
        self.inheritable &= !flags;

        self
    }

    /// Set the effective capabilities
    pub fn set_effective(&mut self, flags: CapabilityFlags) {
        self.effective = flags;
    }

    /// Set the permitted capabilities
    pub fn set_permitted(&mut self, flags: CapabilityFlags) {
        self.permitted = flags;
    }

    /// Set the inheritable capabilities
    pub fn set_inheritable(&mut self, flags: CapabilityFlags) {
        self.inheritable = flags;
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum CapabilitiesVersion {
    V1,
    V2,
    V3,
}

impl CapabilitiesVersion {
    pub const fn value(&self) -> u32 {
        match self {
            CapabilitiesVersion::V1 => sys::LINUX_CAPABILITIES_VERSION_1,
            CapabilitiesVersion::V2 => sys::LINUX_CAPABILITIES_VERSION_2,
            CapabilitiesVersion::V3 => sys::LINUX_CAPABILITIES_VERSION_3,
        }
    }
}

const fn cap_header(version: CapabilitiesVersion, pid: i32) -> __user_cap_header_struct {
    __user_cap_header_struct { version: version.value(), pid }
}

/// Returns the maximum capability bit that is defined in `libcap`.  If the
/// kernel returns a capability with a higher value then we need to update
/// `libcap`.
///
/// TODO: Implement tests to check when either this library or libcap are out
///       of date.
#[allow(dead_code)]
fn cap_max_bits() -> cap_value_t {
    // SAFETY: This function takes no arguments and can't fail.
    unsafe { sys::cap_max_bits() }
}
