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
use log::{error, info};

use crate::{
    config,
    file_descriptors::{self, FileDescriptorRegistry},
    introspection::get_proc_fd_path,
    messages::{Command, Message},
    sys::{self, LoopExit, LoopStatus, PollFd},
};

const COMMAND_SOCKET_BUFFER_SIZE: usize = 16;
const POLL_BUFFER_SIZE: usize = 64;
const SERVER_SOCKET_BACKLOG: core::ffi::c_int = 10;
const ZYGOTE_SOCKET_PREFIX: &str = "/dev/socket/";

type PollBuffer = ArrayVec<PollFd, POLL_BUFFER_SIZE>;

impl std::convert::From<&mut Server> for PollBuffer {
    fn from(server: &mut Server) -> PollBuffer {
        let mut poll_buffer = PollBuffer::new();

        poll_buffer.push(PollFd::new(server.signal_fd, libc::POLLIN));
        poll_buffer.push(PollFd::new(server.server_socket, libc::POLLIN));

        for command_socket in &server.command_sockets {
            poll_buffer.push(PollFd::new(*command_socket, libc::POLLIN));
        }

        poll_buffer
    }
}

struct PollPartition<'a> {
    pub signal: &'a PollFd,
    pub server: &'a PollFd,
    pub command: &'a [PollFd],
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

#[derive(Debug, Eq, PartialEq)]
enum ServerStatus {
    Continue,
    Exit,
}

/// The main data structure for the Zygote process server.
pub struct Server {
    registry: FileDescriptorRegistry,

    signal_fd: RawFd,
    server_socket: RawFd,
    command_sockets: ArrayVec<RawFd, COMMAND_SOCKET_BUFFER_SIZE>,

    server_socket_path: Option<String>,
}

impl Server {
    /// Create a new Zygote process server from a [`crate::config::Config`]
    /// reference.
    ///
    /// Add a destructor to clean up the socket if we create it.
    pub fn new(config: &config::Server) -> Self {
        // TODO: Only enable for device builds?
        // NOTE: Operation not permitted on gLinux machines
        // linux::process::nice(-19).expect("Unable to set Zygote nice level");

        let mut registry = FileDescriptorRegistry::new(config.species);

        let (server_socket, server_socket_path) = Server::get_server_socket(config).unwrap();
        registry.register(server_socket, file_descriptors::Action::Close);

        let sigset = sys::build_sigset(&[libc::SIGCHLD, libc::SIGINT, libc::SIGTERM]).unwrap();
        sys::sigprocmask(libc::SIG_BLOCK, &sigset).unwrap();
        let signal_fd = sys::signalfd(-1, &sigset, libc::SFD_NONBLOCK).unwrap();
        registry.register(signal_fd, file_descriptors::Action::Close);

        Self {
            registry,

            signal_fd,
            server_socket,
            command_sockets: ArrayVec::new(),

            server_socket_path,
        }
    }

    fn get_server_socket(config: &config::Server) -> Result<(RawFd, Option<String>)> {
        if config.socket.is_empty() {
            let mut socket_path = std::path::PathBuf::from(ZYGOTE_SOCKET_PREFIX);
            socket_path.push(config.name.clone());

            std::fs::create_dir_all(ZYGOTE_SOCKET_PREFIX).unwrap();
            let socket_fd =
                sys::create_bound_socket(socket_path.to_str().unwrap(), libc::SOCK_SEQPACKET)?;
            sys::fcntl_setfl(socket_fd, libc::O_NONBLOCK)?;
            sys::listen(socket_fd, SERVER_SOCKET_BACKLOG)?;

            Ok((socket_fd, Some(socket_path.to_str().unwrap().to_owned())))
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

            Ok((fd, None))
        } else {
            let arg_path = Path::new(&config.socket);

            if arg_path.exists() {
                bail!("Socket argument paths already exists: {}", &config.socket);
            }

            std::fs::create_dir_all(
                arg_path.parent().ok_or(anyhow!("Socket path must have a parent directory"))?,
            )?;

            let socket_fd = sys::create_bound_socket(&config.socket, libc::SOCK_SEQPACKET)?;
            sys::fcntl_setfl(socket_fd, libc::O_NONBLOCK)?;
            sys::listen(socket_fd, SERVER_SOCKET_BACKLOG)?;

            Ok((socket_fd, Some(config.socket.clone())))
        }
    }

    /// Check each of the polled file descriptors to check if we should read
    /// from them.
    ///
    /// The function's return value indicates if the server should terminate
    /// after this call.
    fn handle_poll_event(&mut self, partition: PollPartition<'_>) -> ServerStatus {
        if self.handle_signalfd(&partition) == ServerStatus::Exit {
            return ServerStatus::Exit;
        }

        self.handle_server_socket(&partition);
        self.handle_command_sockets(&partition)
    }

