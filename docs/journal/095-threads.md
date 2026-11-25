# Threads in a program: pthreads for real, on every CPU

*Journal entry #95 · Era 9, step R2. A program on TheTaaJ had one thread: the kernel has run threads on every CPU since entry 046, but a program couldn't start a second one, and `pthread_create` said EAGAIN. Now it can:
- the kernel gives a process many threads, each with its own thread pointer, and futexes to sleep on;
- the C library has real pthreads: threads, mutexes, conditions, rwlocks, once, keys, errno for each thread, `_Thread_local`;
- its file table, stdio and the heap are safe to use from many threads at once;
- `std::thread` works in C++, and Rust programs have `taaj::thread::spawn` and `taaj::sync::Mutex`.

The same work done by 4 threads takes 0.07 s instead of 0.25 s.*

## Why

The user asked for multithreading next. It is R2 of Era 9 (`docs/era-9-rustc.md`): rustc runs its compiler on a spawned thread and uses thread-locals throughout, so the real rustc can't run on TheTaaJ without it. It's useful well before that: LLVM's tools, C++ programs and anything that waits for two things at once.

## How it fits together

```
 program                                    kernel
 ───────────────────────────────────        ─────────────────────────────────────
 pthread_create(f, arg)
   stack = memory object (1 MiB)
   record = Thread { self, errno, ... }
   THREAD_CREATE(begin, stack, record, ──►  process.threads += new thread, same
                 pointer = record)            address space and handles; FS base /
                                              TPIDR_EL0 = pointer, kept per thread
 pthread_mutex_lock: CAS 0 -> my id
   held? mark WAITING, FUTEX_WAIT ───────►  read the word and queue, as one step,
 pthread_mutex_unlock: swap 0                 under one lock: no wake can be missed
   was WAITING? FUTEX_WAKE(1) ───────────►  first waiter at (pid, address) woken
 thread ends: THREAD_EXIT(&running) ─────►  *running = 0, wake all waiting there;
 pthread_join: FUTEX_WAIT(&running)           the last thread ends the process
 exit(7) from any thread ────────────────►  first exit code wins; every other
                                              thread stops on its way to user mode
```

**The kernel** (TheKernel):
- Five system calls (`abi`):
  - THREAD_CREATE (26): `(entry, stack, argument, pointer)`;
  - THREAD_EXIT (27): `(clear)`;
  - THREAD_POINTER (28);
  - FUTEX_WAIT (29) and FUTEX_WAKE (30).
- A process keeps the list of its threads (`process.threads`), at most `THREADS_MAX` = 64. A kernel stack is 60 KiB, and its address range stays the kernel's once made, so the limit bounds what one program can take that way.
- **The thread pointer** is part of each thread: `threads::schedule` loads the next thread's into FS's base (an MSR) or TPIDR_EL0 on every switch. On AArch64 a program can write TPIDR_EL0 itself, so the leaving thread's value is saved first. On x86_64 it can't (FSGSBASE is off), so the record is always right.
- **Futexes** (`futex.rs`): there is no kernel object per address. A waiter is a record `(pid, address, thread, woken)` in one list and sleeps in one of 64 wait queues, chosen by hashing its address. FUTEX_WAIT reads the user word and joins the list under the list's lock. A waker changes the word in user mode first, then takes the same lock. So a wake can't fall between the check and the sleep. The semantics are Linux's.
- **Ending**: any thread's EXIT, or a fault in any thread, ends the process. `end_current` sets the exit code with a compare-and-swap, so when two threads exit at the same moment the first code wins and only one of them does the cleanup. The others get a kill request and are woken. Each stops at the next return to user mode: from a system call, or from an interrupt that came from user mode. That one rule also stops a thread spinning in user mode, at its next timer tick. The last THREAD_EXIT ends the process with 0.

**The Rust side of programs** (`user/src`):
- `sync.rs`: `Lock`, the classic three-state futex lock (0 free, 1 held, 2 held and maybe waited for), and `Mutex<T>` with a guard. The heap (`allocator.rs`) now takes this lock instead of assuming one thread.
- `thread.rs`: `spawn(closure)` and `join()`, for Rust programs.

