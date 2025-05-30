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

use std::ffi::OsStr;
use std::{os::fd::RawFd, path::Path};

use anyhow::{anyhow, bail, Result};
use arrayvec::ArrayVec;
use flatbuffers::{self};
use libloading::os::unix::{Library, RTLD_GLOBAL, RTLD_NOW};
use log::{error, info, warn};

use crate::{
    assert_ok, config, debug_assert_ok,
    file_descriptors::{self, FileDescriptorRegistry},
    introspection::{debug_assert_single_threaded, get_proc_fd_path},
    messages::{self, Message, MessageBuffer, Parcel, ToFlatBuffer, MESSAGE_BUFFER_SIZE},
    species::SpeciesRef,
    sys::{
        self, LibcResult,
        LoopControl::{self, *},
        LoopExit, PollFd,
    },
};

const BUFFER_SIZE_CLIENT_SOCKETS: usize = 16;
const BUFFER_SIZE_POLL: usize = 64;
const SERVER_SOCKET_BACKLOG: core::ffi::c_int = 10;
const ZYGOTE_SOCKET_PREFIX: &str = "/dev/socket/";

type PollBuffer = ArrayVec<PollFd, BUFFER_SIZE_POLL>;

impl std::convert::From<&mut Server> for PollBuffer {
    fn from(server: &mut Server) -> PollBuffer {
        let mut poll_buffer = PollBuffer::new();

        poll_buffer.push(PollFd::new(server.signal_fd, libc::POLLIN));
        poll_buffer.push(PollFd::new(server.server_socket, libc::POLLIN));

        for client_socket in &server.client_sockets {
            poll_buffer.push(PollFd::new(*client_socket, libc::POLLIN));
        }

        poll_buffer
    }
}

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

enum ClientLoopControl<T> {
    Child(T),
    NextSocket,
    Error(RawFd, sys::Errno),
    Shutdown,
}

/// The main data structure for the Zygote process server.
pub struct Server {
    name: String,
    species: SpeciesRef,
    pid: libc::pid_t,

    registry: FileDescriptorRegistry,

    signal_fd: RawFd,
    server_socket: RawFd,
    client_sockets: ArrayVec<RawFd, BUFFER_SIZE_CLIENT_SOCKETS>,

    priority_initial: Option<i32>,
    priority_final: Option<i32>,
    server_socket_path: Option<String>,

    preload_uid: Option<libc::uid_t>,
    preload_gid: Option<libc::gid_t>,
}

impl Server {
    /// Create a new Zygote process server from a [`crate::config::Config`]
    /// reference.
    ///
    /// Add a destructor to clean up the socket if we create it.
    pub fn new(config: config::Server) -> Self {
        let mut registry = FileDescriptorRegistry::new(config.species);

        let (server_socket, server_socket_path) = Self::get_server_socket(&config).unwrap();
        registry.register(server_socket, file_descriptors::Action::Close);

        let sigset = sys::build_sigset(&[libc::SIGCHLD, libc::SIGINT, libc::SIGTERM]).unwrap();
        sys::sigprocmask(libc::SIG_BLOCK, &sigset).unwrap();
        let signal_fd = sys::signalfd(-1, &sigset, libc::SFD_NONBLOCK).unwrap();
        registry.register(signal_fd, file_descriptors::Action::Close);

        let server = Self {
            name: config.name,
            species: config.species,
            pid: sys::getpid(),

            registry,

            signal_fd,
            server_socket,
            client_sockets: ArrayVec::new(),

            priority_initial: config.priority_initial,
            priority_final: config.priority_final,
            server_socket_path,

            preload_uid: config.preload_uid,
            preload_gid: config.preload_gid,
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
            error!("Failed to create process group for Zygote server: {}", errno);
            std::process::exit(1);
        }

        server.preload(&config.preload_libraries);

        #[cfg(target_os = "android")]
        if let Err(errno) = sys::mallopt(libc::M_PURGE_ALL, 0) {
            error!("Failed to mallopt(M_PURGE_ALL): {}", errno);
        }

        server
    }

