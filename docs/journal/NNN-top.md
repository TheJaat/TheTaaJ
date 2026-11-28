# top

*Journal entry #NNN · K141 (Era 22). Written on TheJat-WSL/ci, by day, with quick checks only. `lanes.md` still had it waiting for `sched`'s K21, CPU time per process, but K21 was done in entry 114.*

## What it shows

```
top: 15 processes, 18 threads, 39 channels; memory 24 MiB used of 511; 2 CPUs
  PID  NAME            CPU%      CPU TIME    MEMORY  THREADS  HANDLES
   48  forever        99.6%       1.485 s      32 KiB        1        3
   43  e1000           0.2%       0.006 s      32 KiB        1        5
   37  display         0.0%       0.012 s    5640 KiB        1        7
   ...
```

Every second, until Ctrl+C. `-n TIMES` shows that many frames and ends; `-d SECONDS` sets the interval. Each frame starts with a form feed, which clears the screen as `clear` does.

- **CPU%** is of one CPU, over the last interval, from the CPU time K21 counts (`cpu_ns`) and the monotonic clock. A program that keeps two CPUs busy shows 200%. So the first frame waits one interval, which is what its percentages are over. The busiest come first, then the biggest.
- **CPU TIME** is all a process has used.
- **MEMORY** is what its account is charged: its program's pages, its stack, the reserved memory it has touched, and the memory objects it made. That is what its memory limit counts.
- **THREADS** are its threads that have not ended; **HANDLES** are the handles it holds.
- Ended processes, still referenced by someone, are left out. `ps` shows them.

## How

- **`abi::ProcessEntry`** grows from 5 words to 8: `memory`, `threads`, `handles`. `PROCESS_LIST` fills them (`ipc.rs`) from the process's account (`Account::used`, in pages), its thread list (`Process::thread_count`, new), and its handle table. Its host test covers the new fields. No system call is new, so there is no number to give at merge (rule 16). Every program that lists processes is built with the new size, as `ps` and `top` are.
- **`top`** (`user/src/bin/top.rs`, on the disk) asks the registry for `system`. That is the handle `ps` uses: it can look at the system and not touch it.
- **The shell** gives it that name: a new list, `SYSTEM_PROGRAMS = ["top"]`, beside `LOOK_PROGRAMS`.

In other lanes' files (as for K136): `ipc.rs` (`ipc`), filling the three fields; `process.rs`, `thread_count()`; `shell.rs`, the new list. The disk's root has one file more, so `ROOT_FILES` is 42, or 30 without the C compiler.

## Checked

- **`top64-test`, new:** a PC with two CPUs and a test disk runs `forever spin &`, then `top -n 2`, then `top -x`.
  - The system's line, the columns, and two frames are there.
  - `forever` is first in both frames, at over 80% of a CPU. It showed 99.6%.
  - `init` and the shell show memory, threads and handles that are not zero.
  - `top -x` says how to use it.
- **Quick checks:**
  - `kernel64-test`;
  - `coverage`: `kernel/abi` went from 88.9% to 89.5%;
  - `unsafe-check`;
  - `boot64-test` and `boot64-uefi-test`: `ps`, and the root's count;
  - `aarch64-test`.
- The full `make test64` is for the night.

## For later

- More than 32 processes: `taaj::process_list` reads at most 32 at a time, and `top` says how many more there are. The kernel takes up to 256.
- A key to sort by memory, and a line for each CPU, once there is a way to read keys while a program runs (`edit` has its window).

## Testing note

| Do | You should see |
|---|---|
| `forever spin &`, then `top` | `forever` first, near 100%; Ctrl+C ends `top` (and `jobs` and `kill` the `forever` job) |
| `top -n 1 -d 2` | One frame, its CPU% over two seconds |

Automated: `make top64-test` (part of `make test64`).
