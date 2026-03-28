# Android Zygote

This is the Rust implementation of the Android Zygote process. The Zygote is a fundamental component of the Android operating system, responsible for preloading core libraries and quickly spawning new application processes via `fork()`.

## Project Structure & Architecture Highlights

The project is modularized into several crates:

### `zygote-sys`
The foundational low-level crate. It acts as the safe Foreign Function Interface (FFI) boundary, wrapping raw OS interactions (POSIX/Linux syscalls via `libc` and Android-specific APIs) into idiomatic, safe Rust.

**Core Responsibilities:**
- **Memory Safety:** Encapsulates raw pointers and buffer allocations (e.g., C-strings).
- **Idiomatic Error Handling:** Translates raw `libc` integer error codes into the standard Rust `Result<T, Error>` pattern using the custom `Errno` type.
- **System Call Wrappers:** Provides safe interfaces for standard POSIX/Linux syscalls (`fork`, `clone3`, `open`, `read`, `pipe`, `socket`, `bind`, `accept`, `epoll_wait`, `setuid`/`setgid`, `prctl`, etc.).
- **Android-Specific APIs:** Includes bindings for Android-specific functionality (e.g., `set_selinux_context`, `set_cpuset_policy`).
- **Process Introspection:** Contains utilities (`procfs.rs`) for querying `/proc/self` to parse process statistics (`ProcStat`) and safely manage open file descriptors.

### `zygote-proc-macros`
Procedural macros used across the project to reduce boilerplate (e.g., automatically deriving IPC message serialization).

**Core Responsibilities:**
- **Message Serialization:** `#[derive(MarshalParcel)]` for marshalling Rust structs/enums.
- **Message Deserialization:** `#[derive(UnmarshalParcel)]` for unmarshalling IPC messages.
- **Message Flattening:** `#[derive(FlattenParcel)]` to optimize message layouts by flattening nested structures.

### `zygote-messages`
Defines the communication protocols and message formats used between the system server and the Zygote.

**Core Responsibilities:**
- **IPC Schema:** Houses the FlatBuffers schema definition (`schemas/messages.fbs`).
- **Rust Bindings:** Provides strongly-typed Rust structures (e.g., `Message`, `SpawnParamsCommon`).
- **Serialization/Deserialization:** Utilizes `ToParcel`, `TryToParcel`, and `FromParcel` traits to convert between raw byte buffers and safe Rust types.

### `zygote-core`
The core logic and state management for the Zygote daemon.

**Core Responsibilities:**
- **Reporting Initialization:** Exposes `init_reporting` to centrally configure and instantiate logging and tracing frameworks.
- **Level Parsing:** Utility functions to translate command-line arguments into typed verbosity levels.
- **Android Telemetry Integration:** Conditionally hooks into the Android tracing system (`atrace_tracing_subscriber`).

### `zygote` (Daemon)
The ultimate server implementation and resulting system binaries (like `zygote_next`).

**Core Responsibilities:**
- **Server Management:** Implements the main `Server` event loop (via `epoll`), handling concurrent client connections and IPC messages.
- **Process Spawning:** Handles the mechanics of safely forking child processes and ensuring clean environments.
- **Species Polymorphism:** Defines the `Species` trait to support multiple execution environments (e.g., `AndroidNative`, `LibApp`).
- **File Descriptor Sanitization:** Rigorously controls file descriptor inheritance using allowlists.

## Development and Testing

Please refer to the `TESTING.md` file for instructions on running the test suite.