# Android Zygote

This is the Rust implementation of the Android Zygote process. The Zygote is a fundamental component of the Android operating system, responsible for preloading core libraries and quickly spawning new application processes via `fork()`.

## Project Structure

The project is modularized into several crates:

*   **`zygote-sys`**: The foundational low-level crate. It acts as the safe Foreign Function Interface (FFI) boundary, wrapping raw OS interactions (POSIX/Linux syscalls via `libc` and Android-specific APIs) into idiomatic, safe Rust.
*   **`zygote-core`**: The core logic and state management for the Zygote daemon.
*   **`zygote-messages`**: Defines the communication protocols and message formats used between the system server and the Zygote.
*   **`zygote-proc-macros`**: Procedural macros used across the project to reduce boilerplate (e.g., automatically deriving IPC message serialization).
*   **`memmark`**: Memory marking utilities.

## Architecture Highlights

### `zygote-sys`
The `zygote-sys` sub-crate is critical to the security and stability of the Zygote. It ensures that the rest of the application does not need to handle unsafe C pointers or raw file descriptors directly.

Key features include:
*   **Safe System Calls:** Idiomatic wrappers around `fork`, `clone3`, `epoll_wait`, socket operations, and credential management (`setuid`, `setgid`).
*   **Platform Integrations:** Safe bindings to Android's `selinux` (for context switching) and `processgroup` (for CPU and scheduling policy management).
*   **Process Introspection:** Utilities for parsing `/proc/self/stat` to monitor memory usage and safely iterating over open file descriptors to prevent leaks across process boundaries.

### `zygote-messages`
The `zygote-messages` crate defines the rigid IPC contract for the system. It leverages **FlatBuffers** (`schemas/messages.fbs`) to establish a zero-copy, cross-language messaging standard between the Android System Server (C++/Java) and the Zygote daemon (Rust). It parses requests for spawning specific payload variants (e.g., `AndroidNative`, `LibApp`) and configures constraints like capabilities, priority, and `rlimits`.

### `zygote-proc-macros`
To maintain a robust and bug-free IPC layer, the `zygote-proc-macros` crate exposes custom `#[derive]` macros (`MarshalParcel`, `UnmarshalParcel`, `FlattenParcel`). These automatically generate the complex binary serialization and deserialization code needed to map between raw FlatBuffers and safe Rust structs inside `zygote-messages`.

### `zygote-core`
The `zygote-core` crate manages the fundamental operational state and standardizes the runtime environment for the daemon. Most notably, it acts as the centralized hub for telemetry. It provides a highly controlled initialization sequence (`init_reporting`) that unifies standard Rust logging with the `tracing` framework and conditionally pipes events directly into Android's low-level `atrace` mechanism.

### `zygote` (Daemon)
The `zygote` crate houses the ultimate server implementation and the resulting system binaries (like `zygote_next`). It binds all the lower-level libraries together to provide:
*   **The Server Loop:** An `epoll`-driven asynchronous event loop capable of multiplexing numerous incoming IPC spawn requests efficiently.
*   **Species Polymorphism:** A trait-based system (`Species`) that allows a single Zygote daemon to securely manage the diverse setup requirements (seccomp filters, library preloading, uid/gid transitions) for completely different types of processes (e.g., `AndroidNative` apps vs. isolated `LibApp` instances).
*   **File Descriptor Sanitization:** A rigorous, allowlist-based file descriptor registry that prevents malicious or accidental file descriptor leaks across process boundaries during a `fork`.

## Development and Testing

Please refer to the `TESTING.md` file for instructions on running the test suite.
