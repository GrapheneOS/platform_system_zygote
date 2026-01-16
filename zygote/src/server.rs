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

use std::{convert::Infallible, ffi::OsStr, os::fd::RawFd, path::Path};

use arrayvec::ArrayVec;
use libloading::os::unix::{Library, RTLD_GLOBAL, RTLD_NOW};
use thiserror::Error;
use tracing::{error, info, warn};

use crate::{
    assert_ok, child_process, config, debug_assert_ok,
    file_descriptors::{self, FileDescriptorRegistry, ForkType},
    species::SpeciesRef,
};
use zygote_messages::{
    self as messages, FromParcel, Message, MessageBuffer, SpawnParamsCommon, ToParcel,
    MESSAGE_BUFFER_SIZE,
};
use zygote_sys::procfs::{self, debug_assert_single_threaded, get_proc_fd_path, ProcStat};
use zygote_sys::{
    self as sys,
    LoopControl::{self, *},
    LoopExit, PollFd,
};

const BUFFER_SIZE_CLIENT_SOCKETS: usize = 16;
const BUFFER_SIZE_POLL: usize = 64;
const SERVER_SOCKET_BACKLOG: core::ffi::c_int = 10;

type PollBuffer = ArrayVec<PollFd, BUFFER_SIZE_POLL>;

impl std::convert::From<&Server> for PollBuffer {
    fn from(server: &Server) -> PollBuffer {
        let mut poll_buffer = PollBuffer::new();

        poll_buffer.push(PollFd::new(server.signal_fd, libc::POLLIN));
        poll_buffer.push(PollFd::new(server.server_socket, libc::POLLIN));

        for client_socket in &server.client_sockets {
            poll_buffer.push(PollFd::new(*client_socket, libc::POLLIN));
        }

        poll_buffer
    }
}

#[derive(Debug)]
struct PollPartition<'a> {
    pub signal: &'a PollFd,
    pub server: &'a PollFd,
    pub clients: &'a [PollFd],
}

trait Partition<'a> {
    fn partition(&'a self) -> PollPartition<'a>;
}

impl<'a> Partition<'a> for PollBuffer {
    fn partition(&'a self) -> PollPartition<'a> {
        PollPartition::<'a> {
            signal: &self[0],
            server: &self[1],
            clients: if self.len() > 2 { &self[2..] } else { &[] },
        }
    }
}

#[derive(Debug, Error)]
enum ServerSocketError {
    #[error("Failed to create server socket: {0}")]
    CreationError(#[from] sys::Error),

    #[error("Failed to create server socket directory: {0}")]
    DirectoryCreationError(std::io::Error),

    #[error("Failed to fstat provided file descriptor: {0}")]
    FstatFailure(sys::Error),

    #[error("Socket argument path already exists: {0}")]
    PathExists(String),

    #[error("Socket path must have a parent directory")]
    PathHasNoParent,

    #[error("Provided integer argument does not refer to an open file: {0}")]
    ProvidedFdNotOpenFile(RawFd),

    #[error("Provided file descriptor does not refer to a seqpacket socket: {0}")]
    ProvidedFdNotSeqPacket(RawFd),

    #[error("Provided file descriptor does not refer to a valid socket: {0}")]
    ProvidedFdNotSocket(RawFd),
}

#[derive(Debug)]
enum ServerControl<T> {
    Continue,
    Shutdown,
    Trampoline(T),
}

impl<T> ServerControl<T> {
    #[allow(dead_code)]
    pub fn is_continue(&self) -> bool {
        matches!(self, ServerControl::Continue)
    }

    pub fn is_exit(&self) -> bool {
        matches!(self, ServerControl::Shutdown)
    }

    #[allow(dead_code)]
    pub fn is_trampoline(&self) -> bool {
        matches!(self, ServerControl::Trampoline(_))
    }
}

#[derive(Debug, Error)]
enum ClientLoopError {
    #[error("Failed to send IdentityQuery response: {0}")]
    IdentityQueryResponseFailure(sys::Error),

    #[error("Failed to create new process: {0}")]
    ProcessCreationFailure(sys::Error),

    #[error("Failed to send Spawn response: {0}")]
    SpawnResponseFailure(sys::Error),

    #[error("Failed to read procfs stats file: {0}")]
    StatReadError(procfs::ProcFsError),

