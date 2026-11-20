# The First C Program: What Has to Exist Before `main`

*Journal entry #36 · a C file compiled on my Linux machine by a compiler that has never heard of TheTaaJ, linked with a libc written in Rust, copied to the disk, and run from the shell*

Everything that has run on this system so far was written in Rust, in this repository, against the system's own runtime. The milestone for this era says something else: *I compile a C program on my host, copy it to the disk, and run it from the shell.* This entry is the first half of that sentence working, for the smallest C program worth the name.

```
taaj:/> chello one "two words"
Hello from C
argc 3
argv[1] one
argv[2] two words
taaj:/> chello fail
Hello from C
argc 2
argv[1] fail
chello exited with code 3
taaj:/> chello x | wc
3 lines, 7 words, 30 bytes
```

It is the first of seven steps towards a usable C library. The plan, so the rest make sense: (1) **the toolchain and crt0 — this entry**; (2) standard error; (3) memory and strings; (4) `printf`; (5) file descriptors; (6) `stdio` on files; (7) a real program and the recipe written down.

---

## What a C program assumes

```c
int main(int argc, char **argv)
{
    write(1, "Hello from C\n", 13);
    return 0;
}
```

Three assumptions are hiding in those four lines, and none of them is part of the C language:

1. **Somebody calls `main`**, with a count and an array of strings.
2. **`write` exists**, and descriptor `1` means "my output".
3. **Returning from `main` ends the program**, and the number goes somewhere.

On Linux a library called *libc* provides all three, and the compiler links it in without being asked. For TheTaaJ there is no such library — the compiler has no idea this system exists. Making one is the job. A C library is not "C's standard functions"; it is **the adapter between what C programs assume and what one particular operating system actually offers**. Every OS has its own.

## Three questions, answered in order

### 1. What does the compiler produce, with no OS to target?

```
clang --target=x86_64-unknown-none-elf -ffreestanding -nostdlibinc -isystem libc/include ...
```

- `--target=x86_64-unknown-none-elf`: machine code for a 64-bit x86, in the ELF format, for **no** operating system. The compiler makes an object file — functions, not a program.
- `-ffreestanding`: do not assume a libc. Without it Clang may turn a loop into a call to `memset`, or assume `main` is special.
- `-nostdlibinc`: do not look in `/usr/include`. My Linux machine's `unistd.h` describes Linux. But *keep* the compiler's own headers — `stddef.h`, `stdint.h`, `stdarg.h` — which describe the machine and the compiler, not an OS. (`-nostdinc` would drop those too; the difference between the two flags is exactly the difference between "the OS's headers" and "the compiler's".)
- `-isystem libc/include`: look here instead. These are TheTaaJ's headers.

And one that matters more than it looks:

- `-mgeneral-regs-only`: use no floating-point or SSE registers. My kernel does not save them when it switches between programs (it was never needed — the Rust target I use avoids them too). A C program that used them would have its numbers silently changed by whichever program ran in between.

  I expected this flag to make `double` a compile error, and wrote so in the first draft of this entry. It does not — I checked, and was wrong. Clang compiles `x / 2` on a `double` into a *call*: `__divdf3`, a function that does the division with integer instructions. This is **soft float** — what machines without a floating-point unit have always done. And the functions it calls are already in `libc.a`: Rust's own support library carries them, for exactly this kind of target. So `float` and `double` work, slowly and correctly, and never touch a register the kernel forgets to save:

  ```
  taaj:/> chello float
  7.0 / 2 * 4 = 14
  ```

  Teaching the kernel to save FPU state would make them fast. It is on the list, and no longer urgent.

### 2. Where do `write` and `exit` come from?

A header says a function exists:

```c
/* libc/include/unistd.h */
ssize_t write(int fd, const void *buffer, size_t count);
```

That is a promise, not an implementation. The compiler emits "call something named `write`" and leaves a hole. The **linker** fills the hole from a library — a file of compiled functions with names on them.

Here is the decision of this entry. That library could be written in C. I wrote it in Rust:

```rust
#[no_mangle]
pub unsafe extern "C" fn write(fd: c_int, buffer: *const c_void, count: usize) -> isize {
    if !matches!(fd, 1 | 2) || (buffer.is_null() && count != 0) {
        return -1;
    }
    ...
    let bytes = unsafe { core::slice::from_raw_parts(buffer.cast::<u8>(), count) };
    match taaj::write(bytes) {
        Ok(_) => count as isize,
        Err(_) => -1,
    }
}
```

Two attributes make a Rust function into a C one:

- `extern "C"` — use the C **calling convention**: arguments in the registers C uses (`rdi`, `rsi`, `rdx`, …), result in `rax`. Rust's own convention is unspecified and may differ.
- `#[no_mangle]` — call it exactly `write` in the object file. Rust normally *mangles* names (`_ZN4taaj5write17h3f…E`) so that two functions called `write` in different modules do not collide; C has no modules and no mangling, so the name must be the plain one.

To the linker the result is indistinguishable from a function compiled from C. The C compiler never learns what language it was.

**Why Rust, for a C library?** Look at what `write` does: it calls `taaj::write`. That one function already knows that standard output may be the screen (copy to the log, give up on a stuck display), or a file the shell is writing, or a pipe to another program (and that the program must end if the reader has gone) — entries #31 and #32. The steps to come need the registry's protocol, the file service's, the arguments' encoding. All of that exists, tested, in the runtime the Rust programs use. A libc in C would be every one of those a second time, to be kept in step by hand for ever.

| | Rust core, C names | All in C |
|---|---|---|
| Protocols implemented | Once | Twice |
| A change to the file protocol | One place | Two, or C programs silently break |
| What I learn | How two languages meet at the linker | How a traditional libc is laid out |
| `printf` | Has to be C anyway — see below | C |

The exception, coming in step 4: `printf` takes a variable number of arguments, and stable Rust cannot *define* such a function. So the formatting family will be C, compiled into the same library. "Rust core, C on top."

The crate is built as a **static library**:

```toml
[lib]
name = "c"
crate-type = ["staticlib"]
```

which produces `libc.a`: an archive of object files containing my four functions, the `taaj` runtime, and Rust's `core` and `alloc` — including the `memcpy` and `memset` the C compiler may still call even when freestanding.

### 3. Who calls `main`?

Not the kernel. The kernel starts a program at the address the ELF file names as its entry — a symbol called `_start` — with two numbers in two registers: how many startup handles, and the parent's flags. It knows nothing of `argc`.

The code between "where the kernel starts you" and "`main`" has a traditional name: **crt0**, *C runtime, part zero*. It is the first thing in every C program ever linked, and most C programmers never see it.

```rust
#[no_mangle]
pub extern "C" fn _start(startup_handles: u64, argument: u64) -> ! {
    taaj::set_startup(startup_handles, argument);
    let mut strings: Vec<Vec<u8>> = Vec::new();
    strings.push(NAME.to_vec());
    strings.extend(taaj::arguments().into_iter().map(|argument| argument.into_bytes()));
    for string in &mut strings {
        string.push(0);
    }
    let mut argv: Vec<*mut c_char> = strings.iter_mut().map(|string| string.as_mut_ptr().cast()).collect();
    argv.push(core::ptr::null_mut());
    let code = unsafe { main(strings.len() as c_int, argv.as_mut_ptr()) };
    exit(code)
}
```

Its whole job is translation between two ideas of "a program's arguments":

