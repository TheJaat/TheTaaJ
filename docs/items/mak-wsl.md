# MaK-WSL: real hardware, speed, and the machine itself

*Part of the work list. The rules, who is doing what now, and what waits for the user are in [`docs/who-does-what.md`](../who-does-what.md); the lanes in [`docs/lanes.md`](../lanes.md).*


### Real hardware first (Era 7's milestone)

| # | Item | Size | Notes |
|---|---|---|---|
| K1 | **R7: boot from a USB stick on the real PC** (BIOS and UEFI), write down every failure | M | Needs you at the machine; the flushes and clock from entry 097 make it safer |
| K2 | **USB hubs** (xHCI behind a hub: ports, power, reset through the hub) | M | On real PCs keyboards and sticks are often behind one |
| K3 | **Use TheTaaJ for a day** on the real PC (Era 7's "done when"), noting what hurts | L | |
| K4 | Fix whatever K1-K3 find (likely: ACPI tables, interrupt routing, the screen mode, the disk controller) | L | |
| K5 | **Intel I219 and later onboard network cards** (e1000e) | M | The common onboard card |
| K6 | **Realtek RTL8111/8168** network card | M | The other common one |
| K7 | **x2APIC** and APIC ids above 255 | S | Bigger real machines |
| K8 | NVMe: shutdown notification (CC.SHN), more than one command at a time, 4K blocks | M | |
| K9 | AHCI: NCQ, more than one disk, 4Kn disks | M | |
| K10 | Only one disk request in flight → a queue per disk | M | Speed on real disks |
| K11 | GPT partitions, and finding the TaajFS partition on a real disk | S | |
| K12 | Install TheTaaJ onto the internal disk (an installer program) | M | After K11 |

### ACPI and power

| # | Item | Size | Notes |
|---|---|---|---|
| K13 | **An AML interpreter** (a small one, enough for `_S5`, `_PTS`, `_PRT`) | L | |
| K14 | **Interrupt routing from `_PRT`** for PCI cards | M | After K13 |
| K15 | **The power button** shuts down cleanly (services asked, disks flushed) | S | After K13 |
| K16 | Shutdown asks services to finish instead of killing them; NVMe and AHCI flushed | S | |
| K17 | Battery level (laptops), lid switch | M | Later |
| K18 | Suspend to RAM (S3) | L | Later |

### Speed and timing (KVM shows these first)

| # | Item | Size | Notes |
|---|---|---|---|
| K19 | **Run `make test64` under KVM repeatedly** (say 20 times) and fix every race found | M | **Done (entries 104, 105)**: the tests choose KVM themselves, the typed boot test's typing races; twenty rounds under KVM (`tools/race-hunt.sh`): SIGCHLD before `waitid` returns fixed in the kernel, upper time limits retried; one failure left open as K50 |
| K20 | Process priorities and choosing a thread's CPU (affinity) | M | **Done (entry 124)**, with J80: a priority (-20 to 19) and an affinity per thread, strict priorities and taking a CPU at once, a ceiling per process (init's the highest), SCHED_SET/SCHED_GET; init raises the keyboard-to-screen services to -5. Left: shares (K100), real-time classes (K99), K190 |
| K21 | CPU time per process in `ps` | S | **Done (entry 114)**: the kernel counts each process's CPU time at every switch and tick (`clock::PROCESS_CPU`, one more word in `ProcessEntry`); `ps` has a CPU column; libc's `clock()` and `CLOCK_PROCESS_CPUTIME_ID` give it |
| K190 | A child takes its parent's `nice` - through `posix_spawn` and the shell, as the memory limit travels with the program file (entry 096) - instead of always starting at 0 | S | **Done (entry 130)**: MEMORY_CREATE records its maker's priority, SPAWN_IMAGE gives it to the child, clamped to the child's ceiling (so the shell's -5 is not passed on) |
| K191 | More POSIX timers: several per process (`timer_create` makes one now), SIGEV_THREAD and SIGEV_NONE, `timer_getoverrun` counting; fine timeouts for futexes and OBJECT_WAIT_UNTIL (`pthread_cond_timedwait`, `poll`), which still wait to the tick | M | After K24 (entry 128) |
| K192 | **Waking a thread on another CPU is slower under KVM** since the one-shot timers: two CPUs' wake-to-run median 46-109 us against 4 on `main`; not under TCG. Find why (the host waking a halted vCPU; halt polling; the APIC timer's mode) | S | Found in entry 128, bug 5 |
| K193 | **A lock per CPU's run queue** instead of one for all (K68's other half): a switch takes only its CPU's lock, a thread moving between CPUs takes two, in order; rule 1 of threads.rs (the lock held across a switch) rewritten for it | M | After K68 part 1 (entry 129) |
| K22 | A transmit-interrupt serial writer (AArch64 output capped at ~3200 bytes/s) | S | |
| K23 | The faster boots: measure boot time on KVM and on the real PC, cut the slow parts | S | |
| K24 | Timer: alarms and sleeps finer than a 10 ms tick (one-shot timer, `setitimer`, `timer_create`) | M | **Done (entry 128)**, with K96 and J53: one-shot timers (each CPU's set for its next tick or its first sleeper), deadlines in nanoseconds, SLEEP_UNTIL, ALARM's interval; `nanosleep` to the nanosecond, `clock_nanosleep`, `setitimer`/`getitimer`, `timer_*` (one timer). Left: K191, K192 |

### Security

K27 (the IOMMU) is in Era 15. Era 18 (Security: hardening, trust and people) has the rest, and points to these.

| # | Item | Size | Notes |
|---|---|---|---|
| K25 | **ASLR**: random program, stack and mmap addresses (after J2) | M | |
| K26 | Stack protector and stack probes in C programs | S | |
| K28 | Only a chosen program may switch the machine off (not anyone with init's channel) | S | |
| K29 | A random TCP initial sequence number (after J2) | S | |

### Network (Era 12's ground)

The protocols are careful and tested on the host (the `net` crate: ARP, IPv4, ICMP, UDP, a TCP with RFC 9293's states, timeouts from the round trip and congestion control); what is missing is reach (one fixed address, no DNS, TCP only from C) and scale (few sockets, many copies, never measured). K30 comes first: without it the real PC has no network.

| # | Item | Size | Notes |
|---|---|---|---|
| K30 | **DHCP** | S | Needed on any real network |
| K31 | **DNS** (`getaddrinfo`) | M | |
| K32 | Link state noticed (cable unplugged) | S | |
| K33 | Loopback (`127.0.0.1`), UDP and non-blocking sockets from C | M | |
| K34 | IP fragments, ICMP errors to programs | S | |
| K35 | TCP: window scaling, SACK, timestamps, keepalive; a lossy-network boot test | M | |
| K36 | IPv6 | L | Later |
| K37 | TLS 1.3 with its cryptography | L | After J2; its cryptography from Era 29's K176 |
| K38 | HTTP server: several connections, keep-alive | S | |
| K49 | Socket readiness for `poll`: the network service tells a socket's channel when bytes arrive, so poll can wait for it (entry 100 treats a socket as always ready) | S | After K33 |
| K53 | **A network benchmark**: TCP throughput both ways, connections per second, a ping's round trip; virtio and e1000, KVM and emulated; the numbers kept in the journal | S | First, for the speed items below |
| K54 | **Frames through shared rings** between the card's driver and the network service, and between the service and a program's socket, instead of messages: a 1514-byte frame is two 1 KiB messages today, each copied twice | M | After K53; with K63 |
| K55 | **More sockets**: 16 connections, 10 sockets, 4 listeners and a backlog of 4 are fixed because the service waits on all of them in one `object_wait` (`WAIT_HANDLES_MAX`); a wait over a set that grows, or one channel per program, lifts it | M | A web server or a parallel download needs more |
| K56 | **Which programs may listen on which ports**, and a simple packet filter | S | Today ports are first come |
| K57 | **A routing table**: more than one card and network | M | Later |
| K58 | **Checksum and segmentation offload** on virtio and e1000 | M | After K53 |
| K59 | **Sockets that survive a restart of the network service**, or at least a clear "the network went away" to every program | M | Today they are all gone, and programs learn it by their channels closing |

### Desktop

Era 19 (Desktop: applications people can use daily) builds on these, and points to them.

| # | Item | Size | Notes |
|---|---|---|---|
| K39 | The screen at the real PC's native resolution (UEFI GOP and VESA modes chosen well) | S | |
| K40 | Windows: resize, minimise, a task bar | M | |
| K41 | Copy and paste between windows | S | |
| K42 | Key up/down and lone Shift/Control reach windows | S | |
| K43 | A font with more than ASCII (UTF-8 drawn, not '?') | M | |
| K44 | The terminal as its own program; pseudo-terminals | M | |
| K45 | Job control: process groups, Ctrl+Z, `fg`, `bg`, SIGTSTP/SIGCONT | M | |
| K46 | Scroll wheel, more mouse buttons, pointer acceleration | S | |

### Sound (Era 10's ground)

| # | Item | Size | Notes |
|---|---|---|---|
| K47 | Intel HD Audio driver and an audio service | L | Testable in QEMU (`-device intel-hda`) |
| K48 | WAV playback, then a microphone recorded to a file | M | |
| K50 | **`bigmem64-test` once did not finish its steps** under KVM (race hunt round 6 of 20, entry 105); not seen in 5 runs under full CPU load; its log was lost | S | Likeliest: host memory and CPU both short with 12 test machines on 11 GiB. `tools/race-hunt.sh` now keeps a failed round's files, so run it again and read them |
| K51 | **The BIOS loader is slow emulated**: 14 of an 18 s boot pass before the kernel's first byte (idle host, TCG; under KVM well under a second) - stage2 reads the kernel through `int 13h`, back to real mode and out again for each read | S | Measured in entry 105. Read more per `int 13h` call, or stay in real mode for the whole read. The bootloader is MaK-WSL's (rule 7) |
| K52 | **`aarch64-tcp-test` expects no segment for a closed connection**: once, in a full emulated `test64` on a busy host, `netinfo` counted 1 answered "no such connection" (41 segments sent again; alone, 0 of 3 runs, with 1-4 sent again) | S | Likely a late retransmission from the host after TheTaaJ closed passively (correct TCP answers it). Check what segment it was, then either allow it or keep the connection longer (entry 105) |
