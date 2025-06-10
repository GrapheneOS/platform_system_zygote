# Tasks

See b/411209346 to track the high-level development progress for this project.

* [ ] Eliminate all instances of `String` or ensure that they are arena allocated via `alloc-api`.
* [ ] Make `introspection::get_open_file_descriptors` return an iterator
* [ ] Implement a safe wrapper for reading the string from a `libc::dirent` struct.
* [ ] Add support for selecting the species at compile time
* [ ] Make each species a crate feature
* [ ] Enforce maximum lengths for all message argument strings
* [ ] Tune buffer sizes
* [X] Report the PID of the created process back through the command socket
* [X] Support ABI query messages
* [ ] Load Zygote server configuration from file (Serde?)
* [X] Create new process group when launching the server
* [X] Add option to set process name
* [X] Add an option to set the child process priority
* [ ] Add new "preload-list" argument to Zygote server
* [X] Add support for secondary uid/gid to be used during preloading
* [ ] Take `cgroup` as argument in spawn messages
* [X] Purge memory allocator after preloading
* [ ] Add an `UnknownMessageType` response message
* [ ] Add an `UnsupportedMessageType` response message
* [ ] De-duplicate code between `zygote_cli.rs` and `zygote_launcher.rs`
* [ ] Take `priority-initial` and `priority-final` arguments in Spawn messages