| | TheTaaJ | C |
|---|---|---|
| Arrive as | One message on a channel, each argument followed by a zero byte (entry #30) | An array of pointers |
| A string is | Bytes with a known length | Bytes up to the first zero |
| How many | However many are in the message | `argc`, and *also* a null pointer after the last |
| The program's name | Not sent | `argv[0]` |

The zero byte that *separates* arguments on the wire is the same byte that *ends* a C string — which is not a coincidence; I took the encoding from how Unix passes arguments, for a reason I did not need until today.

And `main` is declared in Rust but defined in C:

```rust
extern "C" {
    fn main(argc: c_int, argv: *mut *mut c_char) -> c_int;
}
```

So the dependency runs both ways. The C program calls into the library (`write`); the library calls into the C program (`main`). Neither can be linked alone. The linker resolves both directions in one pass, and a C file with no `main` fails *at link time*, with "undefined symbol: main" — which is the right moment.

Calling `main` is `unsafe`, and the `// SAFETY:` comment has something real to say: the strings and the array must outlive `main`. They do, by an argument I rather like — `exit` never returns, so the function never reaches its end, so the vectors are never dropped.

### Then the linker

```
ld.lld -static -nostdlib --gc-sections -s -T user/user.ld chello.o libc.a -o chello
```

**The same linker script as the Rust programs.** `user.ld` says where a TheTaaJ program lives (4 MiB) and that code, constants and data go in separate segments with separate permissions, because the kernel's loader refuses anything writable and executable. A C program must obey the same rules, and by sharing the script it cannot fail to.

`--gc-sections` throws away every function nothing reaches. The library archive is 12 MB (it contains the whole runtime: the file protocol, the registry client, the heap). `chello` on the disk is **21 KB**.

The result is an ELF file the kernel cannot tell from `ls`.

## What `chello` shows

It writes its own numbers — there is no `printf` yet — and checks one promise crt0 made:

```c
/* The array ends in a null pointer: C promises it. */
if (argv[argc] != NULL)
    print("argv does not end in NULL\n");
```

- `chello a b` — `argc` is 3; the arguments arrive divided as the shell divided them, quotes and all (`"two words"` is one).
- `chello fail` — `return 3` from `main` becomes the exit code; the shell says so.
- `chello exit` — `exit(4)` from the middle of `main` does the same.
- `chello a b > cout`, `chello x | wc` — its output goes to a file and down a pipe. **Nothing was done to make this work.** `write(1, …)` is `taaj::write`, which has known about files and pipes since entry #31. This is the argument for the Rust core, demonstrated: a C program got redirection and pipes on its first day, for nothing.

## The build

```
make cprogs64                       builds libc.a, then every libc/programs/*.c listed
make cprogs64 C_PROGRAMS=mine       builds libc/programs/mine.c  ->  build/c/mine
```

`make run64` and the boot tests put the C programs on the disk beside the Rust ones. Without Clang and lld the C programs are skipped, with a message, and the tests do not ask for them.

## The design, in one table

| Decision | Chosen | Rejected, and why |
|---|---|---|
| Language of the library | Rust, exported under C names | C — every protocol twice |
| Kind of library | Static (`libc.a`), linked into each program | Shared — there is no dynamic loader |
| crt0 | A Rust `_start` in the library | Assembly — nothing here needs it; the kernel enters with an ordinary call's stack |
| Linker script | The Rust programs' own | A second one — two layouts to keep legal |
| Headers | My own, plus the compiler's (`-nostdlibinc`) | The host's — they describe Linux |
| Floating point | In software (`-mgeneral-regs-only`): slow, correct | The FPU's registers — wrong whenever the kernel switches, until it saves them |
| Warnings | `-Wall -Wextra -Werror` | Warnings that scroll past |
| Where it lives | A new top-level `libc/` | Inside `user/` — two build systems in one directory |

## The bugs

**A wrong belief, caught before it was published** — the floating-point story above. Not a bug in the code; a false sentence in this journal, which I count as worse.

The OS had none: the first boot ran the C program correctly, in every form I typed.

**The test was wrong.** I had written the expectation for `chello x | wc` before running it: `5 lines, 9 words, 44 bytes`. The system said `3 lines, 7 words, 30 bytes`. I had counted the output of `chello a b` — a different command, two lines up in my own list. Counted properly: `Hello from C`, `argc 2`, `argv[1] x` — 13 + 7 + 10 = 30 bytes, 3 + 2 + 2 = 7 words. The system was right. That is the second time `wc` has corrected my arithmetic (entry #31), and I am starting to think of a failing count as a suggestion to recount before debugging.

## How I checked it

**In the boot test** (BIOS and UEFI), 7 new lines are typed:

| Typed | Checked |
|---|---|
| `chello` | `Hello from C`, `argc 1` |
| `chello one "two words"` | `argc 3`, `argv[1] one`, `argv[2] two words` |
| `chello fail` | `chello exited with code 3` — `return` from `main` |
| `chello exit` | `chello exited with code 4` — `exit()` |
| `chello float` | `7.0 / 2 * 4 = 14` — arithmetic on a `double`, in software |
| `chello a b > cout` | the host reads `cout` from the disk: the four lines, exactly |
| `chello x \| wc` | `3 lines, 7 words, 30 bytes` |

The root listing now expects 18 files. The count of processes, threads and channels is still the same at the end of the test as at the start.

`make test64` passed.

**Not tested:**

- I have not typed any of this into a `make run64` window myself.
- **No host tests.** Everything so far in this project has a part that runs on my laptop. This library has none yet: it is four thin functions over a runtime that cannot be built for the host. That will change at step 4 — the formatting code will be plain C, and plain C *can* be compiled and tested on the host.
- **`write` to a bad descriptor, or with a null buffer** — returns -1 on paper.
- **A build without Clang.** The Makefile has a branch for it that I have not run.
- **`make cprogs64 C_PROGRAMS=mine`** with a file of my own. I compiled and linked such a file by hand with the same flags (it came to 21 KB), but did not run the `make` line, nor run the result in the OS.
- **Floating point beyond one division and one multiplication.** `chello float` shows the soft-float path works at all; nothing tests it in depth, and there is no way to print a fraction yet.

## What is still missing

Nearly everything a C programmer expects. In the order I mean to add it:

- **Standard error**: descriptor 2 is standard output for now.
- **`malloc`**, `string.h`, `ctype.h`, `errno`.
- **`printf`**.
- **`open`, `read`, `close`** and the rest of the descriptor calls; reading standard input.
- **`fopen`** and the `FILE` functions.
- **`argv[0]` is the word `program`.** The shell does not tell a program its name.
- **Floating point is done in software**, until the kernel saves the FPU's state; and nothing can print a fraction yet.
- **No environment** (`getenv`), no `fork`, no signals — and no plan for fork: starting a program here goes through init.

## What I learned

- **A C library is an adapter, not a standard.** It is where one OS's reality is made to look like what C programs assume.
- **`main` is not where a program starts.** Something has to build `argc` and `argv`, and it is ordinary code with a silly name.
- **Languages meet at the linker.** A name and a calling convention are the whole interface; `extern "C"` and `#[no_mangle]` supply both.
- **The header promises, the library delivers, the linker checks.** Three tools, three moments — and the error for a missing function comes from the third.
- **`-nostdlibinc` vs `-nostdinc`**: the compiler's headers describe the machine, the system's describe the OS. A new OS needs the first and must replace the second.
- **Check the claim, especially the confident one.** I "knew" the flag would refuse `double`. One compile showed it does something better. The entry nearly went out saying the opposite.
- **Soft float**: arithmetic the hardware could do, done by library calls instead, so that a whole class of registers need never be saved.
- **Reuse shows up as features you did not write.** Redirection and pipes for C programs cost nothing.
- **When a count fails, recount.**

---

## Testing note

```
make run64
```

`make run64` builds the C library and `chello`, and puts `chello` on your disk. You need Clang and lld (`sudo apt install clang lld`); you have them already if the UEFI loader builds.

**A C program runs**

| Type | You should see |
|---|---|
| `chello` | `Hello from C` and `argc 1` |
| `chello one "two words"` | `argc 3`, `argv[1] one`, `argv[2] two words` |
| `chello a 'b c' d\ e` | `argc 4`, with `b c` and `d e` each one argument *(not in the automated test)* |
| `ls` | `chello` in the listing, about 21000 bytes |

**How it ends**

| Type | You should see |
|---|---|
| `chello fail` | Its lines, then `chello exited with code 3` |
| `chello exit` | Its lines, then `chello exited with code 4` |
| `run chello` | Its lines, then `chello exited with code 0` — `run` always says *(not in the automated test)* |

**Arithmetic with a fraction**

| Type | You should see |
|---|---|
| `chello float` | `Hello from C`, `argc 2`, `argv[1] float`, then `7.0 / 2 * 4 = 14` |

**Its output goes where the shell says**

| Type | You should see |
|---|---|
| `chello a b > cout` | Nothing |
| `cat cout` | `Hello from C`, `argc 3`, `argv[1] a`, `argv[2] b` *(the `cat` is not automated; the host reads the file)* |
| `chello x \| wc` | `3 lines, 7 words, 30 bytes` |
| `chello & ` | `[1] chello`, its two lines, and at the next prompt `[1] done  chello` *(not in the automated test)* |

**It is given nothing it did not ask for**

| Type | You should see |
|---|---|
| `ps` while nothing runs, before and after `chello` | The same last line — nothing left behind |

**Building one of your own** *(not tested: please try it)* — on the host:

```
cat > libc/programs/mine.c <<'EOF'
#include <unistd.h>
int main(void) { write(1, "mine\n", 5); return 7; }
EOF
make cprogs64 C_PROGRAMS=mine
tools/tfstool-rs/target/release/tfstool put build/taaj-disk.img mine build/c/mine
```

(with QEMU closed), then `make run64` and type `mine`: you should see `mine` and `mine exited with code 7`.

And one that must **not** link — a file with no `main`:

```
printf 'int f(void) { return 1; }\n' > libc/programs/nomain.c
make cprogs64 C_PROGRAMS=nomain
```

The linker should say `undefined symbol: main` *(I ran the same two commands by hand, outside `make`, and saw that)*. Delete `libc/programs/nomain.c` and `mine.c` afterwards.

**From the host** — with QEMU closed:

```
tools/tfstool-rs/target/release/tfstool get build/taaj-disk.img cout
```

prints the four lines.

**Cleaning up**, in the OS: `rm cout`.

Automated: `make test64`.
