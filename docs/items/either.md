# Either machine: whoever is free

*Part of the work list. The rules, who is doing what now, and what waits for the user are in [`docs/who-does-what.md`](../who-does-what.md); the lanes in [`docs/lanes.md`](../lanes.md).*


| # | Item | Size | Notes |
|---|---|---|---|
| E1 | `fsck` for TaajFS: check and repair a damaged disk | M | |
| E2 | Services that crash on one "poisoned" request again and again: give up on that request | S | Era 23's K148 |
| E3 | A watchdog for the shell, init and the registry | S | Era 23's K145 and K147 |
| E4 | A redirected file written in one commit, not one per message | S | Part of J20 |
| E5 | Swap to a disk file | L | Later; with Era 16's K89 |
| E6 | TheKernel standalone (not needing `heap`, `virtio`, `console`, `user/` from the parent) | M | |
| E7 | The 32-bit kernel booted again, or retired officially | S | Decide with the user |
| E8 | Users' documentation: a "how to use TheTaaJ" page for the website | S | **Done (entry 145)**: `docs/using-taaj.md`, linked from the README - what is on `main`; kept up to date by the steps that change what a user sees |
