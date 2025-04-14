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

//! This module provides classes and functions for configuring a Zygote
//! process.

use clap::{ArgAction, Parser};

use crate::species::SpeciesRef;

/// Configuration values used to determine the runtime behavior of a Zygote.
/// Parsing implementations are derived using the `clap` crate.
#[derive(Parser)]
pub struct Config {
    /// A runtime-defined reference to Species-specific behavior
    /// implementations.
    #[arg(short, long)]
    pub species: SpeciesRef,

    /// A flag controlling logging levels.
    #[arg(short, long, action = ArgAction::SetTrue)]
    pub verbose: bool,
}
