# Tasks

See b/411209346 to track the high-level development progress for this project.

* [ ] Eliminate all instances of `String` or ensure that they are arena allocated via `alloc-api`.
* [ ] Make `introspection::get_open_file_descriptors` return an iterator
* [ ] Implement a safe wrapper for reading the string from a `libc::dirent` struct.
* [ ] Add support for selecting the species at compile time
* [ ] Make each species a crate feature
* [ ] Enforce maximum lengths for all message argument strings
* [ ] Tune buffer sizes
* [ ] Report the PID of the created process back through the command socket
* [ ] Support ABI query messages
* [ ] Load Zygote server configuration from file (Serde?)
* [X] Create new process group when launching the server
* [ ] Add option to set process name
* [ ] Add an option to set server process priority
* [ ] Add an option to set the child process priority
* [ ] Add new "preload-list" argument to Zygote server
* [ ] Add support for secondary uid/gid to be used during preloading
  * This can be used to prevent static initializers from executing with elevated privileges