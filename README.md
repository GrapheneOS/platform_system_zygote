This project implements a process-server architecture similar to Android's Zygote.

This server is responsible for:
* Preloading/initializing shared resources
* Maintaining file descriptor hygiene
* Forking new processes
* Transitioning new processes to appropriate security contexts