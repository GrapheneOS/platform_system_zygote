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

use std::{os::fd::RawFd, path::Path};

use anyhow::{anyhow, bail, Result};
use arrayvec::ArrayVec;
use flatbuffers;

use crate::{
    config::Config,
    file_descriptors::{self, FileDescriptorRegistry},
    introspection::get_proc_fd_path,
    messages::{Command, Message},
    sys::{self, LoopExit, LoopStatus, PollFdExt},
};

const COMMAND_SOCKET_BUFFER_SIZE: usize = 16;
const POLL_BUFFER_SIZE: usize = 64;
const SERVER_SOCKET_BACKLOG: core::ffi::c_int = 10;
const ZYGOTE_SOCKET_PREFIX: &str = "/dev/socket/";

type PollBuffer = ArrayVec<libc::pollfd, POLL_BUFFER_SIZE>;

impl std::convert::From<&mut Server<'_>> for PollBuffer {
    fn from(server: &mut Server) -> PollBuffer {
        let mut poll_buffer = PollBuffer::new();

        poll_buffer.push(libc::pollfd::new(server.signal_fd, libc::POLLIN));
        poll_buffer.push(libc::pollfd::new(server.server_socket, libc::POLLIN));

        for command_socket in &server.command_sockets {
            poll_buffer.push(libc::pollfd::new(*command_socket, libc::POLLIN));
        }

        poll_buffer
    }
}

struct PollPartition<'a> {
    pub signal: &'a libc::pollfd,
    pub server: &'a libc::pollfd,
    pub command: &'a [libc::pollfd],
}

trait Partition<'a> {
    fn partition(&'a self) -> PollPartition<'a>;
}

impl<'a> Partition<'a> for PollBuffer {
    fn partition(&'a self) -> PollPartition<'a> {
        PollPartition::<'a> {
            signal: &self[0],
            server: &self[1],
            command: if self.len() > 2 { &self[2..] } else { &[] },
        }
    }
}

const MESSAGE_BUFFER_SIZE: usize = 512;
type MessageBuffer = [u8; MESSAGE_BUFFER_SIZE];

/// The main data structure for the Zygote process server.
pub struct Server<'a> {
    _config: &'a Config,
    registry: FileDescriptorRegistry,

    signal_fd: RawFd,
    server_socket: RawFd,
    command_sockets: ArrayVec<RawFd, COMMAND_SOCKET_BUFFER_SIZE>,
}

impl<'a> Server<'a> {
    /// Create a new Zygote process server from a [`crate::config::Config`]
    /// reference.
    pub fn new(config: &'a Config) -> Self {
        // TODO: Only enable for device builds?
        // NOTE: Operation not permitted on gLinux machines
        // linux::process::nice(-19).expect("Unable to set Zygote nice level");

        let server_socket = Server::get_server_socket(config).unwrap();

        let sigset = sys::build_sigset(&[libc::SIGCHLD, libc::SIGTERM]).unwrap();
        sys::sigprocmask(libc::SIG_BLOCK, &sigset).unwrap();

        Self {
            _config: config,
            registry: FileDescriptorRegistry::new(config.species),

            signal_fd: sys::signalfd(-1, &sigset, libc::SFD_NONBLOCK).unwrap(),
            server_socket,
            command_sockets: ArrayVec::new(),
        }
    }

    fn get_server_socket(config: &Config) -> Result<RawFd> {
        if config.socket.is_empty() {
            let mut socket_path = std::path::PathBuf::from(ZYGOTE_SOCKET_PREFIX);
            socket_path.push(config.name.clone());

            std::fs::create_dir_all(ZYGOTE_SOCKET_PREFIX).unwrap();
            let socket_fd =
                sys::create_bound_socket(socket_path.to_str().unwrap(), libc::SOCK_SEQPACKET)?;
            sys::fcntl_setfl(socket_fd, libc::O_NONBLOCK)?;
            sys::listen(socket_fd, SERVER_SOCKET_BACKLOG)?;

            Ok(socket_fd)
        } else if let Ok(fd) = config.socket.parse::<RawFd>() {
            if !get_proc_fd_path(fd).exists() {
                bail!("Provided integer argument does not refer to an open file: {}", fd);
            }

            let stat = sys::fstat(fd).unwrap();
            if sys::get_file_type(stat) != libc::S_IFSOCK {
                bail!("Provided file descriptor does not refer to a valid socket: {}", fd);
            }

            sys::fcntl_setfl(fd, libc::O_NONBLOCK)?;
            sys::listen(fd, SERVER_SOCKET_BACKLOG)?;

            Ok(fd)
        } else {
            let arg_path = Path::new(&config.socket);

            if arg_path.exists() {
                bail!("Socket argument paths already exists: {}", &config.socket);
            }

            std::fs::create_dir_all(
                arg_path.parent().ok_or(anyhow!("Socket path must have a parent directory"))?,
            )?;

            let socket_fd = sys::create_bound_socket(&config.socket, libc::SOCK_STREAM)?;
            sys::fcntl_setfl(socket_fd, libc::O_NONBLOCK)?;
            sys::listen(socket_fd, SERVER_SOCKET_BACKLOG)?;

            Ok(socket_fd)
        }
    }

