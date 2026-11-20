# Memory and Strings: `malloc`, `free`, and the Sixteen Bytes in Front

*Journal entry #38 · a C program can ask for memory while it runs and give it back, and has `string.h` — the heap was already there; the work was the one thing C's `free` is not told*

Until now a C program on this system could do three things: read its arguments, write bytes, and end. It could not hold anything whose size it did not know when it was compiled, and it had to write its own loop to find the length of a string. After this entry:

```
taaj:/> cmem
strings: 28 checks, all passed
memory: 15 checks, all passed
taaj:/> cmem sort pear apple fig apple
apple apple fig pear
taaj:/> cmem huge
cmem: no memory for a terabyte
cmem exited with code 1
```

`cmem sort` is a small program of the ordinary C kind: a list that grows with `realloc` as words arrive, each word a copy made by `strdup`, put in order with `strcmp`, every block given back with `free`.

This is step 3 of the seven towards a C library: (1) toolchain and crt0 — entry 36; (2) standard error — entry 37; (3) **memory and strings — this entry**; (4) `printf`; (5) file descriptors; (6) `stdio` on files; (7) a real program and the recipe.

---

## What was already there

Two things, and finding them was half the entry.

**The heap.** Every Rust program on this system has had one since entry 13: `Vec` and `String` ask it for memory, and it asks the kernel for more pages when it runs out. The C library is linked with the same runtime (that was the "Rust core, C on top" decision), so the heap is *in* every C program already. `malloc` did not need a heap written. It needed the existing one given a C name.

**Five functions.** Before writing `memcpy` I looked at what `libc.a` already held:

```
$ nm libc/target/x86_64-unknown-none/release/libc.a | grep -E " [TW] (mem|str|bcmp)"
0000000000000000 W bcmp
0000000000000000 W memcmp
0000000000000000 W memcpy
0000000000000000 W memmove
0000000000000000 W memset
0000000000000000 W strlen
```

They come with `compiler_builtins`, the small library Rust links into everything. The Rust compiler itself turns `a = b` on a big struct into a call to `memcpy`, so on a target with no operating system it has to bring its own. The `W` means *weak*: "use this unless somebody defines a real one". For those five, all I had to write was the line in the header that tells a C compiler they exist.

So the list of what had to be *written* was: `malloc`, `free`, `calloc`, `realloc`, `strdup`, and eleven string functions.

## The one thing that did not fit

Here is how Rust gives memory back:

```rust
unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout)
```

and here is how C does:

```c
void free(void *block);
```

Rust's allocator is told the **size** (inside `Layout`) when a block comes back. It costs Rust nothing — a `Vec` knows its capacity, a `Box<T>` knows the size of `T` — and it saves the heap from having to remember. C's `free` is told an address and nothing else. `free(p)` has to work for a block of 3 bytes and a block of 3 megabytes, and the only thing it gets is `p`.

So somebody has to remember each block's size, and the classic answer — the one almost every `malloc` ever written uses — is to keep it **just in front of the block**:

```
 what the heap handed over
 |
 v
 +----------------+-----------------------------------+
 | size (8) + pad |  the bytes the program asked for  |
 +----------------+-----------------------------------+
                  ^
                  |
                  what malloc returns, and free is given
```

`malloc(100)` asks the heap for 116 bytes, writes `100` at the start, and returns the address 16 bytes in. `free(p)` steps back 16 bytes, reads the size, and now knows what to tell the heap.

```rust
const HEADER: usize = 16;
const ALIGNMENT: usize = 16;

#[no_mangle]
pub extern "C" fn malloc(size: usize) -> *mut c_void {
    let Some(layout) = layout(size) else {
        return ptr::null_mut();
    };
    // SAFETY: the layout is never of size zero: it includes the header.
    let block = unsafe { alloc(layout) };
    if block.is_null() {
        return ptr::null_mut();
    }
    unsafe {
        block.cast::<usize>().write(size);
        block.add(HEADER).cast()
    }
}
```

```rust
#[no_mangle]
pub unsafe extern "C" fn free(pointer: *mut c_void) {
    if pointer.is_null() {
        return;
    }
    unsafe {
        let size = size_of_block(pointer);
        let layout = Layout::from_size_align_unchecked(size + HEADER, ALIGNMENT);
        dealloc(pointer.cast::<u8>().sub(HEADER), layout);
    }
}
```

**Why 16 bytes, when the size is only 8?** Alignment. C promises that what `malloc` returns can hold *any* ordinary type, and some types must sit at addresses that are multiples of 16 (`long double` on x86-64). If the heap's block starts on a multiple of 16 and the header were 8 bytes, the program's part would start at 16n + 8 — wrong. A 16-byte header keeps the program's address as well aligned as the block's.

