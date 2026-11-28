# Era 23 · Services, recovery and configuration

*Part of the work list. The rules, who is doing what now, and what waits for the user are in [`docs/who-does-what.md`](../who-does-what.md); the lanes in [`docs/lanes.md`](../lanes.md).*


**Done when**: every service - the registry and init's own work included - can crash and come back with its clients carrying on; a service that was restarted gets back the state it had (open files, sockets, windows) or its clients are told exactly what was lost; services are described in configuration files, not in init's code, and a new service is added without changing init; and the system shuts down by asking every service to finish, in order. Whose it is, and when, is for the user to say. Pointed to: E2 (poisoned requests), E3 (a watchdog for the shell, init and the registry), K16 (shutdown that asks), K59 (Era 12), K76 (Era 15), K105 (Era 18).

What is right: init is a supervisor. It wires the services with channels, gives each only the handles it needs and fewer rights than it holds, keeps the originals so a restarted driver gets the same channels and its clients never notice, restarts a service that ends, gives up after 5 restarts in 30 seconds, and keeps a watchdog for `fs` and the disk driver (a request a second; no answer and it is killed and restarted). The registry gives each client only the names meant for it. A dead service is noticed by its clients through `PeerClosed`.

What is missing: **init itself and the registry** are restarted by nothing (if init dies, the system has no supervisor); only `fs` and the disk driver are watched; **a restarted service starts empty** - `net` forgets every socket, `display` every window, `fs` its streams - and each client finds out its own way; **everything is in init's code**: which services exist, their order, their handles by number (`Grant` lists with hand-numbered slots), their names in the registry; there is no configuration at all - `ramdisk-src/config.cfg` (`timer.hz=1000`) is a leftover the 64-bit system never reads; no way to stop, start or restart a service by hand; shutdown kills rather than asks (K16).

| # | Item | Size | Notes |
|---|---|---|---|
| K143 | **Services described in files**: each service's program, the handles it gets, the names it serves and may use, its restart policy and watchdog, read by init at boot - replacing init's tables; the unused `config.cfg` removed | M | First. With K105 (Era 18), which describes programs the same way |
| K144 | **`service` commands**: list, stop, start, restart, status - through init, with a capability for it | S | After K143 |
| K145 | **A watchdog for every service that answers requests**, declared in its configuration | S | After K143; covers E3's shell and registry |
| K146 | **State that survives a restart**: a service may keep what its clients depend on in a memory object init holds for it (as init keeps a driver's DMA memory) and find it again when it starts; for those that can't, a clear "your session was lost" to each client | L | `fs`, `net`, `display` first |
| K186 | **Files right after the disk driver is restarted**: when init stopped `blk` as hung (2 s) during `times` - a 301 MiB file being written - the program's file times came out wrong ("its time is a time of day", "is newer, to the nanosecond") and one file was missing (errno 2). Seen on `main` under load (entry 117); `make test64` no longer provokes it (its disks are `cache=unsafe`), so it needs a test that stops the driver mid-write on purpose | M | Found by TheJat-WSL/ci; for `services` (or `taajfs`) |
| K147 | **init and the registry restartable**: a small, fixed first process that only restarts init, or init and the registry each watching the other | M | The last supervisor must be the simplest |
| K148 | **A request that crashes a service again and again is refused** after the second time, instead of crashing it forever | S | Is E2 |
| K149 | **Shutdown in order**: every service asked to finish (disks flushed, files committed) in the reverse of the start order, with a time limit | S | Is K16 |
| K150 | **Settings**: one place for the machine's settings (keyboard layout, time zone, screen, network), a service that keeps them and tells those who asked when one changes | M | For Era 19's settings application |