    #[error("Failed to send Stat response: {0}")]
    StatResponseFailure(sys::Error),
}

enum ClientLoopControl<T> {
    Child(T),
    Break,
    NextSocket,
    Error(RawFd, ClientLoopError),
    Shutdown,
}

/// The main data structure for the Zygote process server.
#[derive(Debug)]
pub struct Server {
    name: String,
    species: SpeciesRef,
    pid: libc::pid_t,

    registry: FileDescriptorRegistry,

    signal_fd: RawFd,
    server_socket: RawFd,
    client_sockets: ArrayVec<RawFd, BUFFER_SIZE_CLIENT_SOCKETS>,

    server_socket_path: Option<String>,

    preload_uid: Option<libc::uid_t>,
    preload_gid: Option<libc::gid_t>,

    spawn_params: SpawnParamsCommon,
}

impl Server {
    /// Create a new Zygote process server from a [`crate::config::Config`]
    /// reference.
    ///
    /// Add a destructor to clean up the socket if we create it.
    #[tracing::instrument(level = "trace", skip_all)]
    pub fn new(config: &config::Server) -> Self {
        let mut registry = FileDescriptorRegistry::new(config.species)
            .expect("Failed to create file descriptor registry");

        let socket_path_or_fd = config.socket();
        let (server_socket, server_socket_path) =
            Self::get_server_socket(socket_path_or_fd).expect("Failed to create server socket");
        registry
            .register(server_socket, file_descriptors::Action::Close)
            .expect("Failed to register server socket");

        let sigset =
            sys::build_sigset(Self::blocked_signals()).expect("Failed to build signal set");
        // These masks are unblocked in Server::drop.
        sys::sigprocmask(libc::SIG_BLOCK, &sigset).expect("Failed to set signal mask");
        let signal_fd =
            sys::signalfd(-1, &sigset, libc::SFD_NONBLOCK).expect("Failed to create signalfd");
        // The signal fd is reused after forking the subspecies process.
        // Per `man signalfd`:
        //     "After a fork(2), the child inherits a copy of the signalfd file
        //     descriptor.  A read(2) from the file descriptor in the child will
        //     return information about signals queued to the child."
        registry
            .register(signal_fd, file_descriptors::Action::CloseUnlessSpawnSubspecies)
            .expect("Failed to register signalfd");

        // Register any unregistered file descriptors such as those used for logging.
        registry.register_new().expect("Failed to register new file descriptors");
        registry.audit().expect("File descriptor audit failed");

        let server = Self {
            name: config.name.clone(),
            species: config.species,
            pid: sys::getpid(),

            registry,

            signal_fd,
            server_socket,
            client_sockets: ArrayVec::new(),

            server_socket_path,

            preload_uid: config.preload_uid,
            preload_gid: config.preload_gid,

            spawn_params: config.to_spawn_params(),
        };

        sys::prctl_set_name(&server.name);

        // Create a new process group for this Zygote server process and its
        // children.  The following list contains the possible error codes
        // returned and why they are not applicable to this call site:
        //  * EACCESS: Not possible; calling on self
        //  * EINVAL: Both arguments are literals >= 0
        //  * EPERM: Not applicable; we're creating a new process group, not
        //           moving between existing ones
        //  * EPERM: Not applicable; calling on self
        if let Err(errno) = sys::setpgid(0, 0) {
            error!("Failed to create process group for Zygote server: {errno}");
            std::process::exit(1);
        }

        server.preload(&config.preload_libraries);

        #[cfg(target_os = "android")]
        if let Err(errno) = sys::mallopt(libc::M_PURGE_ALL, 0) {
            error!("Failed to mallopt(M_PURGE_ALL): {errno}");
        }

        server
    }

    /// Tailor the Server instance for the subspecies.
    #[tracing::instrument(level = "trace", skip(self))]
    fn re_initialize_as_subspecies(&mut self, child_socket_path: String) {
        self.registry.reset_for_subspecies().expect("Failed to reset file descriptor registry");

        let (child_socket_fd, child_socket_path) = Self::get_server_socket(child_socket_path)
            .expect("Failed to create server socket for subspecies");
        self.registry
            .register(child_socket_fd, file_descriptors::Action::Close)
            .expect("Failed to register server socket for subspecies");

        // All the RawFds of client sockets are closed in the
        // `reset_for_subspecies()` call above and they are stateless, thus
        // clear the client_sockets.
        self.client_sockets.clear();
        // The server RawFd is also already closed in
        // `reset_for_subspecies()`, so replace with a new one.
        self.server_socket = child_socket_fd;
        self.server_socket_path = child_socket_path;

        self.pid = sys::getpid();
    }