**`Layout`**, since it was new to me as a thing I build by hand: it is a pair — a size and an alignment — and it is how Rust describes "a piece of memory" to an allocator. `Layout::from_size_align` checks the pair makes sense (the alignment a power of two, the size not absurd) and returns a `Result`. The little helper turns both possible failures into one `None`:

```rust
fn layout(size: usize) -> Option<Layout> {
    Layout::from_size_align(size.checked_add(HEADER)?, ALIGNMENT).ok()
}
```

`checked_add` returns `None` instead of wrapping round if `size + 16` overflows; the `?` passes that `None` straight out of the function. `malloc(SIZE_MAX)` therefore returns `NULL` rather than asking the heap for 15 bytes.

## The other three, each in a sentence

**`calloc(count, size)`** is `malloc(count * size)` plus zeros — *and the multiplication checked*. That check is the reason `calloc` exists as a separate function: written by hand, `count * size` can wrap round to a small number, the program gets a small block, and then writes `count` things into it. `checked_mul` again.

**`realloc(block, size)`** here always takes a new block, copies the smaller of the two sizes, and frees the old one. That is correct and it is the simplest thing that is. A better one would look at whether the block can grow where it stands; mine cannot, because the heap underneath has no such question to ask.

**`strdup(s)`** is the one string function that needs `malloc`, so it lives with it.

## What the kernel says when there is no memory

`malloc` returning `NULL` is not decoration. Rust's `Vec` *panics* when the heap fails; C's contract is that the program is told and decides for itself. The raw `alloc::alloc::alloc` I call is the lower-level function under `Vec`, and it returns a null pointer rather than panicking — so the C contract comes out naturally:

```
taaj:/> cmem huge
cmem: no memory for a terabyte
cmem exited with code 1
```

A terabyte is refused before the kernel is even asked: it is bigger than any one memory object may be (`abi::MEMORY_MAX`, 16 MiB), and the heap's `grow` says no. That limit is worth knowing for its own sake: no single block from `malloc` can be bigger than that today, however much memory the machine has.

## The strings

A C string is not a type. It is a promise: *at this address there are bytes, and somewhere after them one that is zero.* Every function in `string.h` finds the length by walking. That is why all of them are `unsafe` in Rust — the compiler cannot check the promise.

My way of keeping the `unsafe` small was to turn the promise into a Rust slice once, at the top, and do the real work in safe code:

```rust
/// A C string as a Rust slice: its bytes, without the zero.
unsafe fn bytes<'a>(s: *const c_char) -> &'a [u8] {
    unsafe { slice::from_raw_parts(s.cast::<u8>(), length(s)) }
}

#[cfg_attr(not(test), no_mangle)]
pub unsafe extern "C" fn strcmp(a: *const c_char, b: *const c_char) -> c_int {
    match unsafe { bytes(a).cmp(bytes(b)) } {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    }
}
```

`strcmp` is one line of work, because comparing two `&[u8]` slices in Rust is *already* dictionary order with bytes as numbers 0–255 — which is exactly what C specifies, including the easy-to-get-wrong part: `"\xe9"` (an `é` in Latin-1, a *negative* number as a signed `char`) sorts after `"z"`.

`strstr` is the same idea:

```rust
match within_bytes.windows(find.len()).position(|window| window == find) {
    Some(at) => unsafe { within.add(at).cast_mut() },
    None => ptr::null_mut(),
}
```

`windows(n)` gives every run of `n` neighbouring bytes in turn; `position` gives the index of the first that matches.

The functions that are told a limit (`strncmp`, `strncpy`, `strncat`, `strnlen`) could not use `bytes`, and the reason is worth writing down: `bytes` walks to the zero, and a caller of `strncmp(a, b, 4)` has only promised four readable bytes. There may be no zero at all. Those use a second helper, `length_within(s, max)`, that never looks past `max`.

What is there:

| Written here | Already in `libc.a` |
|---|---|
| `strnlen` `strcmp` `strncmp` `strcpy` `strncpy` `strcat` `strncat` `strchr` `strrchr` `strstr` `memchr` `strdup` | `memcpy` `memmove` `memset` `memcmp` `strlen` |

## Testing a libc on the host, without a new crate

The string functions ask the system for nothing — they are arithmetic on bytes — so they can be tested on my own machine like the rest of the pure code. But `libc/` as a whole cannot be built for Linux: it depends on the `taaj` runtime, which *is* the operating system interface.

So the Makefile gives the one file to the compiler on its own:

```make
libc-test:
	rustc --edition 2021 --test $(LIBC_DIR)/src/string.rs -o $(LIBC_DIR)/target/string-test
	$(LIBC_DIR)/target/string-test
```