**The C library** (`libc/src/threads.rs`, new, replacing `libc/c/pthread.c` and `libc/c/emutls.c`, which pretended for one thread):
- **A thread is its `Thread` record, and its thread pointer points at that record.** On x86_64 the record's first word holds its own address, so `mov rax, fs:0` finds it, which is what the x86_64 ELF ABI asks for. On AArch64 TPIDR_EL0 holds it. `pthread_t` is the record's address.
- **Mutexes** are 8 bytes, `[kind | depth << 8][owner id | WAITING]`: a free one is taken with one compare-and-swap. Conditions are a sequence number that every signal moves on. Rwlocks are a count plus a sequence number. `pthread_once` uses three states.
- **join** sleeps on the record's `running` word, which the kernel clears at THREAD_EXIT. A detached thread can't unmap the stack it is standing on, so it goes on a list, and the next `pthread_create` or `pthread_detach` frees whatever on the list has ended.
- **errno** is `(*__errno_location())`, a field of the running thread's record. **Keys** run their destructors at the thread's end, up to 4 rounds. **`_Thread_local`** and C++'s `thread_local` work through `-femulated-tls`: `__emutls_get_address` gives each thread its own copy, made from the starting value on first use.
- **The last `pthread_exit` calls `exit(0)`**, so atexit functions run and stdio is flushed, which the kernel ending the process wouldn't do.

**Making the rest of libc safe for threads:**
- **The descriptor table** (`descriptors.rs`) was a spin lock held across every `read` and `write`. Now:
  - the table is behind a futex lock and is held only to look something up;
  - `Open` is `Clone`, and what a descriptor stands for is an `Arc`: a file with its own `Mutex`, a socket, a pipe end;
  - `read` and `write` take a copy of the entry, let the table go, and then wait, holding only that object's own lock.

  Before, a thread waiting to read a pipe kept every other thread out of the table, including the one about to write to that pipe, so both waited forever. One of the mutation experiments below puts that back and watches the program hang.
- **Standard input** has a lock of its own. The **file service** and **network service** clients are shared as `Arc`. Each request makes its own reply channel, so two threads can use one client at once.
- **Sockets** (`sockets.rs`): `connect`, `listen` and `accept` wait holding only the socket's lock.
- **Pipes** (`pipes.rs`): a read end's buffered bytes have a lock; its channel handle doesn't need one.
- **Children** (`spawn.rs`): the lists of unwaited children and of popen's commands are `Mutex`es.
- **stdio** (`stdio.c`): every `FILE` has a recursive pthread mutex, held for the whole of each call. Two threads' `printf` never mix inside a line. `puts` holds it across the string and its new line. `flockfile`, `ftrylockfile`, `funlockfile` and the `*_unlocked` calls are there too. Only two places take two locks, always in the same order: the list of open streams before a stream, and stdin before stdout. `fclose` lets its stream go before it takes the list.
- **`dup` of a socket** works now: the copy shares the socket, unread bytes and all. Before, it was EOPNOTSUPP.

## Design decisions

| Decision | Chosen | Rejected, and why |
|---|---|---|
| What the thread pointer points at | The thread's own record, its first word its own address | The ELF TLS layout (a static block below the pointer, `PT_TLS` copied per thread): needed for `__thread` without emulation, and by Rust's std later (R3). Emulated TLS works with what clang already does for C++ here, and the record layout leaves room for the real thing |
| Futex waiters | One list plus 64 hashed wait queues, a `woken` flag per waiter | A wait queue per address, made on demand: memory the kernel would have to allocate on a user's behalf, and free at the right moment |
| Futex keys | (process, virtual address) | Physical address, for futexes across processes: TheTaaJ has no memory shared between processes for locks yet |
| How a process ends with many threads | A kill request; each thread stops on its next way to user mode | Stopping other CPUs at once with an interrupt: more machinery, and a thread in the kernel holding a lock must not stop there anyway |
| libc's table and blocking I/O | Take what the descriptor stands for, let the table go, wait holding only that object's lock | Keep one big lock: the read-waits-for-write deadlock above. A lock per descriptor slot: `dup2` and `close` would have to take two in order |
| stdio | A recursive lock per FILE | One lock for all of stdio: a thread waiting on a pipe through `fgets` would stop every other thread's `printf` |
| pthread types | The sizes they had with one thread | New sizes: code compiled against the old header (libc++, LLVM's objects) would break silently; 8 bytes is enough for a futex mutex with an owner and a depth |
| The last `pthread_exit` | `exit(0)` | Letting the kernel end the process: atexit and stdio's flush would not run, and output in a buffer would be lost (the `threads last` check catches it) |
| A thread's stack | A 1 MiB memory object (attributes may say 16 KiB to any size) | 8 MiB like Linux: memory objects are made whole, not page by page as touched, so it is memory taken, not reserved |

## Bugs

- **errno, and the host test that renames symbols.** `errno` became `(*__errno_location())`. The host test of stdio.c renames every symbol it uses (`host_names`), so it can live beside glibc's stdio. `errno` was on the list, `__errno_location` wasn't. So stdio.c on the host read and wrote *glibc's* errno, and 7 of 124 checks failed. The fix renames it in all three tests that rename (stdio, stdlib, unix) and gives each test its own `taaj___errno_location`.
- **The speed run measured nothing.** My first `threads speed` said "1 thread 0.00 s". Clang saw that the single-thread result was unused and deleted the loop. The work is now a xorshift whose answer goes to a `volatile`, and I checked the disassembly: 5 instructions a step, all there.
- **A check that said threads were impossible.** `clib.c` (entry 073) checked that `pthread_create` failed with EAGAIN: "no second thread". The first `make test64` of this step failed in the three places that run clib: the BIOS and UEFI boots, and `tcc64-test`, where TCC compiles it on TheTaaJ. cxxlib had the same assumption, and I'd changed that one; I missed clib's. Now it makes a second thread and joins it, with the thread's answer. The second run found one more leftover: `tcc-test.sh` compares every object TCC builds on TheTaaJ with the host's, and its list still had `pthread.o` and `emutls.o`, which this step removed. (That run was also longer than any before it: changing pthread.h rebuilt LLVM for TheTaaJ, 2,163 files.)
- **The C test missed a broken heap lock.** See the mutation experiments: making `Lock::lock` do nothing didn't fail threads.c, because none of its checks had threads using malloc at once. I added one (4 threads, 20,000 mallocs and frees each, every block checked for its owner's mark), and then it did.

