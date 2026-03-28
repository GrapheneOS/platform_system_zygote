@./README.md

# Zygote Development Guidelines

This document provides context and guidelines for agents contributing to the development of the Zygote project.

## `zygote-sys`
- **Strict Isolation of `unsafe`:** Prevent `unsafe` code from leaking into the core logic. Wrap all `libc` or C calls in safe Rust functions.
- **Idiomatic Error Handling:** Wrap integer status codes (like `-1`) to return a `Result` using `check_failure` and `check_success` helpers.
- **Safe Memory Management:** Use `AsCStr` for C-strings. Use `CStringBuffer` or `ArrayVec` for stack-allocated buffers.
- **Android Specifics:** Isolate Android-specific APIs in `android.rs` with `#[cfg(target_os = "android")]`.
- **Procfs Introspection:** Handle potential file descriptor leaks carefully (e.g., using `ProcFdIterator`).

## `zygote-proc-macros`
- **Code Generation:** Prefer `#[derive(MarshalParcel, UnmarshalParcel)]` over manual byte-packing logic.
- **Attribute Support:** Leverage existing macro attributes (`#[flatten]`, `#[unmarshal_from]`, `#[error_type]`).
- **Extending Macros:** If unsupported, update `syn` parsing securely in `src/marshal.rs` and `src/unmarshal.rs` using the `quote!` macro.

## `zygote-messages`
- **Schema First:** The source of truth is `schemas/messages.fbs`. Modify it first for new parameters/commands.
- **Safe Rust Wrappers:** Create safe, idiomatic Rust structs in `src/lib.rs` mapping to FlatBuffer variants instead of exposing raw types.
- **Validation at the Edge:** All data validation (ranges, buffer sizes) should happen here before passing to `zygote-core`.
- **Cross-Language Support:** Avoid breaking changes to the `.fbs` schema; prefer adding optional fields.

## `zygote-core`
- **Centralized Telemetry:** Route all logging/tracing initialization through `zygote_core::init_reporting`.
- **Initialization Safety:** `init_reporting` uses atomic flags (`REPORTING_INITIALIZED`). Panic immediately if re-initialized.
- **Platform Neutrality:** Keep core utility functions generic. Use `#[cfg(target_os = "android")]` only when necessary.

## `zygote`
- **The Species Abstraction:** Use the `Species` trait for different process "kinds". Delegate to the active `Species` rather than hardcoding.
- **Zero-Leak Policy:** File descriptor leaks are critical vulnerabilities. `file_descriptors.rs` enforces an allowlist. Ensure any newly opened files are registered, marked close-on-exec, or added to the allowlist.
- **Robust Event Loop:** The `epoll`-based `Server` must not block the main thread. Ensure all client socket I/O is non-blocking.
- **Sanitization Before Execution:** Before a child invokes `execve`, it must pass strict sanitization (`child_process.rs` and `Species` implementation).