There is a trap in this, and it is the reason for the odd attribute on every function:

```rust
#[cfg_attr(not(test), no_mangle)]
pub unsafe extern "C" fn strcmp(...)
```

`#[no_mangle]` means "call this function exactly `strcmp` in the object file". In the real library that is the whole point. In a *test program running on Linux* it would be a disaster: the test binary is linked with the host's C library, and Rust's own standard library calls `strcmp`, `strlen` and friends internally. A function of mine exported under that name would quietly take the place of the host's — and I would be testing my `strcmp` with a test harness that runs on my `strcmp`. `cfg_attr(not(test), no_mangle)` reads: *apply `no_mangle` except when compiling tests.* In the test build the functions keep their ordinary, mangled Rust names and collide with nothing.

## `cmem`

The host tests cannot see three things: whether the **header** a C compiler reads matches the function, whether the **linker** finds every name, and whether the **heap of a real process** behaves. `cmem` is a C program that checks those from the inside: 28 checks of `string.h` and 15 of `malloc` and its family, each named, a failure reported on standard error (the stream from the last entry earning its place).

The memory check I like best:

```c
/* free really gives memory back: 64 KiB taken and returned twenty
 * thousand times is 1.3 GiB, more than this machine has. If free
 * did nothing, malloc would run out. */
for (int round = 0; returned && round < 20000; round++) {
    unsigned char *block = malloc(64 * 1024);
    if (block == NULL) {
        returned = 0;
        break;
    }
    block[0] = 1;
    block[64 * 1024 - 1] = 1;
    free(block);
}
```

A test of `free` is awkward, because a `free` that does nothing *looks* fine — the program just uses more memory. This one can only pass if the memory really comes back.

## The design, in one table

| Decision | What I chose | What I rejected, and why |
|---|---|---|
| The heap under `malloc` | The runtime's existing heap, through `alloc::alloc::alloc` | A second heap written for C — two allocators in one process, and twice the bugs |
| Where a block's size is kept | In a 16-byte header in front of the block | A table of address → size beside the heap (a search on every `free`); or changing the `heap` crate to remember sizes itself (it would cost every Rust allocation too, which already knows its size) |
| Header size | 16 bytes, though the size needs 8 | 8 bytes — the program's address would be 8 off a multiple of 16, breaking C's alignment promise |
| `malloc(0)` | A real, freeable, non-null address | `NULL` — allowed by the standard, but then "NULL" means both "no memory" and "you asked for nothing", and programs that check for failure break |
| `realloc` | Always new block, copy, free | Growing in place — needs a question the heap cannot answer yet. Correct first |
| `realloc(p, 0)` | Frees, returns `NULL` | Returning a zero-size block — the old behaviour is what existing programs expect |
| Out of memory | `NULL` | Panicking as Rust's `Vec` does — not C's contract |
| The five `mem*`/`strlen` | Use the ones `compiler_builtins` brings | Writing my own — they would have to be at least as good, and the compiler's own generated calls go to the same names either way |
| String functions' insides | Turn the pointer into a slice, then safe Rust | Pointer loops throughout — more `unsafe`, and every one a place to be off by one |
| Host tests | `rustc --test` on the one file | A new crate for the pure parts (more structure than eleven functions deserve); or testing only in the booted system (slow, and a failure there says less) |
| No `errno` | `malloc` does not set `ENOMEM` | Adding `errno` now — it belongs with file descriptors, step 5, where every call needs it |

## The bugs

None that ran. The Rust side compiled and passed its tests the first time, and so did the C program — which mostly says the step was small because the hard parts (the heap, entry 13; the toolchain, entry 36) were done.

Because nothing failed, I did not trust it, and broke it on purpose: I changed `free` to return at once and booted. The result —

```
strings: 28 checks, all passed
cmem: failed: free gives memory back
memory: 14 checks, 1 FAILED
cmem exited with code 1
```

— told me two things: the check for `free` really does check it, and a process that has *actually used up all the memory it may have* gets a `NULL` and carries on, with the rest of the system unharmed. Then I put `free` back.

## How I checked it

**Automated** (`make test64`):

- **Host:** 10 tests of the string functions (`make libc-test`, now part of the host tests): order and unsigned comparison in `strcmp`/`strncmp`, the limits of the `n` functions, `strncpy`'s padding and its missing zero, nothing written past the end by `strcat`/`strncat`, first and last in `strchr`/`strrchr` including finding the zero, `strstr` at the start, the end, absent and empty, `memchr` past a zero.
- **In the booted system, BIOS and UEFI:** `cmem` must print `strings: 28 checks, all passed` and `memory: 15 checks, all passed`; `cmem sort pear apple fig apple` must print `apple apple fig pear`; `cmem huge` must print `cmem: no memory for a terabyte` and exit with code 1.
- `ls` must now count 19 files in the root.