## How I checked it

**New test: `make thread64-test` and `make aarch64-thread-test`** (`thread-test.sh`, 4 CPUs). TheTaaJ runs:
- `with files threads`: **30 checks**:
  - 16 threads joined, each with its own return value; `pthread_exit`'s value;
  - 300 threads made and joined one after another;
  - 8 threads adding 100,000 each under one mutex: exactly 800,000;
  - 4 threads mallocing at once;
  - trylock EBUSY, unlock by a non-owner EPERM, relock EDEADLK, a recursive mutex's count;
  - 2 producers and 3 consumers through a 16-slot queue: all 20,000 items taken once;
  - `pthread_cond_timedwait` timing out after its time;
  - a rwlock whose readers never see half a write;
  - `pthread_once` from 8 threads; `_Thread_local`, errno and a key's value, each thread its own; the key's destructor once per thread;
  - 200 detached threads, more than a process may have at once, so ended ones must be given back;
  - a write to a pipe that another thread is already waiting to read;
  - 4 threads `fprintf`ing 200 lines each to one FILE: 800 whole lines, each thread's in order.
- `threads exit`: `exit(7)` from a thread while others wait on a mutex, a condition and a pipe, and one spins. The program ends with 7.
- `threads last`: main calls `pthread_exit`. The other thread's `printf`, with no new line and no `fflush`, still comes out, and the program ends with 0.
- `threads speed`: **1 thread 0.25 s, 4 threads 0.07 s (3.6 times as fast)**, on both architectures.
- `threadcheck` (Rust): 4 threads allocating through the one heap and adding under a `Mutex`.

**cxxlib**: 62 → **66 checks** on both architectures. The old "std::thread can't start a second thread" check is gone. In its place:
- 4 `std::thread`s adding under a `std::mutex`;
- `thread_local` per thread;
- `call_once` from 4 threads;
- a `condition_variable` notified from another thread;
- an exception thrown and caught on another thread's stack;
- `std::async`.

The same file on the host, against glibc and libstdc++, also gives 66 checks.

**Host tests**: `make libc-test` passes. stdio has 124 → 126 checks; the two new ones say that every stream lock a call took, it gave back, and that the open-streams lock was never taken twice. The stand-ins count the locks.

**Mutation experiments**, each undone:

| What I broke | What happened |
|---|---|
| `vfprintf` without the stream's lock | 2 checks failed: "800 whole lines" and "each thread's lines in its own order" |
| `read` from a pipe waits with the descriptor table held (as before this step) | The program hung: the steps never finished |
| The scheduler doesn't load the next thread's pointer | `threads` killed by a page fault in each run (its record looked up through FS) |
| `taaj::sync::Lock::lock` returns at once | `threadcheck` killed by a page fault (a corrupted heap). threads.c passed all 29 checks it had then, which is how I found it had no check for malloc from many threads. With the new check: killed by a page fault too |

`make test64` passed on the third run, in 31 min 24 s (libc++ rebuilt first). The first two failed on two places that still assumed one thread (see Bugs).

## What is still missing

