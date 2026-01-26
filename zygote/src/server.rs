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

#[allow(unused_imports)]
use crate::debug_assert_ok;
use crate::{
    child_process, config,
    file_descriptors::{self, FdRegistryError, FileDescriptorRegistry, ForkType},
    species::SpeciesRef,
};
use zygote_messages::{
    self as messages, FromParcel, Message, MessageBuffer, SpawnParamsCommon, ToParcel,
    MESSAGE_BUFFER_SIZE,
};
use zygote_sys::{
    self as sys,
    procfs::{self, debug_assert_single_threaded, get_proc_fd_path, ProcStat},
    EpollEvent, EpollHandle,
    LoopControl::{self, *},
    LoopExit, TaskFailure,
};

const BUFFER_SIZE_CLIENT_SOCKETS: usize = 16;
const SERVER_SOCKET_BACKLOG: core::ffi::c_int = 10;

#[repr(i8)]
#[derive(Debug, PartialEq, Eq)]
enum EpollTag {
    Client = 0,
    Server = 1,
    Signal = 2,
}

#[derive(Debug)]
struct EpollData {
    fd: RawFd,
    class: EpollTag,
}

/// Wrapper type for errors
#[derive(Debug)]
#[repr(transparent)]
pub struct OpaqueEpollData(EpollData);

impl From<EpollData> for OpaqueEpollData {
    fn from(value: EpollData) -> Self {
        OpaqueEpollData(value)
    }
}

impl EpollData {
    fn new(fd: RawFd, class: EpollTag) -> Self {
        EpollData { fd, class }
    }
}

static_assertions::const_assert_eq!(size_of::<RawFd>(), size_of::<u32>());

impl From<EpollData> for u64 {
    fn from(value: EpollData) -> Self {
        // The extra cast to `u32` is to ensure that the cast to `u64` causes
        // the value to be zero-extended.
        value.fd as u32 as u64 | (value.class as u64) << 32
    }
}

impl TryFrom<u64> for EpollData {
    type Error = ServerError;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        Ok(EpollData {
            fd: value as RawFd,
            // SAFETY: This value was written by the `From<EPollData>` implementation
            class: match (value >> 32) as i8 {
                0 => EpollTag::Client,
                1 => EpollTag::Server,
                2 => EpollTag::Signal,
                _ => return Err(ServerError::InvalidEpollData(value)),
            },
        })
    }
}

/// Errors that may be encountered when performing signal-handling operations
#[derive(Debug, Error)]
pub enum SignalHandlerError {
    /// Failed to poll the signalfd
    #[error("Failed to poll the signalfd: {0}")]
    Poll(sys::Error),

    /// Failed to account for a terminated child process
    #[error("Failed to account for a terminated child process: {0}")]
    ProcessAccounting(sys::Error),

    /// Failed to read signal information
    #[error("Failed to read signal information: {0}")]
    Read(sys::Error),

    /// Failed to set the signal mask
    #[error("Failed to set signal mask: {0}")]
    SetMask(sys::Error),

    /// Failed to build a signal set
    #[error("Failed to build signal set: {0}")]
    SigSet(sys::Error),

    /// Failed to create a signalfd
    #[error("Failed to create signalfd: {0}")]
    SignalFd(sys::Error),
}

impl TaskFailure for SignalHandlerError {
    fn task_failure(error: sys::Error) -> Self {
        SignalHandlerError::Read(error)
    }
}

/// Errors that may be encountered when interacting with the server socket
#[derive(Debug, Error)]
pub enum ServerSocketError {
    /// Failed to accept a new client connection
    #[error("Failure to accept a new client connection: {0}")]
    Accept(sys::Error),

    /// Failed to register a new client socket file descriptor
    #[error("Failure while registering new client socket: {0}")]
    ClientRegistration(FdRegistryError),

    /// Failed to create the server socket
    #[error("Failed to create the server socket: {0}")]
    Creation(sys::Error),

    /// Failed to create the server socket directory
    #[error("Failed to create server socket directory: {0}")]
    DirectoryCreation(std::io::Error),

    /// Failed to fstat the provided file descriptor
    #[error("Failed to fstat provided file descriptor: {0}")]
    Fstat(sys::Error),

    /// Failed to listen on the server socket
    #[error("Failed to listen on the server socket: {0}")]
    Listen(sys::Error),

    /// Failed to gather metadata about the socket
    #[error("Failed to gather socket metadata: {0}")]
    Metadata(sys::Error),