**By hand:** the broken-`free` boot above, once.

**Not tested:**

- `memcpy`, `memmove`, `memset`, `memcmp`, `strlen` have no host tests of mine — they are not my code. `cmem` uses each of them at least once.
- `malloc`, `free`, `calloc`, `realloc` have no host tests: they need the runtime's heap. They are tested only inside the booted system.
- Freeing twice, freeing an address that never came from `malloc`, writing past a block. Nothing detects these; they corrupt the heap, as on most systems.
- `realloc` failing (returning `NULL` with the old block intact).
- Many live blocks at once, or fragmentation after long use.

## What is still missing

- `realloc` never grows a block in place.
- No single block can be bigger than 16 MiB (one memory object), though many blocks together can be.
- No `errno`, so no `ENOMEM`.
- No protection at all against double `free` or overruns — no canaries, no checks.
- Not in `string.h` yet: `strtok`, `strspn`/`strcspn`/`strpbrk`, `strerror`, `memccpy`, the locale ones (`strcoll`, `strxfrm`).
- Not in `stdlib.h` yet: `atoi`/`strtol`, `abs`, `qsort`, `abort`, `getenv`, `atexit`.
- `argv[0]` is still the placeholder `program`.
- C still has to print its own numbers. `printf` is next.

## What I learned

- **Look before writing.** One `nm` showed five functions I was about to write were already in the file. The same habit found the heap. Most of this step was *recognising* what existed.
- **`free` is told less than Rust's `dealloc`, and that single difference is the whole design of a C allocator's header.** I had read that "malloc stores the size before the block" many times. It only became obvious *why* when I held the two function signatures next to each other.
- **A header's size is decided by alignment, not by what is in it.**
- **`checked_add` and `checked_mul` are how overflow becomes an ordinary `None`** — and `calloc` exists precisely because C has no such thing built in.
- **`#[no_mangle]` is a statement to the linker, and the linker does not care which language spoke.** That is what makes a libc in Rust possible; it is also what would have let my `strcmp` replace Linux's inside my own test program. `cfg_attr(not(test), …)` is the guard.
- **A test that cannot fail is not a test.** `free` doing nothing passes every check except the one designed around it. Breaking the code on purpose is the only way I know to find out which kind of test I wrote.

---

## Testing note

```
make run64
```

**The checks**

| Type | You should see |
|---|---|
| `cmem` | `strings: 28 checks, all passed` then `memory: 15 checks, all passed` (the second takes a moment: it grows a block to a megabyte and takes and frees 64 KiB twenty thousand times) |

**A program that uses them**

| Type | You should see |
|---|---|
| `cmem sort pear apple fig apple` | `apple apple fig pear` |
| `cmem sort b a C` | `C a b` — capitals sort before small letters, as bytes do *(not in the automated test)* |
| `cmem sort` | An empty line *(not in the automated test)* |
| `cmem sort "two words" one` | `one two words` *(not in the automated test)* |
| `cmem sort pear fig \| wc` | `1 lines, 2 words, 9 bytes` *(not in the automated test)* |

**No memory**

| Type | You should see |
|---|---|
| `cmem huge` | `cmem: no memory for a terabyte`, then `cmem exited with code 1` |
| `cmem huge > out` | The same two lines — the complaint is on standard error — and `cat out` shows nothing *(not in the automated test)* |
| `ps` afterwards | The system as it was: nothing left behind |

**Called wrongly**

| Type | You should see |
|---|---|
| `cmem what` | `usage: cmem [sort WORD... \| huge]`, then `cmem exited with code 2` *(not in the automated test)* |

**On the host**

```
make libc-test
```

ends with `test result: ok. 10 passed; 0 failed`.

**Using them in a program of your own** *(not tested: please try it)* — on the host:

```
cat > libc/programs/mine.c <<'EOF'
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
int main(int argc, char **argv)
{
    for (int n = 1; n < argc; n++) {
        size_t length = strlen(argv[n]);
        char *backwards = malloc(length + 1);
        for (size_t at = 0; at < length; at++)
            backwards[at] = argv[n][length - 1 - at];
        backwards[length] = '\n';
        write(1, backwards, length + 1);
        free(backwards);
    }
    return 0;
}
EOF
make cprogs64 C_PROGRAMS=mine
tools/tfstool-rs/target/release/tfstool put build/taaj-disk.img mine build/c/mine
```

(with QEMU closed), then `make run64` and type `mine hello world`: you should see `olleh` and `dlrow`. Delete `libc/programs/mine.c` afterwards, and `rm mine` in the OS.

Automated: `make test64`.
