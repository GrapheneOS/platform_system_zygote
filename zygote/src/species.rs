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

//! "Species" are an abstraction over the preloaded resources, process
//! creation details, and control flow transfer mechanism utilized by different
//! clients of the Zygote process server architecture.

use core::ffi::{c_int, CStr};
use std::{env, fmt::Debug, str::FromStr};

use anyhow::Result;

use crate::file_descriptors;
use zygote_messages::{SpawnParamsCommon, SpawnPayload, SpawnPayloadParser};

#[cfg(all(target_os = "android", feature = "android-native"))]
pub mod android_native;
#[cfg(feature = "libapp")]
pub mod lib_app;
#[cfg(feature = "mock")]
pub mod mock;

/// An entry structure for allow lists.
#[allow(dead_code)]
pub(crate) struct AllowListEntry<T: Debug + ?Sized + 'static> {
    /// Path, name, or other data associated with the entry.
    data: &'static T,
    action: file_descriptors::Action,

    /// A brief description of why the path/name is in the allow list.
    #[cfg(any(test, feature = "test"))]
    reason: &'static str,
    /// The username of the person who last reviewed the entry.
    #[cfg(any(test, feature = "test"))]
    reviewer: &'static str,
    /// The date the entry was last reviewed.  Expected format: "YYYY-MM-DD"
    #[cfg(any(test, feature = "test"))]
    reviewed: &'static str,
}

impl<T: Debug + ?Sized + 'static> std::borrow::Borrow<T> for &AllowListEntry<T> {
    fn borrow(&self) -> &T {
        self.data
    }
}

impl<T: Debug + ?Sized + 'static> AllowListEntry<T> {
    /// A constructor for [`FileAllowListEntry`] structs
    //
    // Depending on feature selection this function may not be used.
    #[allow(dead_code, unused_variables)]
    pub(crate) const fn new(
        data: &'static T,
        action: file_descriptors::Action,
        reason: &'static str,
        reviewer: &'static str,
        reviewed: &'static str,
    ) -> Self {
        #[cfg(not(any(test, feature = "test")))]
        return AllowListEntry { data, action };

        #[cfg(any(test, feature = "test"))]
        return AllowListEntry { data, action, reason, reviewer, reviewed };
    }
}

/// A collection of callbacks implemented by Zygote payloads that determine
/// runtime behaviors such as preloading, process creation, and transfer
/// of control flow.
pub trait Species: std::fmt::Debug {
    /// Returns true if a bound abstract socket name is allowed to be registered
    fn bound_abstract_socket_is_allowed(&self, name: &str) -> bool;

    /// Returns true if a bound socket path is allowed to be registered
    fn bound_socket_path_is_allowed(&self, name: &str) -> bool;

    /// Gather data that will later be used to re-initialize the child process
    fn gather_reinitialization_data(&self) -> ReInitWrapper;

    /// Attempt to fetch the socket FD for the Zygote from the environment
    fn get_socket_env_var_prefix(&self) -> &'static str {
        "ZYGOTE_SOCKET_"
    }

    /// Return true if the provided payload is associated with this species
    fn is_spawn_payload_type(&self, message: &SpawnPayload) -> bool;

    /// Returns true if the file is allowed to be registered
    fn file_is_allowed(&self, path: &CStr) -> bool;

    /// Take over control flow for the new process
    fn gestate(&self, spawn_params: &SpawnParamsCommon, spawn_payload: &SpawnPayload) -> !;

    /// Returns the default action for a given file path
    fn get_file_action(&self, path: &CStr) -> Option<crate::file_descriptors::Action>;

    /// Returns the default action for the given path to a connected socket
    fn get_peer_socket_action(&self, path: &str) -> Option<crate::file_descriptors::Action>;

    /// A callback called when the zygote server in the parent process is ready.
    fn on_server_ready(&self) {}

    /// A callback called when the zygote server in the parent process is being destroyed.
    fn on_server_destroy(&self) {}

    /// Returns true if a peer socket path is allowed to be registered
    fn peer_socket_path_is_allowed(&self, name: &str) -> bool;

    /// Child-process re-initialization logic that runs before the rest of the
    /// code in [`child_process::re_initialize`]
    fn re_initialize_epilogue(
        &self,
        spawn_params: &SpawnParamsCommon,
        re_init_data: &ReInitWrapper,
    );

    /// Child-process re-initialization logic that runs before the rest of the
    /// code in [`child_process::re_initialize`]
    fn re_initialize_prologue(
        &self,
        spawn_params: &SpawnParamsCommon,
        re_init_data: &ReInitWrapper,
    );

    /// A callback for setting SecComp filters
    fn set_seccomp_filters(&self, spawn_params: &SpawnParamsCommon, spawn_payload: &SpawnPayload);

    /// Perform post-fork work in the new child zygote process
    fn speciate(&self, payload: &SpawnPayload);

    /// Syncs the internal state of file descriptors in libraries like liblog.
    fn sync_fd_state(&self);

    /// Get the associated [`SpeciesTag`].  Dyn trait references are not
    /// guaranteed to be equal, so this allows for dynamic testing of the
    /// species implementation.
    fn tag(&self) -> SpeciesTag;

    /// Handle a SIGCHLD signal.
    fn handle_sigchld(&self, _pid: libc::pid_t, _uid: libc::uid_t, _status: c_int) {}

    //
    // Helper functions
    //

    /// Checks for stale entries, printing any that it finds.  Returns true if
    /// all of the entries are fresh.
    #[cfg(any(test, feature = "test"))]
    fn allowlists_are_fresh(&self) -> bool;

    /// Attempt to fetch the socket FD for a given Zygote from the environment
    fn get_socket_env_var(&self, name: &String) -> Option<String> {
        env::var(format!("{}{}", self.get_socket_env_var_prefix(), name)).ok()
    }

    /// Returns the name of the species
    fn name(&self) -> &'static str {
        self.tag().name()
    }

    /// A simple test to check of a provided path string is absolute or not.
    fn path_is_absolute(&self, path_str: &str) -> bool {
        path_str.starts_with("/") && !path_str.contains("/../")
    }
}

