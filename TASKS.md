# Tasks

See b/411209346 to track the high-level development progress for this project.

* [ ] Eliminate all instances of `String` or ensure that they are arena allocated via `alloc-api`.
* [ ] Make `introspection::get_open_file_descriptors` return an iterator
* [ ] Implement a safe wrapper for reading the string from a `libc::dirent` struct.
* [ ] Add support for selecting the species at compile time