    const fn blocked_signals() -> &'static [libc::c_int] {
        &[libc::SIGCHLD, libc::SIGINT, libc::SIGTERM]
    }

    /// Produce a socket file descriptor by one of the following methods:
    ///   * Using the provided integer as a file descriptor
    ///   * Opening a new socket and binding it to the provided path
    ///   * Opening a new socket and binding it to a default path
    fn get_server_socket(
        socket_path_or_fd: String,
    ) -> Result<(RawFd, Option<String>), ServerSocketError> {
        if let Ok(fd) = socket_path_or_fd.parse::<RawFd>() {
            if !get_proc_fd_path(fd).exists() {
                return Err(ServerSocketError::ProvidedFdNotOpenFile(fd));
            }

            let stat = sys::fstat(fd).map_err(ServerSocketError::FstatFailure)?;
            if sys::get_file_type(stat) != libc::S_IFSOCK {
                return Err(ServerSocketError::ProvidedFdNotSocket(fd));
            }

            if sys::get_socket_type(fd)? != libc::SOCK_SEQPACKET {
                return Err(ServerSocketError::ProvidedFdNotSeqPacket(fd));
            }

            sys::fcntl_setfl(fd, libc::O_NONBLOCK)?;
            sys::listen(fd, SERVER_SOCKET_BACKLOG)?;

            Ok((fd, None))
        } else {
            let arg_path = Path::new(&socket_path_or_fd);

            let (socket_fd, server_socket_path) =
                if let Some(abs_socket_addr) = socket_path_or_fd.strip_prefix("@") {
                    (sys::create_abstract_socket(abs_socket_addr, libc::SOCK_SEQPACKET)?, None)
                } else {
                    if arg_path.exists() {
                        return Err(ServerSocketError::PathExists(socket_path_or_fd.clone()));
                    }
                    std::fs::create_dir_all(
                        arg_path.parent().ok_or(ServerSocketError::PathHasNoParent)?,
                    )
                    .map_err(ServerSocketError::DirectoryCreationError)?;
                    (
                        sys::create_bound_socket(&socket_path_or_fd, libc::SOCK_SEQPACKET)?,
                        Some(socket_path_or_fd.clone()),
                    )
                };
            sys::fcntl_setfl(socket_fd, libc::O_NONBLOCK)?;
            sys::listen(socket_fd, SERVER_SOCKET_BACKLOG)?;

            Ok((socket_fd, server_socket_path))
        }
    }

    /// Check each of the polled file descriptors to check if we should read
    /// from them.
    ///
    /// The function's return value indicates if the server should terminate
    /// after this call.
    #[tracing::instrument(level = "trace", skip_all)]
    fn check_poll_events(
        &mut self,
        partition: PollPartition<'_>,
    ) -> ServerControl<impl FnOnce() -> Infallible + use<>> {
        if self.check_signalfd_events(&partition).is_exit() {
            return ServerControl::Shutdown;
        }

        self.check_server_socket_events(&partition);
        self.check_client_sockets_events(&partition)
    }

    #[tracing::instrument(level = "trace", skip_all)]
    fn check_client_sockets_events(
        &mut self,
        partition: &PollPartition<'_>,
    ) -> ServerControl<impl FnOnce() -> Infallible + use<>> {
        for client_pollfd in partition.clients {
            // POLLERR and POLLNVAL should never occur for a client socket.
            let checked_pollfd =
                client_pollfd.check().unwrap_or_else(|err| panic!("Client socket: {err}"));

            let pollin_result = checked_pollfd.handle_event(libc::POLLIN, &mut |fd| {
                sys::call_until_would_block(
                    || sys::recvmsg::<MESSAGE_BUFFER_SIZE>(fd),
                    &mut |(readlen, message_buffer): (isize, MessageBuffer)| {
                        if readlen == 0 {
                            return Break(ClientLoopControl::NextSocket);
                        }

                        match Message::try_from_parcel(&message_buffer) {
                            Ok(_) => self.dispatch_message_handler(fd, message_buffer),
                            Err(err) => {
                                // TODO: Respond with an error
                                warn!(
                                    "Invalid message received from client (PID {}): {}",
                                    sys::get_socket_creds(fd)
                                        .expect("Failed to get socket credentials")
                                        .pid,
                                    err
                                );
                                Break(ClientLoopControl::NextSocket)
                            }
                        }
                    },
                )
                // TODO: Add more extensive error handling.
                .expect("Unexpected error in recvmsg loop")
            });

            match pollin_result {
                None => {
                    // No event was registered for this file descriptor
                }
                Some(LoopExit::WouldBlock) => {
                    // All available messages were read from the socket
                }
                Some(LoopExit::Early(control)) => {
                    match control {
                        ClientLoopControl::Child(thunk) => {
                            // We are in the child process and should exit the
                            // server with the thunk.
                            return ServerControl::Trampoline(thunk);
                        }
                        ClientLoopControl::Break => {
                            // We have just reset the server instance for App
                            // Zygote thus the pollfds are invalidated. Move
                            // back to the top of the server loop.
                            return ServerControl::Continue;
                        }
                        ClientLoopControl::NextSocket => {
                            // Zero-length read from socket, continue and wait for SIGHUP
                        }
                        ClientLoopControl::Error(fd, err) => {
                            error!(
                                "Error encountered while responding to client socket {fd}: {err}"
                            );

                            self.remove_client_socket(fd);
                            continue;
                        }
                        ClientLoopControl::Shutdown => return ServerControl::Shutdown,
                    }
                }
            }

            let _ = checked_pollfd.handle_event(libc::POLLHUP, |fd| {
                info!("client socket disconnected: {fd}");
                self.remove_client_socket(fd);
            });
        }

        // Continue the server loop
        ServerControl::Continue
    }