/// A reference type for a statically allocated Species VTable.
pub type SpeciesRef = &'static (dyn Species + Sync);

/// All configured species.
const SPECIES_LIST: &[SpeciesRef] = &[
    #[cfg(all(target_os = "android", feature = "android-native"))]
    &android_native::App,
    #[cfg(feature = "libapp")]
    &lib_app::App,
    #[cfg(feature = "mock")]
    &mock::Turtle,
];

/// A tag used to dynamically identify species.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeciesTag {
    /// Tag returned by the AndroidNative species
    AndroidNative,
    /// Tag returned by the LibApp species
    LibApp,
    /// Tag returned by the Mock species
    Mock,
}

impl SpeciesTag {
    fn name(&self) -> &'static str {
        match self {
            SpeciesTag::AndroidNative => "android-native-app",
            SpeciesTag::LibApp => "lib-app",
            SpeciesTag::Mock => "mock",
        }
    }
}

impl FromStr for SpeciesRef {
    type Err = String;

    /// Iterates through [`SPECIES_LIST`] to find a reference to a species with
    /// the provided name.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        for species in SPECIES_LIST {
            if species.name() == s {
                return Ok(*species);
            }
        }

        Err(format!("No species defined with name '{s}'"))
    }
}

impl From<&SpawnPayloadParser> for SpeciesRef {
    fn from(value: &SpawnPayloadParser) -> Self {
        match value {
            #[cfg(all(target_os = "android", feature = "android-native"))]
            SpawnPayloadParser::AndroidNative { .. }
            | SpawnPayloadParser::AndroidNativeSubspecies { .. } => &android_native::App,
            #[cfg(feature = "libapp")]
            SpawnPayloadParser::LibApp { .. } => &lib_app::App,
            #[cfg(any(test, feature = "mock"))]
            SpawnPayloadParser::Mock { .. } => &mock::Turtle,
        }
    }
}

/// A helper trait for converting types into species references.
pub trait ToSpecies {
    /// Use the value to fetch a species reference.
    fn to_species(self) -> SpeciesRef;
}

impl<T> ToSpecies for T
where
    SpeciesRef: From<T>,
{
    fn to_species(self) -> SpeciesRef {
        SpeciesRef::from(self)
    }
}

/// Re-initialization data that is gathered and then consumed by species code
#[derive(Debug)]
pub enum ReInitWrapper {
    /// Android specific data
    #[cfg(all(target_os = "android", feature = "android-native"))]
    AndroidNative(android_native::ReInitData),
    /// LibApp specific data
    #[cfg(feature = "libapp")]
    LibApp,
    /// Mock specific data
    #[cfg(feature = "mock")]
    Mock,
}

impl ReInitWrapper {
    /// Retrieve a reference to this enum's [`android_native::ReInitData`]
    /// struct
    #[cfg(all(target_os = "android", feature = "android-native"))]
    #[allow(unreachable_patterns)]
    pub fn as_android_native(&self) -> anyhow::Result<&android_native::ReInitData> {
        match self {
            ReInitWrapper::AndroidNative(data) => Ok(data),
            _ => Err(anyhow::anyhow!("Invalid re-initialization data type")),
        }
    }
}