    /// Check each of the polled file descriptors to check if we should read
    /// from them.
    ///
    /// The function's return value indicates if the server should terminate
    /// after this call.
    fn handle_poll_event(&mut self, partition: PollPartition<'_>) -> Result<()> {
        self.handle_signalfd(&partition)?;
        self.handle_server_socket(&partition);
        self.handle_command_sockets(&partition)
    }

    fn handle_command_sockets(&mut self, partition: &PollPartition<'_>) -> Result<()> {
        for command_pollfd in partition.command {
            let event_handler_result = command_pollfd.handle_event(libc::POLLIN, &mut |fd| {
                sys::call_until_would_block(
                    || sys::recvmsg::<MESSAGE_BUFFER_SIZE>(fd),
                    &mut |(_, message_buffer): (usize, MessageBuffer)| {
                        let message = flatbuffers::root::<Message>(&message_buffer).unwrap();

                        match message.command_type() {
                            Command::Spawn => {
                                let spawn_cmd = message.command_as_spawn().unwrap();
                                println!("Received command: (Spawn {})", spawn_cmd.name());

                                LoopStatus::Continue
                            }
                            Command::Exit => {
                                let _ = message.command_as_exit().unwrap();
                                let creds: libc::ucred =
                                    sys::getsockopt(fd, libc::SOL_SOCKET, libc::SO_PEERCRED)
                                        .unwrap();

                                println!("Received command: (Exit {})", creds.pid);

                                LoopStatus::Break(())
                            }
                            Command::Stat => {
                                println!("Received command: (Stat)");

                                LoopStatus::Continue
                            }
                            Command(tag) => {
                                panic!("Invalid command variant encountered: {}", tag)
                            }
                        }
                    },
                )
                .unwrap()
            });

            match event_handler_result {
                Ok(None) => {
                    // No event was registered for this file descriptor
                    continue;
                }
                Ok(Some(LoopExit::WouldBlock)) => {
                    // All available messages were read from the socket
                    continue;
                }
                Ok(Some(LoopExit::Early(_))) => {
                    // Messages were read until we received a shutdown command
                    bail!("Received Exit message");
                }
                Err(errcode) => match errcode {
                    errcode if (errcode | libc::POLLHUP) == libc::POLLHUP => {
                        self.registry.remove(command_pollfd.fd).unwrap();
                        continue;
                    }
                    errcode => {
                        panic!("Unexpected error code when polling command socket: {}", errcode);
                    }
                },
            }
        }

        Ok(())
    }

    fn handle_server_socket(&mut self, partition: &PollPartition<'_>) {
        partition
            .server
            .handle_event(libc::POLLIN, &mut |fd| {
                sys::call_until_would_block(|| sys::accept(fd), &mut |new_command_fd| {
                    self.command_sockets.push(new_command_fd);
                    self.registry.register(fd, file_descriptors::Action::Close);

                    // Continue reading
                    LoopStatus::<()>::Continue
                })
                .unwrap();
            })
            // POLLERR, POLLHUP, and POLLNVAL should never occur for the server
            // socket.
            //
            // We don't need to check to see if an event was handled as not
            // receiving any new connections is valid.
            .unwrap();
    }

    fn handle_signalfd(&mut self, partition: &PollPartition<'_>) -> Result<()> {
        // Handle any pending signals
        partition
            .signal
            .handle_event(libc::POLLIN, &mut |fd| {
                sys::call_until_would_block(
                    || sys::read_exact::<libc::signalfd_siginfo>(fd),
                    &mut |siginfo: libc::signalfd_siginfo| {
                        match siginfo.ssi_signo as i32 {
                            libc::SIGCHLD => {
                                println!(
                                    "Received SIGCHLD from PID {} with status {}",
                                    siginfo.ssi_pid, siginfo.ssi_status
                                );

                                // Continue reading
                                LoopStatus::Continue
                            }
                            libc::SIGTERM => {
                                println!("Received SIGTERM FROM PID {}", siginfo.ssi_pid);

                                // Terminate early
                                LoopStatus::Break(libc::SIGTERM)
                            }
                            signo => {
                                // This should never happen as only SIGCHLD
                                // and SIGTERM are added to the signalfd's
                                // mask.
                                panic!("Unhandled signal received: {}", signo);
                            }
                        }
                    },
                )
                .unwrap()
            })
            // POLLERR, POLLHUP, and POLLNVAL should never occur for the
            // signalfd.
            .unwrap()
            // We can only receive SIGTERMs if we read from the signalfd
            // and exited early.
            .and_then(|loop_status| loop_status.would_block().then_some(()))
            .ok_or(anyhow!("Received SIGTERM"))
    }

    /// Execute the main server loop until the server receives a SIGTERM or
    /// [`crate::messages::Exit`] message.
    pub fn serve(&mut self) {
        loop {
            let mut poll_array = PollBuffer::from(&mut *self);
            // Discard the number of ready file descriptors for now.
            sys::poll(&mut poll_array, -1).unwrap();

            if self.handle_poll_event(poll_array.partition()).is_err() {
                return;
            }
        }
    }
}