    /// Socket argument path already exists
    #[error("Socket argument path already exists: {0}")]
    PathExists(String),

    /// Socket argument path has no parent directory
    #[error("Socket path must have a parent directory: {0}")]
    PathHasNoParent(String),

    /// Failed to poll the server socket
    #[error("Failed to poll server socket: {0}")]
    Poll(sys::Error),

    /// Provided integer argument does not refer to an open file
    #[error("Provided integer argument does not refer to an open file: {0}")]
    ProvidedFdNotOpenFile(RawFd),

    /// Provided file descriptor does not refer to a sequence packet socket
    #[error("Provided file descriptor does not refer to a seqpacket socket: {0}")]
    ProvidedFdNotSeqPacket(RawFd),

    /// Provided file descriptor does not refer to a valid socket
    #[error("Provided file descriptor does not refer to a valid socket: {0}")]
    ProvidedFdNotSocket(RawFd),

    /// Too many clients have connected and the server is out of space to store
    /// their sockets.
    #[error("Too many clients attempted to connect. Dropped connection from {0:?}")]
    TooManyClients(Option<libc::ucred>),
}

impl TaskFailure for ServerSocketError {
    fn task_failure(error: sys::Error) -> Self {
        ServerSocketError::Accept(error)
    }
}

type ServerSocketResult<T> = Result<T, ServerSocketError>;

/// Errors that may be encountered when interacting with clients
#[derive(Debug, Error)]
pub enum ClientError {
    /// Failed to close a file descriptor
    #[error("Failed to close file descriptor {0}: {1}")]
    Close(RawFd, sys::Error),

    /// Failed to send IdentityQuery response
    #[error("Failed to send IdentityQuery response: {0}")]
    IdentityQueryResponse(RawFd, sys::Error),

    /// Provided FD is not a client socket
    #[error("Provided FD is not a client socket: {0}")]
    InvalidClientSocketFd(RawFd),

    /// Spawn message is missing spawn parameters
    #[error("Spawn message is missing spawn parameters")]
    MissingSpawnParams,

    /// Spawn message is missing a payload
    #[error("Spawn message is missing a payload")]
    MissingSpawnPayload,

    /// Failed to poll client socket
    #[error("Failed to poll client socket socket: {0}")]
    Poll(sys::Error),

    /// Failed to create a new process
    #[error("Failed to create new process: {0}")]
    ProcessCreationFailure(RawFd, sys::Error),

    /// Failed to read from client socket
    #[error("Failed to read from client socket: {0}")]
    Read(sys::Error),