#[cfg(any(test, feature = "test"))]
pub(crate) mod test {
    use super::*;

    /// Maximum length of time between allow-list entry audits
    const ALLOWLIST_AUDIT_DAYS_STALE: i64 = 365;
    const ALLOWLIST_AUDIT_DAYS_WARN: i64 = 335;

    enum DateFreshness {
        Fresh,
        Warn,
        Stale,
    }

    impl TryFrom<&str> for DateFreshness {
        type Error = anyhow::Error;

        fn try_from(value: &str) -> Result<Self, Self::Error> {
            let reviewed_date = chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(
                chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")?.into(),
                chrono::Utc,
            );

            let review_delta = chrono::Utc::now() - reviewed_date;
            if review_delta >= chrono::TimeDelta::days(ALLOWLIST_AUDIT_DAYS_STALE) {
                Ok(DateFreshness::Stale)
            } else if review_delta >= chrono::TimeDelta::days(ALLOWLIST_AUDIT_DAYS_WARN) {
                Ok(DateFreshness::Warn)
            } else {
                Ok(DateFreshness::Fresh)
            }
        }
    }

    pub(crate) fn allowlist_entries_are_fresh<'a, T: Debug + ?Sized + 'static>(
        list: impl Iterator<Item = &'a AllowListEntry<T>>,
    ) -> bool {
        list.fold(true, |acc, entry| match entry.reviewed.try_into() {
            Ok(DateFreshness::Fresh) => acc,
            Ok(DateFreshness::Warn) => {
                log::warn!("Allow list entry is not fresh: {:?}", entry.data);
                acc
            }
            Ok(DateFreshness::Stale) => {
                log::error!("Allow list entry is stale: {:?}", entry.data);
                false
            }
            Err(e) => {
                log::error!("{e}");
                false
            }
        })
    }

    #[test]
    fn allow_list_audit() {
        crate::config::init_reporting_for_testing();

        assert!(SPECIES_LIST
            .iter()
            .fold(true, |acc, species| species.allowlists_are_fresh() && acc))
    }

    #[test]
    fn test_date_is_fresh_today() {
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let freshness: Result<DateFreshness, _> = today.as_str().try_into();
        assert!(matches!(freshness, Ok(DateFreshness::Fresh)));
    }

    #[test]
    fn test_date_is_fresh_recent() {
        let recent_date =
            (chrono::Utc::now() - chrono::TimeDelta::days(10)).format("%Y-%m-%d").to_string();
        let freshness: Result<DateFreshness, _> = recent_date.as_str().try_into();
        assert!(matches!(freshness, Ok(DateFreshness::Fresh)));
    }

    #[test]
    fn test_date_is_stale() {
        let old_date = (chrono::Utc::now() - chrono::TimeDelta::days(ALLOWLIST_AUDIT_DAYS_STALE))
            .format("%Y-%m-%d")
            .to_string();
        let freshness: Result<DateFreshness, _> = old_date.as_str().try_into();
        assert!(matches!(freshness, Ok(DateFreshness::Stale)));
    }

    #[test]
    fn test_date_is_at_freshness_boundary() {
        let just_fresh_date = (chrono::Utc::now()
            - chrono::TimeDelta::days(ALLOWLIST_AUDIT_DAYS_WARN - 1))
        .format("%Y-%m-%d")
        .to_string();
        let freshness: Result<DateFreshness, _> = just_fresh_date.as_str().try_into();
        assert!(matches!(freshness, Ok(DateFreshness::Fresh)));

        let just_warn_date = (chrono::Utc::now()
            - chrono::TimeDelta::days(ALLOWLIST_AUDIT_DAYS_WARN))
        .format("%Y-%m-%d")
        .to_string();
        let freshness: Result<DateFreshness, _> = just_warn_date.as_str().try_into();
        assert!(matches!(freshness, Ok(DateFreshness::Warn)));

        let just_stale_date = (chrono::Utc::now()
            - chrono::TimeDelta::days(ALLOWLIST_AUDIT_DAYS_STALE))
        .format("%Y-%m-%d")
        .to_string();
        let freshness: Result<DateFreshness, _> = just_stale_date.as_str().try_into();
        assert!(matches!(freshness, Ok(DateFreshness::Stale)));
    }

    #[test]
    fn test_date_is_fresh_panics_on_invalid_date() {
        let freshness: Result<DateFreshness, _> = "invalid-date".try_into();
        assert!(freshness.is_err());
    }
}