    fn handle_command_sockets(&mut self, partition: &PollPartition<'_>) -> ServerStatus {
        for command_pollfd in partition.command {
            match command_pollfd.check() {
                Ok(checked_pollfd) => {
                    let pollin_result = checked_pollfd.handle_event(libc::POLLIN, &mut |fd| {
                        sys::call_until_would_block(
                            || sys::recvmsg::<MESSAGE_BUFFER_SIZE>(fd),
                            &mut |(readlen, message_buffer): (isize, MessageBuffer)| {
                                if readlen == 0 {
                                    return LoopStatus::Break(LoopStatus::Continue);
                                }

                                let message =
                                    flatbuffers::root::<Message>(&message_buffer).unwrap();

                                match message.command_type() {
                                    Command::Spawn => {
                                        let spawn_cmd = message.command_as_spawn().unwrap();
                                        info!("Received command: (Spawn {})", spawn_cmd.name());

                                        LoopStatus::Continue
                                    }
                                    Command::Exit => {
                                        let _ = message.command_as_exit().unwrap();
                                        info!(
                                            "Received command: (Exit {})",
                                            sys::get_socket_creds(fd).unwrap().pid
                                        );

                                        LoopStatus::Break(LoopStatus::Break(()))
                                    }
                                    Command::Stat => {
                                        info!("Received command: (Stat)");

                                        LoopStatus::Continue
                                    }
                                    Command(tag) => {
                                        error!("Invalid command variant encountered: {}", tag);

                                        LoopStatus::Continue
                                    }
                                }
                            },
                        )
                        .unwrap()
                    });

                    match pollin_result {
                        None => {
                            // No event was registered for this file descriptor
                        }
                        Some(LoopExit::WouldBlock) => {
                            // All available messages were read from the socket
                        }
                        Some(LoopExit::Early(LoopStatus::Continue)) => {
                            // Zero-length read from socket, continue and wait for SIGHUP
                        }
                        Some(LoopExit::Early(LoopStatus::Break(_))) => {
                            // Messages were read until we received a shutdown command
                            return ServerStatus::Exit;
                        }
                    }

                    let _ = checked_pollfd.handle_event(libc::POLLHUP, |fd| {
                        self.registry.remove(fd).unwrap();
                        self.command_sockets.remove(
                            self.command_sockets
                                .iter()
                                .position(|&search_fd| search_fd == fd)
                                .unwrap(),
                        );
                        sys::close(fd).unwrap();
                    });
                }
                Err((fd, error_events)) => {
                    // POLLERR and POLLNVAL should never occur for a command socket.
                    panic!("Received polling error for command socket {}: {:?}", fd, error_events);
                }
            }
        }

        ServerStatus::Continue
    }

    fn handle_server_socket(&mut self, partition: &PollPartition<'_>) {
        match partition.server.check() {
            Ok(checked_pollfd) => {
                // No need to check for POLLHUP as it should never occur for a
                // listen socket.

                checked_pollfd.handle_event(libc::POLLIN, &mut |fd| {
                    sys::call_until_would_block(|| sys::accept(fd), &mut |new_command_fd| {
                        info!(
                            "Accepted new command socket connection from PID {}",
                            sys::get_socket_creds(new_command_fd).unwrap().pid
                        );

                        sys::fcntl_setfl(new_command_fd, libc::O_NONBLOCK).unwrap();

                        self.command_sockets.push(new_command_fd);
                        self.registry.register(new_command_fd, file_descriptors::Action::Close);

                        // Continue reading
                        LoopStatus::<()>::Continue
                    })
                    .unwrap();
                });

                // We don't need to check to see if an event was handled as not
                // receiving any new connections is valid.
            }
            Err((_, error_events)) => {
                // POLLERR and POLLNVAL should never occur for the server socket.
                panic!("Received polling error for server socket: {:?}", error_events);
            }
        }
    }

    fn handle_signalfd(&mut self, partition: &PollPartition<'_>) -> ServerStatus {
        match partition.signal.check() {
            Ok(checked_pollfd) => {
                checked_pollfd
                    .handle_event(libc::POLLIN, &mut |fd| {
                        sys::call_until_would_block(
                            || sys::read_exact::<libc::signalfd_siginfo>(fd),
                            &mut |siginfo: libc::signalfd_siginfo| {
                                match siginfo.ssi_signo as i32 {
                                    libc::SIGCHLD => {
                                        info!(
                                            "Received SIGCHLD from PID {} with status {}",
                                            siginfo.ssi_pid, siginfo.ssi_status
                                        );

                                        // Continue reading
                                        LoopStatus::Continue
                                    }
                                    libc::SIGINT => {
                                        info!("Received SIGINT FROM PID {}", siginfo.ssi_pid);

                                        // Terminate early
                                        LoopStatus::Break(libc::SIGINT)
                                    }
                                    libc::SIGTERM => {
                                        info!("Received SIGTERM FROM PID {}", siginfo.ssi_pid);

                                        // Terminate early
                                        LoopStatus::Break(libc::SIGTERM)
                                    }
                                    signo => {
                                        // This should never happen as only SIGCHLD,
                                        // SIGINT, and SIGTERM are added to the signalfd's
                                        // mask.
                                        panic!("Unhandled signal received: {}", signo);
                                    }
                                }
                            },
                        )
                        .unwrap()
                    })
                    // The server should exit early iff the handler exited
                    // early due to receiving a SIGINT or SIGTERM.
                    .map_or(ServerStatus::Continue, |loop_status| match loop_status {
                        LoopExit::Early(_) => ServerStatus::Exit,
                        LoopExit::WouldBlock => ServerStatus::Continue,
                    })
            }
            Err((_, error_events)) => {
                // POLLERR and POLLNVAL should never occur for a signalfd.
                panic!("Received polling error for signalfd: {:?}", error_events);
            }
        }
    }

    /// Execute the main server loop until the server receives a SIGTERM or
    /// [`crate::messages::Exit`] message.
    pub fn serve(&mut self) {
        loop {
            let mut poll_array = PollBuffer::from(&mut *self);

            // Discard the number of ready file descriptors for now.
            sys::poll(&mut poll_array, -1).unwrap();

            if self.handle_poll_event(poll_array.partition()) == ServerStatus::Exit {
                print!("Received shutdown command/signal");
                return;
            }
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        sys::close(self.signal_fd).unwrap();
        sys::close(self.server_socket).unwrap();

        for command_socket in self.command_sockets.drain(0..) {
            sys::close(command_socket).unwrap();
        }

        if let Some(path) = &self.server_socket_path {
            std::fs::remove_file(path).unwrap();
        }
    }
}