    /// Produce a socket file descriptor by one of the following methods:
    ///   * Using the provided integer as a file descriptor
    ///   * Opening a new socket and binding it to the provided path
    ///   * Opening a new socket and binding it to a default path
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
    ) -> ServerControl<impl FnOnce() + use<>> {
        if self.check_signalfd_events(&partition).is_exit() {
            return ServerControl::Shutdown;
        }

        self.check_server_socket_events(&partition);
        self.check_client_sockets_events(&partition)
    }

    fn check_client_sockets_events(
        &mut self,
        partition: &PollPartition<'_>,
    ) -> ServerControl<impl FnOnce() + use<>> {
        for client_pollfd in partition.clients {
            // POLLERR and POLLNVAL should never occur for a client socket.
            let checked_pollfd = client_pollfd.check().unwrap_or_else(|(fd, error_events)| {
                panic!("Received polling error for client socket {}: {:?}", fd, error_events)
            });

            let pollin_result = checked_pollfd.handle_event(libc::POLLIN, &mut |fd| {
                sys::call_until_would_block(
                    || sys::recvmsg::<MESSAGE_BUFFER_SIZE>(fd),
                    &mut |(readlen, message_buffer): (isize, MessageBuffer)| {
                        if readlen == 0 {
                            return Break(ClientLoopControl::NextSocket);
                        }

                        let parcel = flatbuffers::root::<Parcel>(&message_buffer).unwrap();

                        self.dispatch_message_handler(fd, message_buffer, parcel)
                    },
                )
                // TODO: Add more extensive error handling.
                .unwrap()
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
                        ClientLoopControl::NextSocket => {
                            // Zero-length read from socket, continue and wait for SIGHUP
                        }
                        ClientLoopControl::Error(fd, errno) => {
                            error!(
                                "Error encountered while responding to client socket {fd}: {errno}"
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

    fn check_server_socket_events(&mut self, partition: &PollPartition<'_>) {
        // POLLERR and POLLNVAL should never occur for the server socket.
        let checked_pollfd = partition.server.check().unwrap_or_else(|(_, error_events)| {
            panic!("Received polling error for server socket: {error_events:?}");
        });

        checked_pollfd.handle_event(libc::POLLIN, &mut |fd| {
            sys::call_until_would_block(|| sys::accept(fd), &mut |new_client_fd| {
                info!(
                    "Accepted new client socket connection from PID {}",
                    sys::get_socket_creds(new_client_fd).unwrap().pid
                );

                sys::fcntl_setfl(new_client_fd, libc::O_NONBLOCK).unwrap();

                self.client_sockets.push(new_client_fd);
                self.registry.register(new_client_fd, file_descriptors::Action::Close);

                // Continue reading
                LoopControl::<()>::Continue
            })
            // TODO: Add more extensive error handling.
            .unwrap();
        });

        // We don't need to check to see if a POLLIN event was handled as not
        // receiving any new connections is valid.

        // No need to check for POLLHUP as it should never occur for a
        // listen socket.
    }

    fn check_signalfd_events(
        &mut self,
        partition: &PollPartition<'_>,
    ) -> ServerControl<impl FnOnce()> {
        // POLLERR and POLLNVAL should never occur for a signalfd.
        let checked_pollfd = partition.signal.check().unwrap_or_else(|(_, error_events)| {
            panic!("Received polling error for signalfd: {error_events:?}");
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
                .unwrap()
            })
            // The server should exit early iff the handler exited
            // early due to receiving a SIGINT or SIGTERM.
            .map_or(ServerControl::<fn()>::Continue, |loop_control| match loop_control {
                LoopExit::Early(_) => ServerControl::Shutdown,
                LoopExit::WouldBlock => ServerControl::Continue,
            })
    }

    fn dispatch_message_handler(
        &mut self,
        fd: RawFd,
        message_buffer: MessageBuffer,
        parcel: Parcel<'_>,
    ) -> LoopControl<ClientLoopControl<impl FnOnce()>> {
        match parcel.message_type() {
            Message::Exit => self.handle_message_exit(fd),
            Message::IdentityQuery => self.handle_message_identity_query(fd),
            msg if msg == self.species.message_type_spawn() => {
                self.handle_message_spawn(fd, message_buffer)
            }
            Message::Stat => self.handle_message_stat(fd),
            msg @ Message(tag) if tag < Message::ENUM_MAX => {
                error!("Message not supported by this species: {}", msg.variant_name().unwrap());

                // Continue the `recvmsg` loop
                Continue
            }
            Message(tag) => {
                error!("Invalid message variant encountered: {tag}");

                // Continue the `recvmsg` loop
                Continue
            }
        }
    }

    fn handle_message_exit<Thunk: FnOnce()>(
        &mut self,
        fd: RawFd,
    ) -> LoopControl<ClientLoopControl<Thunk>> {
        info!("Received client: (Exit {})", sys::get_socket_creds(fd).unwrap().pid);

        let ack_msg = messages::AckBuilder {}.build();
        if let Err(errno) = Self::send_response(fd, ack_msg.finished_data()) {
            warn!("Failed to acknowledge Exit message: {errno}")
        }

        Break(ClientLoopControl::Shutdown)
    }

    fn handle_message_identity_query<Thunk: FnOnce()>(
        &self,
        fd: RawFd,
    ) -> LoopControl<ClientLoopControl<Thunk>> {
        info!("Received client: (IdentityQuery {})", sys::get_socket_creds(fd).unwrap().pid);

        let response = messages::IdentityQueryResponseBuilder {
            name: &self.name,
            species: self.species.name(),
            arch: std::env::consts::ARCH,
        }
        .build();

        match Self::send_response(fd, response.finished_data()) {
            Ok(_) => Continue,
            Err(errno) => {
                error!("Failed to send IdentityQuery response: {errno}");
                Break(ClientLoopControl::Error(fd, errno))
            }
        }
    }

    fn handle_message_spawn(
        &mut self,
        fd: RawFd,
        message_buffer: MessageBuffer,
    ) -> LoopControl<ClientLoopControl<impl FnOnce() + use<>>> {
        // The server does not spawn any threads.  Preloaded library
        // initializers should not start any threads.  Any threads created by
        // species-specific code during initialization must be terminated when
        // control is returned to the process-server.
        debug_assert_single_threaded();
        debug_assert_ok!(self.registry.audit());

        let parcel = flatbuffers::root::<Parcel>(&message_buffer).unwrap();
        info!("Received message: ({:?})", parcel.message_type());

        #[cfg(target_os = "android")]
        // SAFETY: This is called in a single-threaded context
        let fds_error_level = unsafe { sys::android::fdsan_get_error_level() };

        // SAFETY: This is called in a single-threaded context.
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

            if let Some(priority) = self.priority_initial {
                if sys::setpriority(libc::PRIO_PROCESS, 0, priority).is_err() {
                    // EINVAL, EPERM, and ESRCH only apply when setting the
                    // priority of other processes.
                    warn!("Insufficient permissions to set priority: {}", priority);
                }
            }

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

            // Creating these local copies avoids capturing additional
            // references.

            let species: SpeciesRef = self.species;
            let priority_final = self.priority_final;

            Break(ClientLoopControl::Child(move || {
                debug_assert_single_threaded();

                #[cfg(target_os = "android")]
                {
                    sys::android::reset_stack_guards();

                    // SAFETY: This is called in a single-threaded context
                    unsafe {
                        sys::android::fdsan_set_error_level(fds_error_level);
                    }

                    if sys::android::set_zygote_child().is_err() {
                        error!("Failed to android_mallopt(M_SET_ZYGOTE_CHILD)");
                    }

                    if let Err(errno) = sys::mallopt(libc::M_DECAY_TIME, 1) {
                        error!("Failed to mallopt(M_DECAY_TIME): {}", errno);
                    }
                }

                species.gestate(spawn_message, priority_final)
            }))
        } else {
            // Server process
            info!("Spawned process {}", new_pid);
            let response = messages::SpawnResponseBuilder { pid: new_pid }.build();
            match Self::send_response(fd, response.finished_data()) {
                Ok(_) => Continue,
                Err(errno) => {
                    error!("Failed to send Spawn response: {}", errno);
                    Break(ClientLoopControl::Error(fd, errno))
                }
            }
        }
    }

    fn handle_message_stat<Thunk: FnOnce()>(
        &mut self,
        fd: RawFd,
    ) -> LoopControl<ClientLoopControl<Thunk>> {
        info!("Received client: (Stat)");

        let ack_msg = messages::AckBuilder {}.build();
        match Self::send_response(fd, ack_msg.finished_data()) {
            Ok(_) => {
                // Continue the `recvmsg` loop
                Continue
            }
            Err(errno) => {
                error!("Failed to send Stat response: {}", errno);
                Break(ClientLoopControl::Error(fd, errno))
            }
        }
    }

    fn preload<T: AsRef<OsStr>>(&self, libraries: &Vec<T>) {
        let _eid_context = sys::EffectiveIdContext::enter(self.preload_uid, self.preload_gid)
            .unwrap_or_else(|errno| {
                error!(
                    "Failed to set effective UID/GID ({:?}/{:?}): {}",
                    self.preload_uid, self.preload_gid, errno
                );
                std::process::exit(1);
            });

        for library_path in libraries {
            // SAFETY: The Zygote process-server is designed to load and
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
    }

    fn remove_client_socket(&mut self, fd: RawFd) {
        self.registry.remove(fd).unwrap();
        self.client_sockets
            .remove(self.client_sockets.iter().position(|&search_fd| search_fd == fd).unwrap());
        // Silently ignore EBADF and EIO.
        let _ = sys::close(fd);
    }

    /// Send the provided response to on the file descriptor and panic on
    /// errors that indicate an irrecoverable bug.
    ///
    /// The following errors will cause a panic:
    /// * [`libc::EACCES`]
    /// * [`libc::EALREADY`]
    /// * [`libc::EBADF`]
    /// * [`libc::EDESTADDRREQ`]
    /// * [`libc::EFAULT`]
    /// * [`libc::EINVAL`]
    /// * [`libc::EISCONN`]
    /// * [`libc::EMSGSIZE`]
    /// * [`libc::ENOBUFS`]
    /// * [`libc::ENOMEM`]
    /// * [`libc::ENOTCONN`]
    /// * [`libc::ENOTSOCK`]
    /// * [`libc::EOPNOTSUPP`]
    /// * [`libc::EPIPE`]
    ///
    /// The following errors will be returned to the caller:
    /// * [`libc::EAGAIN`]
    /// * [`libc::EWOULDBLOCK`]
    /// * [`libc::ECONNRESET`]
    fn send_response(fd: RawFd, buffer: &[u8]) -> LibcResult<()> {
        match sys::sendmsg(fd, buffer) {
            Ok(_) => Ok(()),
            Err(errno) if errno.matches(&[libc::EAGAIN | libc::EWOULDBLOCK | libc::ECONNRESET]) => {
                Err(errno)
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
    pub fn serve(&mut self) -> Option<impl FnOnce()> {
        loop {
            let mut poll_array = PollBuffer::from(&mut *self);

            // Discard the number of ready file descriptors for now.
            sys::poll(&mut poll_array, -1).unwrap();

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