    #[tracing::instrument(level = "trace", skip_all)]
    fn check_server_socket_events(&mut self, partition: &PollPartition<'_>) {
        // POLLERR and POLLNVAL should never occur for the server socket.
        let checked_pollfd = partition.server.check().unwrap_or_else(|err| {
            panic!("Server socket: {err}");
        });

        checked_pollfd.handle_event(libc::POLLIN, &mut |fd| {
            sys::call_until_would_block(|| sys::accept(fd), &mut |new_client_fd| {
                info!(
                    "Accepted new client socket connection from PID {}",
                    sys::get_socket_creds(new_client_fd)
                        .expect("Failed to get socket credentials")
                        .pid
                );

                sys::fcntl_setfl(new_client_fd, libc::O_NONBLOCK).expect("fcntl_setfl failed");

                self.client_sockets.push(new_client_fd);
                self.registry
                    .register(new_client_fd, file_descriptors::Action::Close)
                    .expect("Failed to register client socket");

                // Continue reading
                LoopControl::<()>::Continue
            })
            // TODO: Add more extensive error handling.
            .expect("Unexpected error in accept loop");
        });

        // We don't need to check to see if a POLLIN event was handled as not
        // receiving any new connections is valid.

        // No need to check for POLLHUP as it should never occur for a
        // listen socket.
    }

    #[tracing::instrument(level = "trace", skip_all)]
    fn check_signalfd_events(
        &mut self,
        partition: &PollPartition<'_>,
    ) -> ServerControl<impl FnOnce() -> Infallible> {
        // POLLERR and POLLNVAL should never occur for a signalfd.
        let checked_pollfd = partition.signal.check().unwrap_or_else(|err| {
            panic!("Signalfd: {err}");
        });

        checked_pollfd
            .handle_event(libc::POLLIN, &mut |fd| {
                sys::call_until_would_block(
                    || sys::read_exact::<libc::signalfd_siginfo>(fd),
                    &mut |siginfo: libc::signalfd_siginfo| {
                        match siginfo.ssi_signo as i32 {
                            libc::SIGCHLD => {
                                let pid = siginfo.ssi_pid as libc::pid_t;
                                info!(
                                    "Received SIGCHLD from PID {} with status {}",
                                    pid, siginfo.ssi_status
                                );
                                match sys::waitpid(Some(pid), libc::WNOHANG) {
                                    Ok(Some((ret_pid, status))) if ret_pid == pid => {
                                        self.species.handle_sigchld(
                                            pid,
                                            siginfo.ssi_uid,
                                            siginfo.ssi_status,
                                        );
                                        info!(
                                            "Reaped child process {} terminated with {:?}",
                                            pid, status
                                        );
                                    }
                                    _ => error!("Failed to reap child process {}", pid),
                                };

                                // Continue reading
                                Continue
                            }
                            libc::SIGINT => {
                                info!("Received SIGINT FROM PID {}", siginfo.ssi_pid);

                                // Terminate early
                                Break(libc::SIGINT)
                            }
                            libc::SIGTERM => {
                                info!("Received SIGTERM FROM PID {}", siginfo.ssi_pid);

                                // Terminate early
                                Break(libc::SIGTERM)
                            }
                            signo => {
                                // This should never happen as only SIGCHLD,
                                // SIGINT, and SIGTERM are added to the signalfd's
                                // mask.
                                panic!("Unhandled signal received: {signo}");
                            }
                        }
                    },
                )
                // TODO: Add more extensive error handling.
                .expect("Unexpected error in signalfd read loop")
            })
            // The server should exit early iff the handler exited
            // early due to receiving a SIGINT or SIGTERM.
            .map_or(
                ServerControl::<fn() -> Infallible>::Continue,
                |loop_control| match loop_control {
                    LoopExit::Early(_) => ServerControl::Shutdown,
                    LoopExit::WouldBlock => ServerControl::Continue,
                },
            )
    }

