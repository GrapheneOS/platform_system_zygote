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

//! This module provides core functionality for different Zygote executables.

use std::{str::FromStr, sync::atomic::AtomicBool};

use anyhow::{bail, Result};
#[cfg(target_os = "android")]
use atrace_tracing_subscriber::AtraceSubscriber;
use tracing_subscriber::{
    layer::{Layer, SubscriberExt},
    util::SubscriberInitExt,
};

static REPORTING_INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Parse a string into a [`log::LevelFilter`]
pub fn log_level_parser(parse_arg: &str) -> Result<log::LevelFilter> {
    log::LevelFilter::from_str(parse_arg).or_else(|_| match parse_arg {
        "0" => Ok(log::LevelFilter::Off),
        "1" => Ok(log::LevelFilter::Error),
        "2" => Ok(log::LevelFilter::Warn),
        "3" => Ok(log::LevelFilter::Info),
        "4" => Ok(log::LevelFilter::Debug),
        "5" => Ok(log::LevelFilter::Trace),
        level => bail!("Invalid log level: {}", level),
    })
}

/// Parse a string into a [`tracing::level_filters::LevelFilter`]
pub fn trace_level_parser(parse_arg: &str) -> Result<tracing::level_filters::LevelFilter> {
    tracing::level_filters::LevelFilter::from_str(parse_arg).or_else(|_| match parse_arg {
        "0" => Ok(tracing::level_filters::LevelFilter::OFF),
        "1" => Ok(tracing::level_filters::LevelFilter::ERROR),
        "2" => Ok(tracing::level_filters::LevelFilter::WARN),
        "3" => Ok(tracing::level_filters::LevelFilter::INFO),
        "4" => Ok(tracing::level_filters::LevelFilter::DEBUG),
        "5" => Ok(tracing::level_filters::LevelFilter::TRACE),
        level => bail!("Invalid trace level: {}", level),
    })
}

/// Initialize the `log` and `tracing` crates.  This should be called before
/// any log messages or tracing spans are emitted.  May only be called once.
pub fn init_reporting<T: Into<Vec<u8>>>(
    name: T,
    log_level: log::LevelFilter,
    trace_level: tracing::level_filters::LevelFilter,
) -> tracing::subscriber::DefaultGuard {
    if REPORTING_INITIALIZED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        // Given the current design of the Zygote server there is no legitimate
        // reason to call this twice, so we panic to help with debugging.
        panic!("Reporting initialization invoked multiple times");
    } else {
        logger::init(logger::Config::default().with_tag_on_device(name).with_max_level(log_level));

        let fmt_registry = tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .with_span_events(tracing_subscriber::fmt::format::FmtSpan::ACTIVE)
                .with_filter(trace_level),
        );

        // While the Native Zygote has nothing to do with the Dalvik runtime
        // there are a limited number of values in the AtraceTag bitfield and
        // this tag has become a catch-all for system service and runtime
        // related tracing.
        #[cfg(target_os = "android")]
        let fmt_registry = fmt_registry.with(AtraceSubscriber::new(atrace::AtraceTag::Dalvik));

        fmt_registry.set_default()
    }
}

/// Initialize the `log` crate for testing.  May be called multiple times.
pub fn init_reporting_for_testing() {
    if !REPORTING_INITIALIZED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        logger::init(logger::Config::default().with_tag_on_device("zygote_next_test"));
    }
}