- **ELF TLS.** `__thread` / `_Thread_local` work only with `-femulated-tls`. Code compiled without it, as Rust's std will be, expects a TLS block at the thread pointer. That belongs to R3.
- **No guard page** below a thread's stack: libc makes none, so whether an overflow faults or writes into whatever is mapped below depends on where the kernel placed the mappings.
- **No signals per thread**, `pthread_kill` or `pthread_cancel`.
- **No futexes between processes**, so no process-shared mutexes.
- **64 threads per process**, and every thread of every program has the same priority. No affinity, no `sched_setscheduler`.
- **A descriptor closed while another thread waits on it**: the waiting `read` keeps what it took out of the table and goes on until data or the end arrives. POSIX leaves this undefined; Linux behaves much the same.
- **Rust programs** have `spawn`/`join` and `Mutex`, not std's `thread` and `sync`: std comes in R3.

## What I learned

- **A lock held while waiting is a lock everyone waits for.** The old table lock was harmless with one thread, and a deadlock with two the moment one thread read a pipe. The rule I now follow everywhere in libc: look up under the lock, let it go, then block.
- **The order of two locks is a promise across the whole program.** stdio takes two locks together in exactly two places, and they are written down at the top of the file, because a reversed pair anywhere is a deadlock that tests rarely catch.
- **A futex is only "check and sleep, atomically".** All the rest (owner, recursion, fairness) lives in user mode. The kernel side is 116 lines, comments included.
- **Mutation testing finds missing tests, not only missing code.** The test passed with a broken heap lock until I wrote the check that would notice.

**Rust, as used here:**
- **`Rc` → `Arc`, `RefCell` → `Mutex`.** `Rc` counts references with plain additions, and `RefCell` panics on a second borrow. Both are only right on one thread, and the compiler enforces that: `Rc` is not `Send`. `Arc` counts with atomic instructions, and `Mutex` makes the second user wait instead of panicking.
- **`Send` and `Sync`.** `Mutex<T>` is `Sync` only if `T: Send` (`unsafe impl<T: Send> Sync for Mutex<T>`): the lock lets threads take turns with a value, so the value must be allowed to move between threads. Raw pointers are neither, which is why the list of thread records is wrapped in a struct with its own `unsafe impl Send`.
- **A guard that unlocks when dropped.** `counter.lock()` returns a `MutexGuard`. Through `Deref`/`DerefMut` it behaves like `&mut T`, and its `Drop` unlocks, so no path out of a function, `?` and panics included, can forget to.
- **`#[derive(Clone)]` on an enum of `Arc`s** is `dup`: cloning `Open::Socket(arc)` makes a second descriptor that shares the same socket.
- **`with(|state| state.entry(fd))`**: a closure keeps the lock's lifetime visible in the code. The lock is held exactly while the closure runs, and what the closure returns is a copy that outlives it.
- **Atomics and `Ordering`.** `Acquire` when taking a lock, `Release` when giving it back: whatever the last holder wrote is visible to the next. `compare_exchange(FREE, HELD, ...)` is "take it if it is free" as one indivisible step.
- **`Box<dyn FnOnce() + Send + 'static>`** is what `spawn` stores. `FnOnce` because the closure runs once and may consume what it captured. `Send` because it runs on another thread. `'static` because it may outlive the function that spawned it.
- **`-> !`**: `thread_exit` never returns, and the type says so. Code after it is known to be unreachable, and a function that ends with it needs no return value.

## Testing note

**1. The PC**

```
make run-disk
make run64
```

Close QEMU before `make run-disk`. It puts `threads` and `threadcheck` on the disk.

| Type | You should see |
|---|---|
| `with files threads` | after a few seconds, `threads: 30 checks, all passed` |
| `threads speed` | `threads: the same work: 1 thread 0.xx s, 4 threads 0.xx s (N times as fast)`, with N around 3 to 4 on 4 CPUs |
| `threads exit` | `threads: exit(7) from a thread, with others waiting and one spinning`, then `threads exited with code 7` |
| `threadcheck` | `threadcheck: 4 threads, 20000 additions and as many blocks, all passed` |
| `cxxlib` | `c++: 66 checks, all passed` |

**2. AArch64**

```
make run-aarch64-disk
make run-aarch64
```

The same as part 1.

**3. The tests by themselves**

```
make thread64-test
make aarch64-thread-test
make test64
```

| Look for | Meaning |
|---|---|
| `threads on TheTaaJ (x86_64): 30 checks of pthreads on 4 CPUs passed; ...` | every check, exit from a thread, the last thread, Rust threads; the same for aarch64 |
| `test64: all passed` | every part |

Automated: make thread64-test, make aarch64-thread-test, make test64