    fn dispatch_message_handler(
        &mut self,
        fd: RawFd,
        message_buffer: MessageBuffer,
    ) -> LoopControl<ClientLoopControl<impl FnOnce() -> Infallible + use<>>> {
        match Message::try_from_parcel(&message_buffer) {
            Ok(Message::Exit) => self.handle_message_exit(fd),
            Ok(Message::IdentityQuery) => self.handle_message_identity_query(fd),
            Ok(Message::Spawn { payload, .. }) | Ok(Message::SpawnSubspecies { payload, .. })
                if !self.species.is_spawn_payload_type(&payload) =>
            {
                // TODO: Respond with an error
                error!(
                    "Incorrect spawn payload for this species {}: {:?}",
                    self.species.name(),
                    payload
                );

                // Continue the `recvmsg` loop
                Continue
            }
            Ok(Message::Spawn { .. }) => self.handle_message_spawn(fd, message_buffer),
            Ok(Message::SpawnSubspecies { socket_path, .. }) => {
                self.handle_message_spawn_subspecies(fd, socket_path.to_string(), message_buffer)
            }
            Ok(Message::Stat) => self.handle_message_stat(fd),
            Ok(msg) => {
                warn!("Server received unhandled message: {msg:?}");

                // Continue the `recvmsg` loop
                Continue
            }
            Err(err) => {
                warn!(
                    "Invalid message received from client (PID {}): {}",
                    sys::get_socket_creds(fd).expect("Failed to get socket credentials").pid,
                    err
                );
                Continue
            }
        }
    }

    fn handle_message_exit<Thunk: FnOnce() -> Infallible>(
        &mut self,
        fd: RawFd,
    ) -> LoopControl<ClientLoopControl<Thunk>> {
        info!(
            "Received message: (Exit {})",
            sys::get_socket_creds(fd).expect("Failed to get socket credentials").pid
        );

        if let Err(errno) =
            Self::send_response(fd, Message::AckResponse.to_parcel().finished_data())
        {
            warn!("Failed to acknowledge Exit message: {errno}")
        }

        Break(ClientLoopControl::Shutdown)
    }

    #[tracing::instrument(level = "trace", skip_all)]
    fn handle_message_identity_query<Thunk: FnOnce() -> Infallible>(
        &self,
        fd: RawFd,
    ) -> LoopControl<ClientLoopControl<Thunk>> {
        info!(
            "Received message: (IdentityQuery {})",
            sys::get_socket_creds(fd).expect("Failed to get socket credentials").pid
        );

        let response = Message::IdentityQueryResponse {
            name: &self.name,
            species: self.species.name(),
            arch: std::env::consts::ARCH,
        };

        match Self::send_response(fd, response.to_parcel().finished_data()) {
            Ok(_) => Continue,
            Err(error) => {
                error!("Failed to send IdentityQuery response: {error}");
                Break(ClientLoopControl::Error(
                    fd,
                    ClientLoopError::IdentityQueryResponseFailure(error),
                ))
            }
        }
    }

