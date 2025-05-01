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

use zygote::{config::Config, file_descriptors::FileDescriptorRegistry, server::Server};

fn main() -> Result<()> {
    // Planning List:
    //
    // * Parse arguments
    // * Initialize Zygote process state
    // * Preload libraries for species
    // * Initialize process for species
    // * Enter server loop
    //   * Clean up after children
    //   * Spawn new process
    //   * ...

    let config = Config::parse();

    let _server = Server::new(&config);

    FileDescriptorRegistry::scan();

    println!("Selected species: {}", config.species.name());

    Ok(())
}
