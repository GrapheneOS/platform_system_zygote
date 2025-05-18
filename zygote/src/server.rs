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
    assert_ok, config, debug_assert_ok,
    file_descriptors::{self, FileDescriptorRegistry},
    introspection::{debug_assert_single_threaded, get_proc_fd_path},
    messages::{Command, Message},
    species::SpeciesRef,
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
/// Statically allocated arrays used for receiving messages.
pub type MessageBuffer = [u8; MESSAGE_BUFFER_SIZE];

#[derive(Debug)]
enum ServerStatus<T> {
    Continue,
    Exit,
    Trampoline(T),
}

impl<T> ServerStatus<T> {
    #[allow(dead_code)]
    pub fn is_continue(&self) -> bool {
        matches!(self, ServerStatus::Continue)
    }

    pub fn is_exit(&self) -> bool {
        matches!(self, ServerStatus::Exit)
    }

    #[allow(dead_code)]
    pub fn is_trampoline(&self) -> bool {
        matches!(self, ServerStatus::Trampoline(_))
    }
}

/// The main data structure for the Zygote process server.
pub struct Server {
    registry: FileDescriptorRegistry,
    species: SpeciesRef,
    pid: libc::pid_t,

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
            species: config.species,
            pid: sys::getpid(),

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
    fn check_poll_events(
        &mut self,
        partition: PollPartition<'_>,
    ) -> ServerStatus<impl FnOnce() + use<>> {
        if self.check_signalfd_events(&partition).is_exit() {
            return ServerStatus::Exit;
        }

        self.check_server_socket_events(&partition);
        self.check_command_sockets_events(&partition)
    }