    #[tracing::instrument(level = "trace", skip_all)]
    fn handle_spawn<Thunk: FnOnce() -> Infallible>(
        &mut self,
        fd: RawFd,
        message_buffer: MessageBuffer,
        child_continuation: impl FnOnce(&mut Self, SpawnParamsCommon) -> ClientLoopControl<Thunk>,
    ) -> LoopControl<ClientLoopControl<Thunk>> {
        // The server does not spawn any threads.  Preloaded library
        // initializers should not start any threads.  Any threads created by
        // species-specific code during initialization must be terminated when
        // control is returned to the process-server.
        debug_assert_single_threaded();
        debug_assert_ok!(self.registry.audit());

        let message = Message::try_from_parcel(&message_buffer).expect("Invalid message");
        info!("Received message: ({message:?})");

        // TODO: Implement logic to lock some or all of the common spawn
        //       parameters, preventing them from being set by a spawn message.
        let spawn_params =
            message.get_spawn_params().expect("Missing spawn params").or(&self.spawn_params);

        // let clone_args = sys::clone_args::new();

        // SAFETY: This is called in a single-threaded context.
        //
        //         The `clone3()` function can produce the following errors:
        //         EACCES, EAGAIN, EBUSY, EEXIST, EINVAL, ENOSPC, ENOMEM,
        //         EOPNOTSUPP, EPERM, ERESTARTNOINTR, EUSERS.
        //
        //         The Zygote can not recover from EAGAIN or ENOMEM.
        //         ERESTARTNOINTR will not trigger during normal operations as
        //         signals are handled via a signalfd and not asynchronous
        //         signal handlers.  Errors are logged below.
        //
        // TODO: Revert to using `clone3` after b/439747272 is resolved
        match unsafe { sys::fork() } {
            Ok(0) => {
                // Child process

                if let Some(priority) = spawn_params.priority_initial
                    && sys::setpriority(libc::PRIO_PROCESS, 0, priority).is_err()
                {
                    // EINVAL, EPERM, and ESRCH only apply when setting the
                    // priority of other processes.
                    warn!("Insufficient permissions to set priority: {priority}");
                }
                Break(child_continuation(self, spawn_params))
            }
            Ok(new_pid) => {
                // Server process
                info!("Spawned process {new_pid}");
                let response = Message::SpawnResponse { pid: new_pid };
                match Self::send_response(fd, response.to_parcel().finished_data()) {
                    Ok(_) => Continue,
                    Err(error) => {
                        error!("Failed to send Spawn response: {error}");
                        Break(ClientLoopControl::Error(
                            fd,
                            ClientLoopError::SpawnResponseFailure(error),
                        ))
                    }
                }
            }
            Err(sys::Error::Libc(errno)) => {
                // Server process

                if errno.is(libc::EACCES) {
                    // TODO: Print the actual cgroup path once it is present in the spawn params.
                    error!("Version 2 cgroup membership criteria are not met: <TODO>");
                } else if errno.is(libc::EAGAIN) {
                    error!("System has too many running processes");
                } else if errno.is(libc::EBUSY) {
                    // TODO: Print the actual cgroup path once it is present in the spawn params.
                    error!("Version 2 cgroup contains an enabled domain controller: <TODO>");
                } else if errno.is(libc::EEXIST) {
                    error!("`set_tid` value already exists in the current namespace");
                // } else if errno.is(libc::EINVAL) {
                //     error!(
                //         "Invalid argument combination to `clone3()` (see man page for details): {clone_args:?}"
                //     );
                } else if errno.is(libc::ENOMEM) {
                    error!("Cannot allocate sufficient memory for a new process");
                // } else if errno.is(libc::ENOSPC) {
                //     error!(
                //         "Either CLONE_NEWPID or CLONE_NEWUSER were specified and the resulting number of nested namespaces would exceed the maximum allowed depth: {clone_args:?}");
                } else if errno.is(libc::EOPNOTSUPP) {
                    // TODO: Print the actual cgroup path once it is present in the spawn params.
                    error!(
                        "Destination version 2 cgroup is currently in a domain invalid state: <TODO>"
                    );
                // } else if errno.is(libc::EPERM) {
                //     error!("Server lacks the correct permissions to clone with the provided arguments: {clone_args:?}");
                } else {
                    error!("Unexpected error code returned by call to `clone3()`: {errno}");
                }

                Break(ClientLoopControl::Error(
                    fd,
                    ClientLoopError::ProcessCreationFailure(errno.into()),
                ))
            }
            Err(_) => {
                unreachable!("The fork and clone3 calls can only return sys::Error::Libc variants")
            }
        }
    }

