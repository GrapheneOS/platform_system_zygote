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

//! Implementation of a fully native Zygote architecture.
//!
//! This executable can be used to preload and initialize resources before
//! forking child processes.

use anyhow::Result;
use clap::Parser;

use zygote::{config, server, sys};

fn main() -> Result<()> {
    if let Some(thunk) = run_server() {
        thunk()
    }

    Ok(())
}

/// Parse configuration values, instantiate the server, and run it.
///
/// The server is constructed in, and the child-side thunk returned from, this
/// frame to ensure that the configuration and server resources are dropped
/// before the thunk is evaluated.
fn run_server() -> Option<impl FnOnce()> {
    let config = config::Server::parse();

    logger::init(
        logger::Config::default()
            .with_tag_on_device(config.name.clone())
            .with_max_level(config.log_level),
    );

    log::info!("Starting Zygote server ({}) with PID {}", config.name, sys::getpid());

    let mut server = server::Server::new(config);
    server.serve()
}
