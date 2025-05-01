See b/411209346 to track the high-level development progress for this project.

# Process Server

* [X] File descriptor hygiene mechanism
* [ ] IPC format specification
* [ ] Main server loop
* [ ] Handle signals via `signalfd`
* [ ] Standalone Zygote server for host testing
  * In production `init` will construct the socket for the Zygote; in testing we need to either create the socket before launching the test or teach the Zygote to create its own socket when it isn't provided with one
* [ ] Library/CLI for issuing commands to standalone Zygote
* [ ] Add proper logging

## Lessons from Managed Zygote

* Avoid large argument lists for functions
  * See Process.java, ZygoteProcess.java, and com_android_internal_os_Zygote.cpp
* Avoid making the Zygote aware of how many other Zygotes there are on the system and its role as a primary or secondary Zygote
* Encode the Zygote configuration in System Properties; don't detect secondary Zygotes by attempting to connect to their sockets
  * Using the sockets to detect a secondary Zygote means that we can't differentiate a frozen/crashed secondary Zygote from a system configured without them
* Don't create named, special case AppZygotes (e.g. WebView Zygote); instead implement them on top of a more general AppZygote framework
* Use a `signalfd` instead of a regular signal handler; this will allow the signals to be handled as part of the normal poll loop
* Ensure that logging operations don't open sockets unexpectedly

# Native Framework

* [ ] Identify what resources need to be pre-loaded for all native applications

# Chrome Renderer

* [ ] ???

# TODO Items

Smaller work items are documented throughout the codebase using `// TODO` tags.  Examples of these tasks include:

* [ ] Using `strerror_r` to obtain error strings when formatting the `Errno` type.
* [ ] Eliminate all instances of `String` or ensure that they are arena allocated via `alloc-api`.
* [ ] Make `introspection::get_open_file_descriptors` return an iterator
* [ ] Implement a safe wrapper for reading the string from a `libc::dirent` struct.