    #[allow(unreachable_code)]
    #[tracing::instrument(level = "trace", skip_all)]
    fn handle_message_spawn(
        &mut self,
        fd: RawFd,
        message_buffer: MessageBuffer,
    ) -> LoopControl<ClientLoopControl<impl FnOnce() -> Infallible + use<>>> {
        // Creating local copies avoids capturing additional references.
        let species: SpeciesRef = self.species;
        let re_init_data = species.gather_reinitialization_data();

        self.handle_spawn(fd, message_buffer, move |_, spawn_params| {
            // SAFETY: The contents of this message were received from a bound
            //         UNIX Domain socket.  Processes with permission to read
            //         and write to this socket are considered authorized to
            //         spawn processes from this server.
            //
            //         The message data will only be read in the child process
            //         once the server and configuration structs are dropped.
            //         This ensures that all file descriptor registry actions
            //         are taken before control is passed to the species code.
            let spawn_message = unsafe { messages::SpawnMessage::new(message_buffer) };

            ClientLoopControl::Child(move || {
                debug_assert_single_threaded();

                // This function call must occur here, at the top of the child
                // process's stack, to avoid segfaults from changing the stack
                // guard in a callee and then segfaulting when return to the
                // caller's frame.
                child_process::maybe_reset_stack_guards(move || {
                    // Unpack the message in the child process
                    let message =
                        Message::try_from_parcel(spawn_message.as_ref()).expect("Invalid message");
                    let spawn_payload = message.get_spawn_payload().expect("Missing spawn payload");

                    child_process::re_initialize(
                        species,
                        re_init_data,
                        &spawn_params,
                        spawn_payload,
                    );

                    species.gestate(&spawn_params, spawn_payload)
                })
            })
        })
    }

    #[tracing::instrument(level = "trace", skip_all)]
    fn handle_message_spawn_subspecies<Thunk: FnOnce() -> Infallible>(
        &mut self,
        fd: RawFd,
        socket_path: String,
        message_buffer: MessageBuffer,
    ) -> LoopControl<ClientLoopControl<Thunk>> {
        let re_init_data = self.species.gather_reinitialization_data();
        // Creating local copies avoids capturing additional references.
        let species: SpeciesRef = self.species;

        self.handle_spawn(fd, message_buffer, move |server, spawn_params| {
            let message = Message::try_from_parcel(&message_buffer).expect("Invalid message");
            let spawn_payload = message.get_spawn_payload().expect("Missing spawn payload");
            child_process::re_initialize(species, re_init_data, &spawn_params, spawn_payload);
            server.re_initialize_as_subspecies(socket_path);

            species.speciate(spawn_payload);
            ClientLoopControl::Break
        })
    }

    #[tracing::instrument(level = "trace", skip_all)]
    fn handle_message_stat<Thunk: FnOnce() -> Infallible>(
        &mut self,
        fd: RawFd,
    ) -> LoopControl<ClientLoopControl<Thunk>> {
        info!("Received message: (Stat)");
        let proc = match ProcStat::get() {
            Ok(proc) => proc,
            Err(err) => {
                error!("Failed to get ProcStat: {err:?}");
                return Break(ClientLoopControl::Error(fd, ClientLoopError::StatReadError(err)));
            }
        };
        let response = Message::StatResponse {
            pid: proc.pid,
            pgrp: proc.pgrp,
            minflt: proc.minflt,
            cminflt: proc.cminflt,
            majflt: proc.majflt,
            cmajflt: proc.cmajflt,
            utime: proc.utime,
            stime: proc.stime,
            num_threads: proc.num_threads,
            vsize: proc.vsize,
            rss: proc.rss,
        };

        match Self::send_response(fd, response.to_parcel().finished_data()) {
            Ok(_) => {
                // Continue the `recvmsg` loop
                Continue
            }
            Err(error) => {
                error!("Failed to send Stat response: {error}");
                Break(ClientLoopControl::Error(fd, ClientLoopError::StatResponseFailure(error)))
            }
        }
    }

    #[tracing::instrument(level = "trace", skip_all)]
    fn preload<T>(&self, libraries: &Vec<T>)
    where
        T: AsRef<OsStr> + std::fmt::Debug + tracing::Value,
    {
        let _eid_context = sys::EffectiveIdContext::enter(self.preload_uid, self.preload_gid)
            .unwrap_or_else(|errno| {
                error!(
                    "Failed to set effective UID/GID ({:?}/{:?}): {}",
                    self.preload_uid, self.preload_gid, errno
                );
                std::process::exit(1);
            });

        for library_path in libraries {
            span_scope!("dlopen", path = library_path);

            // SAFETY: The Zygote process server is designed to load and
            //         execute code from shared libraries.  If a Zygote process
            //         is launched with permissions to access these files and
            //         the user has specified them in the preload set then the
            //         user acknowledges that the object will be loaded, static
            //         initializers will be called, and the contained functions
            //         may be called.
            let dlopen_result =
                unsafe { Library::open(Some(library_path), RTLD_GLOBAL | RTLD_NOW) };

            match dlopen_result {
                Ok(library) => {
                    info!("Preloaded library SUCCESS: {:?}", library_path.as_ref());

                    // Extract to contained handle to prevent the library from
                    // being closed as we exit the current scope.
                    let _ = library.into_raw();
                }
                Err(err) => {
                    error!("Preload library FAILURE: {:?} : {}", library_path.as_ref(), err);
                }
            };
        }

        // TODO: Add a callback to the species to allow it to preload libraries
    }