    fn check_command_sockets_events(
        &mut self,
        partition: &PollPartition<'_>,
    ) -> ServerStatus<impl FnOnce() + use<>> {
        for command_pollfd in partition.command {
            // POLLERR and POLLNVAL should never occur for a command socket.
            let checked_pollfd = command_pollfd.check().unwrap_or_else(|(fd, error_events)| {
                panic!("Received polling error for command socket {}: {:?}", fd, error_events)
            });

            let pollin_result = checked_pollfd.handle_event(libc::POLLIN, &mut |fd| {
                sys::call_until_would_block(
                    || sys::recvmsg::<MESSAGE_BUFFER_SIZE>(fd),
                    &mut |(readlen, message_buffer): (isize, MessageBuffer)| {
                        if readlen == 0 {
                            return LoopStatus::Break(LoopStatus::Continue);
                        }

                        let message = flatbuffers::root::<Message>(&message_buffer).unwrap();

                        match message.command_type() {
                            Command::Exit => {
                                self.handle_command_exit(fd);

                                // Break out of the `recvmsg` loop
                                LoopStatus::Break(
                                    // Break out of the PollFd loop
                                    LoopStatus::Break(
                                        // Exit the server
                                        ServerStatus::Exit,
                                    ),
                                )
                            }
                            Command::Stat => {
                                self.handle_command_stat();

                                // Continue the `recvmsg` loop
                                LoopStatus::Continue
                            }
                            cmd if cmd == self.species.command_type_spawn() => {
                                if let Some(thunk) = self.handle_command_spawn(message_buffer) {
                                    // Child process

                                    // Break out of the `recvmsg` loop
                                    LoopStatus::Break(
                                        // Break out of the PollFd loop
                                        LoopStatus::Break(
                                            // Launch the new application
                                            ServerStatus::Trampoline(thunk),
                                        ),
                                    )
                                } else {
                                    // Server process

                                    // Continue the `recvmsg` loop
                                    LoopStatus::Continue
                                }
                            }
                            cmd @ Command(tag) if tag < Command::ENUM_MAX => {
                                error!(
                                    "Command not supported by this species: {}",
                                    cmd.variant_name().unwrap()
                                );

                                // Continue the `recvmsg` loop
                                LoopStatus::Continue
                            }
                            Command(tag) => {
                                error!("Invalid command variant encountered: {}", tag);

                                // Continue the `recvmsg` loop
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
                Some(LoopExit::Early(LoopStatus::Break(server_status))) => {
                    // Either messages were read until we received a
                    // shutdown command or we received a spawn command
                    // and are in the child process.
                    return server_status;
                }
            }

            let _ = checked_pollfd.handle_event(libc::POLLHUP, |fd| {
                self.registry.remove(fd).unwrap();
                self.command_sockets.remove(
                    self.command_sockets.iter().position(|&search_fd| search_fd == fd).unwrap(),
                );
                sys::close(fd).unwrap();
            });
        }

        // Continue the server loop
        ServerStatus::Continue
    }

    fn check_server_socket_events(&mut self, partition: &PollPartition<'_>) {
        // POLLERR and POLLNVAL should never occur for the server socket.
        let checked_pollfd = partition.server.check().unwrap_or_else(|(_, error_events)| {
            panic!("Received polling error for server socket: {:?}", error_events);
        });

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

        // We don't need to check to see if a POLLIN event was handled as not
        // receiving any new connections is valid.

        // No need to check for POLLHUP as it should never occur for a
        // listen socket.
    }

    fn check_signalfd_events(&mut self, partition: &PollPartition<'_>) -> ServerStatus<fn()> {
        // POLLERR and POLLNVAL should never occur for a signalfd.
        let checked_pollfd = partition.signal.check().unwrap_or_else(|(_, error_events)| {
            panic!("Received polling error for signalfd: {:?}", error_events);
        });

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

    fn handle_command_exit(&mut self, fd: RawFd) {
        info!("Received command: (Exit {})", sys::get_socket_creds(fd).unwrap().pid);
    }

    fn handle_command_spawn(
        &mut self,
        message_buffer: MessageBuffer,
    ) -> Option<impl FnOnce() + use<>> {
        let message = flatbuffers::root::<Message>(&message_buffer).unwrap();
        info!("Received command: ({:?})", message.command_type());

        debug_assert_ok!(self.registry.audit());

        debug_assert_single_threaded();
        // SAFETY: The server never spawns any threads directly and none of the
        //         used libraries should spawn threads either.  The above debug
        //         assertion is used to verify this property.
        //
        //         The `fork()` function can produce the following errors:
        //         EAGAIN, ENOMEM, ENOSYS, ERESTARTNOINTR.
        //
        //         The Zygote can not recover from EAGAIN or ENOMEM.  ENOSYS
        //         will not trigger as the Zygote is designed for systems that
        //         provide `fork()`.  ERESTARTNOINTR will not trigger during
        //         normal operations as signals are handled via a signalfd and
        //         not asynchronous signal handlers.
        let new_pid = unsafe { sys::fork() }.unwrap();

        if new_pid == 0 {
            // Child process

            // Creating this local variable avoids capturing a reference to
            // self.
            let species: SpeciesRef = self.species;
            Some(move || {
                species.gestate(message_buffer);
            })
        } else {
            // Server process
            info!("Spawned process {}", new_pid);

            None
        }
    }

    fn handle_command_stat(&mut self) {
        info!("Received command: (Stat)");
    }

    /// Execute the main server loop until the server receives a SIGTERM or
    /// [`crate::messages::Exit`] message.
    pub fn serve(&mut self) -> Option<impl FnOnce()> {
        loop {
            let mut poll_array = PollBuffer::from(&mut *self);

            // Discard the number of ready file descriptors for now.
            sys::poll(&mut poll_array, -1).unwrap();

            match self.check_poll_events(poll_array.partition()) {
                ServerStatus::Continue => {}
                ServerStatus::Exit => {
                    return None;
                }
                ServerStatus::Trampoline(thunk) => {
                    return Some(thunk);
                }
            }
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if self.pid == sys::getpid() {
            // Clean up the server code in the server process
            self.registry.override_and_close();

            if let Some(path) = &self.server_socket_path {
                std::fs::remove_file(path).unwrap();
            }
        } else {
            // Clean up the server code in the child process
            self.registry.execute_actions();
        }
    }
}