    /// Failed to remove client file descriptor from the registry
    #[error("Failed to remove client file descriptor from the registry: {0}")]
    RegistryUpdate(#[from] FdRegistryError),

    /// Failed to re-initialize the server for a subspecies
    #[error("Failed to re-initialize the server for a subspecies")]
    Reinitialization,

    /// Failed to send Spawn response
    #[error("Failed to send Spawn response: {0}")]
    SpawnResponse(RawFd, sys::Error),

    /// Failed read process statistics
    #[error("Failed to read procfs stats file: {0}")]
    StatRead(RawFd, procfs::ProcFsError),

    /// Failed to send Stat response
    #[error("Failed to send Stat response: {0}")]
    StatResponse(RawFd, sys::Error),
}

impl TaskFailure for ClientError {
    fn task_failure(error: sys::Error) -> Self {
        ClientError::Read(error)
    }
}

type ClientResult<T> = Result<T, ClientError>;

/// Errors for the [`Server`] class.
#[derive(Debug, Error)]
pub enum ServerError {
    /// A failure was encountered when interacting with a client
    #[error(transparent)]
    ClientFailure(#[from] ClientError),

    /// A failure was encountered while setting effective permissions for the
    /// process
    #[error("Failed to set effective permissions: ({0:?}, {1:?}): {2}")]
    EffectivePermissionsFailure(Option<libc::uid_t>, Option<libc::gid_t>, sys::Error),

    /// Failed to epoll a client socket
    #[error("Failed to epoll file descriptor {0:?}: {1}")]
    Epoll(OpaqueEpollData, u32),

    /// A file descriptor registry action failed
    #[error("File descriptor registry failure: {0}")]
    FdRegistryFailure(#[from] FdRegistryError),

    /// Epoll data is invalid
    #[error("Invalid epoll data: {0}")]
    InvalidEpollData(u64),

    /// A failure was encountered when polling the server's file descriptors
    #[error("Failed to poll server file descriptors: {0}")]
    Poll(sys::Error),

    /// A failure was encountered when creating the server's process group
    #[error("Process group creation failure: {0}")]
    ProcessGroupFailure(sys::Error),

    /// A failure was encountered when interacting with the server socket
    #[error(transparent)]
    ServerSocketFailure(#[from] ServerSocketError),

    /// A failure was encountered while setting signal handlers or reading from
    /// the signalfd.
    #[error(transparent)]
    SignalHandlerFailure(#[from] SignalHandlerError),
}

/// Result type for [`Server`]
pub type ServerResult<T> = Result<T, ServerError>;

#[derive(Debug)]
enum ServerControl<T> {
    PollBreak,
    PollContinue,
    Shutdown,
    Trampoline(T),
}

type ServerControlResult<T, E> = std::result::Result<ServerControl<T>, E>;

/// The main data structure for the Zygote process server.
#[derive(Debug)]
pub struct Server {
    name: String,
    species: SpeciesRef,
    pid: libc::pid_t,

    registry: FileDescriptorRegistry,

    epoll_handle: EpollHandle,
    signal_fd: RawFd,
    server_socket: RawFd,
    client_sockets: ArrayVec<RawFd, BUFFER_SIZE_CLIENT_SOCKETS>,

    server_socket_path: Option<String>,

    preload_uid: Option<libc::uid_t>,
    preload_gid: Option<libc::gid_t>,

    spawn_params: SpawnParamsCommon,
}

impl Server {
    /// Create a new Zygote process server from a [`crate::config::Server`]
    /// reference.
    ///
    /// Add a destructor to clean up the socket if we create it.
    #[tracing::instrument(level = "trace", skip_all)]
    pub fn new(config: &config::Server) -> ServerResult<Self> {
        info!("Constructing server");

        let mut registry = FileDescriptorRegistry::new(config.species)?;

        let epoll_handle = EpollHandle::new().map_err(ServerError::Poll)?;
        // SAFETY: The epoll fd will be closed in two circumstances:
        //           1. The server is shutting down
        //           2. The server is reinitializing
        //
        //         The first case occurs in the drop implementation, closing
        //         file descriptor after any possible use.  In the second case
        //         the handler is immediately overwritten in
        //         [`Server::re_initialize_as_subspecies`].
        registry.register(unsafe { epoll_handle.fd() }, file_descriptors::Action::Close)?;

        let socket_path_or_fd = config.socket();
        let (server_socket, server_socket_path) = Self::get_server_socket(socket_path_or_fd)?;
        registry.register(server_socket, file_descriptors::Action::Close)?;
        epoll_handle
            .add(
                server_socket,
                libc::EPOLLIN as u32,
                EpollData::new(server_socket, EpollTag::Server).into(),
            )
            .map_err(ServerError::Poll)?;

        let sigset =
            sys::build_sigset(Self::blocked_signals()).map_err(SignalHandlerError::SigSet)?;
        // These masks are unblocked in Server::drop.
        sys::sigprocmask(libc::SIG_BLOCK, &sigset).map_err(SignalHandlerError::SetMask)?;
        let signal_fd =
            sys::signalfd(-1, &sigset, libc::SFD_NONBLOCK).map_err(SignalHandlerError::SignalFd)?;
        // The signal fd is reused after forking the subspecies process.
        // Per `man signalfd`:
        //     "After a fork(2), the child inherits a copy of the signalfd file
        //     descriptor.  A read(2) from the file descriptor in the child will
        //     return information about signals queued to the child."
        registry.register(signal_fd, file_descriptors::Action::CloseUnlessSpawnSubspecies)?;
        epoll_handle
            .add(
                signal_fd,
                libc::EPOLLIN as u32,
                EpollData::new(signal_fd, EpollTag::Signal).into(),
            )
            .map_err(ServerError::Poll)?;

        // Register any unregistered file descriptors such as those used for logging.
        registry.register_new()?;
        registry.audit().map_err(|error| ServerError::FdRegistryFailure(error.into()))?;

        let server = Self {
            name: config.name.clone(),
            species: config.species,
            pid: sys::getpid(),

            registry,

            epoll_handle,
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
        sys::setpgid(0, 0).map_err(ServerError::ProcessGroupFailure)?;

        server.preload(&config.preload_libraries)?;

        #[cfg(target_os = "android")]
        if let Err(errno) = sys::mallopt(libc::M_PURGE_ALL, 0) {
            error!("Failed to mallopt(M_PURGE_ALL): {errno}");
        }

        Ok(server)
    }

    /// Tailor the Server instance for the subspecies.
    // TODO: Set the process name.
    #[tracing::instrument(level = "trace", skip(self))]
    fn re_initialize_as_subspecies(&mut self, child_socket_path: String) -> ServerResult<()> {
        self.registry.reset_for_subspecies()?;

        let (child_socket_fd, child_socket_path) = Self::get_server_socket(child_socket_path)?;
        self.registry.register(child_socket_fd, file_descriptors::Action::Close)?;

        // All the RawFds of client sockets are closed in the
        // `reset_for_subspecies()` call above and they are stateless, thus
        // clear the client_sockets.
        self.client_sockets.clear();
        // The server RawFd is also already closed in
        // `reset_for_subspecies()`, so replace with a new one.
        self.server_socket = child_socket_fd;
        self.server_socket_path = child_socket_path;

        self.epoll_handle = EpollHandle::new().map_err(ServerError::Poll)?;
        self.registry
            // SAFETY: The epoll fd will be closed in two circumstances:
            //           1. The server is shutting down
            //           2. The server is reinitializing
            //
            //         The first case occurs in the drop implementation,
            //         closing file descriptor after any possible use.  The
            //         second case occurs in the call to
            //         [`FileDescriptorRegistry::reset_for_subspecies`] in this
            //         function.  We have just overwritten the previous handler
            //         with this new one.
            .register(unsafe { self.epoll_handle.fd() }, file_descriptors::Action::Close)?;
        self.epoll_handle
            .add(
                self.server_socket,
                libc::EPOLLIN as u32,
                EpollData::new(self.server_socket, EpollTag::Server).into(),
            )
            .map_err(ServerError::Poll)?;
        self.epoll_handle
            .add(
                self.signal_fd,
                libc::EPOLLIN as u32,
                EpollData::new(self.signal_fd, EpollTag::Signal).into(),
            )
            .map_err(ServerError::Poll)?;

        self.pid = sys::getpid();

        Ok(())
    }

    const fn blocked_signals() -> &'static [libc::c_int] {
        &[libc::SIGCHLD, libc::SIGINT, libc::SIGTERM]
    }

    /// Produce a socket file descriptor by one of the following methods:
    ///   * Using the provided integer as a file descriptor
    ///   * Opening a new socket and binding it to the provided path
    ///   * Opening a new socket and binding it to a default path
    fn get_server_socket(socket_path_or_fd: String) -> ServerSocketResult<(RawFd, Option<String>)> {
        if let Ok(fd) = socket_path_or_fd.parse::<RawFd>() {
            if !get_proc_fd_path(fd).exists() {
                return Err(ServerSocketError::ProvidedFdNotOpenFile(fd));
            }

            let stat = sys::fstat(fd).map_err(ServerSocketError::Fstat)?;
            if sys::get_file_type(stat) != libc::S_IFSOCK {
                return Err(ServerSocketError::ProvidedFdNotSocket(fd));
            }

            if sys::get_socket_type(fd).map_err(ServerSocketError::Metadata)?
                != libc::SOCK_SEQPACKET
            {
                return Err(ServerSocketError::ProvidedFdNotSeqPacket(fd));
            }

            sys::fcntl_setfl(fd, libc::O_NONBLOCK).map_err(ServerSocketError::Metadata)?;
            sys::listen(fd, SERVER_SOCKET_BACKLOG).map_err(ServerSocketError::Listen)?;

            Ok((fd, None))
        } else {
            let arg_path = Path::new(&socket_path_or_fd);

            let (socket_fd, server_socket_path) =
                if let Some(abs_socket_addr) = socket_path_or_fd.strip_prefix("@") {
                    (
                        sys::create_abstract_socket(abs_socket_addr, libc::SOCK_SEQPACKET)
                            .map_err(ServerSocketError::Creation)?,
                        None,
                    )
                } else {
                    if arg_path.exists() {
                        return Err(ServerSocketError::PathExists(socket_path_or_fd.clone()));
                    }
                    std::fs::create_dir_all(arg_path.parent().ok_or_else(|| {
                        ServerSocketError::PathHasNoParent(arg_path.to_string_lossy().to_string())
                    })?)
                    .map_err(ServerSocketError::DirectoryCreation)?;

                    (
                        sys::create_bound_socket(&socket_path_or_fd, libc::SOCK_SEQPACKET)
                            .map_err(ServerSocketError::Creation)?,
                        Some(socket_path_or_fd.clone()),
                    )
                };
            sys::fcntl_setfl(socket_fd, libc::O_NONBLOCK).map_err(ServerSocketError::Metadata)?;
            sys::listen(socket_fd, SERVER_SOCKET_BACKLOG).map_err(ServerSocketError::Listen)?;

            Ok((socket_fd, server_socket_path))
        }
    }

    fn check_epoll_events(
        &mut self,
        events: &[EpollEvent],
    ) -> ServerControlResult<impl FnOnce() -> Infallible + use<>, ServerError> {
        for event in events {
            let checked_event =
                event.check().map_err(|epoll_err| match EpollData::try_from(epoll_err.data()) {
                    Ok(epoll_data) => ServerError::Epoll(epoll_data.into(), epoll_err.flags()),
                    Err(err) => err,
                })?;

            let event_data: EpollData = checked_event.data().try_into()?;

            let epollin_retval =
                checked_event.try_handle_event(
                    libc::EPOLLIN as u32,
                    &mut |_| match event_data.class {
                        EpollTag::Client => Ok(self.handle_client_events(event_data.fd)?),
                        EpollTag::Server => Ok(self.handle_server_events(event_data.fd)?),
                        EpollTag::Signal => Ok(self.handle_signalfd_events(event_data.fd)?),
                    },
                );

            match epollin_retval {
                None | Some(Ok(ServerControl::PollContinue)) => {
                    let hup_result = checked_event.try_handle_event::<(), ServerError>(
                        libc::EPOLLHUP as u32,
                        |_| {
                            if event_data.class == EpollTag::Client {
                                info!("Client socket disconnected: {}", event_data.fd);
                                Ok(self.remove_client_socket(event_data.fd)?)
                            } else {
                                // The only FDs in the epoll set are: a listen
                                // socket, a signalfd, and zero or more unix-domain
                                // sockets.
                                unreachable!("Listen sockets and signalfds can't hangup.")
                            }
                        },
                    );

                    if let Some(Err(err)) = hup_result {
                        return Err(err);
                    }
                }
                Some(Err(ServerError::ClientFailure(error))) => {
                    error!(
                        "Error encountered while responding to client socket {}: {error:?}",
                        event_data.fd
                    );

                    self.remove_client_socket(event_data.fd).map_err(ServerError::ClientFailure)?;
                }
                Some(Err(ServerError::ServerSocketFailure(
                    err @ ServerSocketError::TooManyClients(_),
                ))) => {
                    warn!("{err:?}");
                }
                Some(result) => {
                    return result;
                }
            }
        }

        Ok(ServerControl::PollBreak)
    }

    fn handle_client_events(
        &mut self,
        client_fd: RawFd,
    ) -> ServerControlResult<impl FnOnce() -> Infallible + use<>, ClientError> {
        let client_event_status = sys::try_until_would_block(
            || sys::recvmsg::<MESSAGE_BUFFER_SIZE>(client_fd),
            &mut |(readlen, message_buffer): (isize, MessageBuffer)| {
                if readlen == 0 {
                    return Ok::<_, ClientError>(Break(ServerControl::PollContinue));
                }

                match Message::try_from_parcel(&message_buffer) {
                    Ok(_) => match self.dispatch_message_handler(client_fd, message_buffer) {
                        Continue => Ok(Continue),
                        Break(value) => Ok(Break(value?)),
                    },
                    Err(err) => {
                        // TODO: Respond with an error
                        warn!(
                            "Invalid message received from client ({:?}): {}",
                            sys::get_socket_creds(client_fd),
                            err
                        );
                        Ok(Break(ServerControl::PollContinue))
                    }
                }
            },
        )?;

        Ok(match client_event_status {
            // All available messages were read from the socket
            LoopExit::WouldBlock => ServerControl::PollContinue,
            LoopExit::Early(control) => control,
        })
    }

    fn handle_server_events<Thunk: FnOnce() -> Infallible>(
        &mut self,
        server_fd: RawFd,
    ) -> ServerControlResult<Thunk, ServerSocketError> {
        sys::try_until_would_block(|| sys::accept(server_fd), &mut |new_client_fd| {
            info!(
                "Accepted new client socket connection from {:?}",
                sys::get_socket_creds(new_client_fd)
            );

            sys::fcntl_setfl(new_client_fd, libc::O_NONBLOCK)
                .map_err(ServerSocketError::Metadata)?;

            if self.client_sockets.len() == BUFFER_SIZE_CLIENT_SOCKETS {
                let client_creds = sys::get_socket_creds(new_client_fd);
                let _ = sys::close(new_client_fd);
                return Err(ServerSocketError::TooManyClients(client_creds.ok()));
            }

            self.client_sockets.push(new_client_fd);
            self.registry
                .register(new_client_fd, file_descriptors::Action::Close)
                .map_err(ServerSocketError::ClientRegistration)?;
            self.epoll_handle
                .add(
                    new_client_fd,
                    libc::EPOLLIN as u32,
                    EpollData::new(new_client_fd, EpollTag::Client).into(),
                )
                .map_err(ServerSocketError::Poll)?;

            // Continue reading
            Ok::<_, ServerSocketError>(LoopControl::<()>::Continue)
        })
        .map(|_| ServerControl::<Thunk>::PollContinue)
    }

    #[tracing::instrument(level = "trace", skip_all)]
    fn handle_signalfd_events<Thunk: FnOnce() -> Infallible>(
        &mut self,
        signalfd: RawFd,
    ) -> ServerControlResult<Thunk, SignalHandlerError> {
        sys::try_until_would_block(
            || sys::read_exact::<libc::signalfd_siginfo>(signalfd),
            &mut |siginfo: libc::signalfd_siginfo| {
                match siginfo.ssi_signo as i32 {
                    libc::SIGCHLD => {
                        let pid = siginfo.ssi_pid as libc::pid_t;
                        info!(
                            "Received SIGCHLD from PID {} with status {}",
                            pid, siginfo.ssi_status
                        );

                        match sys::waitpid(Some(pid), libc::WNOHANG) {
                            Ok(Some((ret_pid, status))) => {
                                // We provide an exact PID so we should
                                // always receive the same PID as the
                                // result.
                                debug_assert_eq!(ret_pid, pid);

                                self.species.handle_sigchld(
                                    pid,
                                    siginfo.ssi_uid,
                                    siginfo.ssi_status,
                                );
                                info!("Child process {} terminated with status {:?}", pid, status);
                                Ok(Continue)
                            }
                            Ok(None) => Ok(Continue),
                            Err(error) => Err(SignalHandlerError::ProcessAccounting(error)),
                        }
                    }
                    libc::SIGINT => {
                        info!("Received SIGINT FROM PID {}", siginfo.ssi_pid);

                        // Terminate early
                        Ok(Break(libc::SIGINT))
                    }
                    libc::SIGTERM => {
                        info!("Received SIGTERM FROM PID {}", siginfo.ssi_pid);

                        // Terminate early
                        Ok(Break(libc::SIGTERM))
                    }
                    signo => {
                        // This should never happen as only SIGCHLD,
                        // SIGINT, and SIGTERM are added to the signalfd's
                        // mask.
                        unreachable!("Unhandled signal received: {signo}");
                    }
                }
            },
        )
        .map(|event_result| match event_result {
            LoopExit::Early(_) => ServerControl::<Thunk>::Shutdown,
            LoopExit::WouldBlock => ServerControl::PollContinue,
        })
    }

    fn dispatch_message_handler(
        &mut self,
        fd: RawFd,
        message_buffer: MessageBuffer,
    ) -> LoopControl<ServerControlResult<impl FnOnce() -> Infallible + use<>, ClientError>> {
        match Message::try_from_parcel(&message_buffer) {
            Ok(message) => {
                if cfg!(debug_assertions) {
                    info!(
                        "Received message from client ({:?}): {:?}",
                        sys::get_socket_creds(fd),
                        message
                    );
                } else {
                    info!(
                        "Received message from PID {:?}: {}",
                        sys::get_socket_creds(fd),
                        message.variant_name()
                    );
                }

                match message {
                    Message::Exit => self.handle_message_exit(fd),
                    Message::IdentityQuery => self.handle_message_identity_query(fd),
                    Message::Spawn { payload, .. } | Message::SpawnSubspecies { payload, .. }
                        if !self.species.is_spawn_payload_type(&payload) =>
                    {
                        // TODO: Respond with an error
                        error!(
                            "Incorrect spawn payload for this species {}: {:?}",
                            self.species.name(),
                            payload
                        );

                        Continue
                    }
                    Message::Spawn { .. } => self.handle_message_spawn(fd, message_buffer),
                    Message::SpawnSubspecies { socket_path, .. } => self
                        .handle_message_spawn_subspecies(
                            fd,
                            socket_path.to_string(),
                            message_buffer,
                        ),
                    Message::Stat => self.handle_message_stat(fd),
                    msg => {
                        warn!(
                            "Invalid message type received from client ({:?}): {}",
                            sys::get_socket_creds(fd),
                            msg.variant_name()
                        );

                        Continue
                    }
                }
            }
            Err(err) => {
                if let Ok(socket_creds) = sys::get_socket_creds(fd) {
                    warn!("Invalid message received from client PID {}: {err}", socket_creds.pid);
                } else {
                    warn!("Invalid message received from client: {err}");
                }

                Continue
            }
        }
    }

    fn handle_message_exit<Thunk: FnOnce() -> Infallible>(
        &mut self,
        fd: RawFd,
    ) -> LoopControl<ServerControlResult<Thunk, ClientError>> {
        if let Err(errno) = sys::sendmsg(fd, Message::AckResponse.to_parcel().finished_data()) {
            warn!("Failed to acknowledge Exit message: {errno}")
        }

        LoopControl::Break(Ok(ServerControl::Shutdown))
    }

    #[tracing::instrument(level = "trace", skip_all)]
    fn handle_message_identity_query<Thunk: FnOnce() -> Infallible>(
        &self,
        fd: RawFd,
    ) -> LoopControl<ServerControlResult<Thunk, ClientError>> {
        let response = Message::IdentityQueryResponse {
            name: &self.name,
            species: self.species.name(),
            arch: std::env::consts::ARCH,
        };

        match sys::sendmsg(fd, response.to_parcel().finished_data()) {
            Ok(_) => LoopControl::Continue,
            Err(error) => {
                error!("Failed to send IdentityQuery response: {error}");
                LoopControl::Break(Err(ClientError::IdentityQueryResponse(fd, error)))
            }
        }
    }

    #[tracing::instrument(level = "trace", skip_all)]
    fn handle_spawn<Thunk: FnOnce() -> Infallible>(
        &mut self,
        fd: RawFd,
        message_buffer: MessageBuffer,
        child_continuation: impl FnOnce(
            &mut Self,
            SpawnParamsCommon,
        ) -> ServerControlResult<Thunk, ClientError>,
    ) -> LoopControl<ServerControlResult<Thunk, ClientError>> {
        // The server does not spawn any threads.  Preloaded library
        // initializers should not start any threads.  Any threads created by
        // species-specific code during initialization must be terminated when
        // control is returned to the process-server.
        debug_assert_single_threaded();
        debug_assert_ok!(self.registry.audit());

        let message =
            Message::try_from_parcel(&message_buffer).expect("Verified message is now invalid");

        // TODO: Implement logic to lock some or all of the common spawn
        //       parameters, preventing them from being set by a spawn message.
        let Some(spawn_params) =
            message.get_spawn_params().map(|params| params.or(&self.spawn_params))
        else {
            return Break(Err(ClientError::MissingSpawnParams));
        };

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
                match sys::sendmsg(fd, response.to_parcel().finished_data()) {
                    Ok(_) => Continue,
                    Err(error) => {
                        error!("Failed to send Spawn response: {error}");
                        Break(Err(ClientError::SpawnResponse(fd, error)))
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

                Break(Err(ClientError::ProcessCreationFailure(fd, errno.into())))
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
    ) -> LoopControl<ServerControlResult<impl FnOnce() -> Infallible + use<>, ClientError>> {
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

            Ok(ServerControl::Trampoline(move || {
                debug_assert_single_threaded();

                // This function call must occur here, at the top of the child
                // process's stack, to avoid segfaults from changing the stack
                // guard in a callee and then segfaulting when return to the
                // caller's frame.
                child_process::maybe_reset_stack_guards(move || {
                    // Unpack the message in the child process
                    let message = Message::try_from_parcel(spawn_message.as_ref())
                        .expect("Verified message is now invalid");

                    // Due to the architecture of the Zygote, this lambda is
                    // executed in the child process and can not return.  For
                    // this reason we call `expect()` here.  At this point any
                    // error is fatal.
                    let spawn_payload = message.get_spawn_payload().expect("Missing spawn payload");

                    child_process::re_initialize(
                        species,
                        re_init_data,
                        &spawn_params,
                        spawn_payload,
                    );

                    species.gestate(&spawn_params, spawn_payload)
                })
            }))
        })
    }

    #[tracing::instrument(level = "trace", skip_all)]
    fn handle_message_spawn_subspecies<Thunk: FnOnce() -> Infallible>(
        &mut self,
        fd: RawFd,
        socket_path: String,
        message_buffer: MessageBuffer,
    ) -> LoopControl<ServerControlResult<Thunk, ClientError>> {
        let re_init_data = self.species.gather_reinitialization_data();
        // Creating local copies avoids capturing additional references.
        let species: SpeciesRef = self.species;

        self.handle_spawn(fd, message_buffer, move |server, spawn_params| {
            let message =
                Message::try_from_parcel(&message_buffer).expect("Verified message is now invalid");
            let spawn_payload =
                message.get_spawn_payload().ok_or(ClientError::MissingSpawnPayload)?;
            child_process::re_initialize(species, re_init_data, &spawn_params, spawn_payload);

            server
                .re_initialize_as_subspecies(socket_path)
                .map_err(|_| ClientError::Reinitialization)?;

            species.speciate(spawn_payload);
            Ok(ServerControl::PollBreak)
        })
    }

    #[tracing::instrument(level = "trace", skip_all)]
    fn handle_message_stat<Thunk: FnOnce() -> Infallible>(
        &mut self,
        fd: RawFd,
    ) -> LoopControl<ServerControlResult<Thunk, ClientError>> {
        let proc = match ProcStat::get() {
            Ok(proc) => proc,
            Err(err) => {
                error!("Failed to get ProcStat: {err:?}");
                return Break(Err(ClientError::StatRead(fd, err)));
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

        match sys::sendmsg(fd, response.to_parcel().finished_data()) {
            Ok(_) => {
                // Continue the `recvmsg` loop
                LoopControl::Continue
            }
            Err(error) => {
                error!("Failed to send Stat response: {error}");
                LoopControl::Break(Err(ClientError::StatResponse(fd, error)))
            }
        }
    }

    #[tracing::instrument(level = "trace", skip_all)]
    fn preload<T>(&self, libraries: &Vec<T>) -> ServerResult<()>
    where
        T: AsRef<OsStr> + std::fmt::Debug + tracing::Value,
    {
        let _eid_context = sys::EffectiveIdContext::enter(self.preload_uid, self.preload_gid)
            .map_err(|error| {
                ServerError::EffectivePermissionsFailure(self.preload_uid, self.preload_gid, error)
            })?;

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

        Ok(())
    }

    fn remove_client_socket(&mut self, fd: RawFd) -> ClientResult<()> {
        self.registry.remove(fd)?;
        self.epoll_handle.remove(fd).map_err(ClientError::Poll)?;
        self.client_sockets.remove(
            self.client_sockets
                .iter()
                .position(|&search_fd| search_fd == fd)
                .ok_or_else(|| ClientError::InvalidClientSocketFd(fd))?,
        );

        sys::close(fd).map(|_| ()).map_err(|err| ClientError::Close(fd, err))
    }

    /// Execute the main server loop until the server receives a SIGTERM or
    /// [`zygote_messages::Exit`] message.
    ///
    /// The child process thunk is returned to allow the caller to drop the
    /// server before control flow is transferred to the species-specific
    /// code.  This allows resources to be cleaned up and possibly sensitive
    /// data to be deallocated.
    pub fn serve(&mut self) -> ServerResult<Option<impl FnOnce() -> Infallible + use<>>> {
        self.species.on_server_ready();

        let mut event_buf = [EpollEvent::new(0, u64::MAX); file_descriptors::REGISTRY_SIZE];

        loop {
            let num_ready =
                self.epoll_handle.wait(&mut event_buf, -1).map_err(ServerError::Poll)?;

            // TODO: Dump the file descriptor table before returning an Err.
            match self.check_epoll_events(&event_buf[..num_ready])? {
                ServerControl::PollContinue | ServerControl::PollBreak => {}
                ServerControl::Shutdown => {
                    return Ok(None);
                }
                ServerControl::Trampoline(thunk) => {
                    return Ok(Some(thunk));
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
            // Clean up the server's resources in the child process.  As we do
            // not want code executing in an unknown context we will treat all
            // errors as fatal.

            self.registry
                .execute_actions(ForkType::Application)
                .expect("Failed to execute file descriptor registry actions");

            let sigset =
                sys::build_sigset(Self::blocked_signals()).expect("Failed to build signal set");
            sys::sigprocmask(libc::SIG_UNBLOCK, &sigset).expect("Failed to unset signal mask");
        }
    }
}
