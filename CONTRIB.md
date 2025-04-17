This document contains details and notes on the development process for the Native Zygote project.

# Lessons from Managed Zygote

* Avoid large argument lists for functions
  * See Process.java, ZygoteProcess.java, and com_android_internal_os_Zygote.cpp
* Avoid making the Zygote aware of how many other Zygotes there are on the system and its role as a primary or secondary Zygote
* Encode the Zygote configuration in System Properties; don't detect secondary Zygotes by attempting to connect to their sockets
  * Using the sockets to detect a secondary Zygote means that we can't differentiate a frozen/crashed secondary Zygote from a system configured without them
* Don't create named, special case AppZygotes (e.g. WebView Zygote); instead implement them on top of a more general AppZygote framework
* Use a `signalfd` instead of a regular signal handler; this will allow the signals to be handled as part of the normal poll loop
* Ensure that logging operations don't open sockets unexpectedly