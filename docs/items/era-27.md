# Era 27 · Time and power

*Part of the work list. The rules, who is doing what now, and what waits for the user are in [`docs/who-does-what.md`](../who-does-what.md); the lanes in [`docs/lanes.md`](../lanes.md).*


**Done when**: the clock shows local time in the user's time zone, is set from the network, and stays right across a suspend; the machine suspends to RAM and resumes with its programs and devices working; the power button and the lid do what the settings say; a laptop shows its battery; and idle CPUs and devices save power. Whose it is, and when, is for the user to say. Pointed to: K13-K18 (ACPI and power), K24 (fine timers), K72 (Era 13), K83 (Era 15), K150 (Era 23).

What is right: a monotonic clock in nanoseconds from the CPU's counter (`clock.rs`), Unix time read from the RTC at boot (`rtc.rs`); powering off and resetting from user space, the `\_S5` value read from the DSDT without an AML interpreter, all of it tested on the host against a simulated chipset (`acpi` crate); PSCI on AArch64.

What is missing: **no time zones** (`localtime` is UTC, `libc/c/time.c`); no network time; the RTC read once and never written; 10 ms ticks (K24); no AML interpreter (K13), so no power button, battery, lid or suspend (K15, K17, K18); every CPU woken every tick; no device power states; no CPU frequency control.

| # | Item | Size | Notes |
|---|---|---|---|
| J103 | **Time zones**: the tz database (or the zones in use) on the disk, `TZ`, `localtime` that means local | M | First: small and seen by everyone |
| K156 | **Network time**: an NTP (SNTP) client service that sets the clock and writes the RTC | S | After K30 (DHCP) and K33 (UDP) |
| K157 | **CPU power**: idle states (C-states, `mwait`; WFI on AArch64) and frequency (P-states) chosen by a user-space policy | M | With K72 and K24 |
| K158 | **Suspend and resume** end to end: services told before and after, devices saved and restored, the clock corrected | L | Is K18, after K13 and K83 |
| K159 | **Power button, lid and battery** as events a power service hands to whoever asked (the desktop shows them; the settings say what they do) | M | After K13; is K15 and K17 |