    fn remove_client_socket(&mut self, fd: RawFd) {
        self.registry.remove(fd).expect("Client socket is not registered");
        self.client_sockets.remove(
            self.client_sockets
                .iter()
                .position(|&search_fd| search_fd == fd)
                .expect("fd not in client_sockets"),
        );
        // Silently ignore EBADF and EIO.
        let _ = sys::close(fd);
    }

    /// Send the provided response through the socket and panic on errors that
    /// indicate an irrecoverable bug.
    ///
    /// The following errors indicate a critical logic error.  In these cases
    /// the server will panic to prevent any unintended behavior:
    /// * [`libc::EACCES`]
    /// * [`libc::EALREADY`]
    /// * [`libc::EBADF`]
    /// * [`libc::EDESTADDRREQ`]
    /// * [`libc::EFAULT`]
    /// * [`libc::EINVAL`]
    /// * [`libc::EISCONN`]
    /// * [`libc::EMSGSIZE`]
    /// * [`libc::ENOTCONN`]
    /// * [`libc::ENOTSOCK`]
    /// * [`libc::EOPNOTSUPP`]
    /// * [`libc::EPIPE`]
    ///
    /// The following errors indicate the system is in a very unhealthy state.
    /// The server will panic and allow the system to recover:
    /// * [`libc::ENOBUFS`]
    /// * [`libc::ENOMEM`]
    ///
    /// The following errors will be returned to the caller:
    /// * [`libc::EAGAIN`]
    /// * [`libc::EWOULDBLOCK`]
    /// * [`libc::ECONNRESET`]
    fn send_response(fd: RawFd, buffer: &[u8]) -> Result<(), sys::Error> {
        match sys::sendmsg(fd, buffer) {
            Ok(_) => Ok(()),
            Err(sys::Error::Libc(errno))
                if errno.matches(&[libc::EAGAIN | libc::EWOULDBLOCK | libc::ECONNRESET]) =>
            {
                Err(errno.into())
            }
            Err(sys::Error::Libc(errno)) if errno.matches(&[libc::ENOBUFS | libc::ENOMEM]) => {
                panic!("System is in a critical low-memory state: {errno}. Exiting.")
            }
            Err(errno) => panic!("Unexpected error when sending response on fd {fd}: {errno}"),
        }
    }

    /// Execute the main server loop until the server receives a SIGTERM or
    /// [`crate::messages::Exit`] message.
    ///
    /// The child process thunk is returned to allow the caller to drop the
    /// server before control flow is transferred to the species-specific
    /// code.  This allows resources to be cleaned up and possibly sensitive
    /// data to be deallocated.
    pub fn serve(&mut self) -> Option<impl FnOnce() -> Infallible + use<>> {
        self.species.on_server_ready();

        loop {
            let mut poll_array = PollBuffer::from(&*self);

            // Discard the number of ready file descriptors for now.
            sys::poll(&mut poll_array, -1).expect("poll failed");

            match self.check_poll_events(poll_array.partition()) {
                ServerControl::Continue => {}
                ServerControl::Shutdown => {
                    return None;
                }
                ServerControl::Trampoline(thunk) => {
                    return Some(thunk);
                }
            }
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if self.pid == sys::getpid() {
            self.species.on_server_destroy();

            // Clean up the server code in the server process
            if let Err(error) = self.registry.override_and_close() {
                error!("Failed to close file descriptors during server shutdown: {}", error);
            }

            if let Some(path) = &self.server_socket_path
                && let Err(error) = std::fs::remove_file(path)
            {
                error!("Failed to remove server socket file: {}", error);
            }
        } else {
            // Clean up the server code in the child process
            self.registry
                .execute_actions(ForkType::Application)
                .expect("Failed to file descriptor registry actions");
        }

        let sigset = sys::build_sigset(Self::blocked_signals()).unwrap();
        sys::sigprocmask(libc::SIG_UNBLOCK, &sigset).expect("Failed to unset signal mask");
    }
}
