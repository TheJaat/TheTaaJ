# Memory Objects, and a Text Console Drawn in Ring 3

*Journal entry #11 · sharing pages instead of copying bytes; the screen leaves the kernel*

Channels (entry #9) move small messages by copying them. That is right for requests and replies, and hopeless for a screen: a 1024×768 framebuffer is 3 MB. This entry adds the second way processes share data — **mapping the same physical pages** — and uses it to move the last piece of hardware the kernel still drove, the display, out to a ring-3 service with its own font and text console.

---

## Memory objects

A **memory object** is a list of physical frames, as a kernel object reached through a handle. A process holding the handle can **map** those frames into its address space. Two processes that map the same object see the same bytes: what one writes, the other reads, with no copy and no system call.

Three calls: `memory_create(pages)`, `memory_map(handle, …)`, `memory_unmap(address)`.

### Rights decide how it may be mapped

Two new rights: `READ` and `WRITE`. A handle with only `READ` can only be mapped read-only — enforced by the page tables, so a write faults and the process is killed (a test program, `memro`, does exactly that at every boot). Since rights only shrink (entry #9), a process can share a buffer and keep the only writable view.

Mappings are **never executable**. Shared memory that could be run would undo the W^X rule from entry #8.

### The kernel chooses the address

When a process maps an object, the kernel picks where. Letting the process choose would work, but every check it would need — aligned? inside user space? clear of the program, the stack, other mappings? — disappears if the kernel simply hands out addresses from an area reserved for mappings.

`regions.rs` tracks that area: first fit over a sorted list, and **one unmapped guard page after every mapping**, so running off the end of a shared buffer faults instead of silently landing in the next one. It is pure logic, tested on the host.

### Lifetime: the question that matters

When are the frames freed? If the answer were "when the last handle closes", a process could map an object, close the handle, and be left holding a mapping of pages that have been **freed and given to someone else** — reading another process's data. That is a use-after-free across process boundaries.

So **a mapping is a reference too**. The frames are freed only when the last handle *and* the last mapping are gone. Closing the handle of a mapped object leaves the mapping working.

And one ordering rule on unmap: the **TLB is flushed before the frames can be freed**. The CPU caches translations; a stale cached entry pointing at a frame that has been reused would be the same bug by another route.

### Device memory

The framebuffer is physical memory too, but not RAM the kernel allocated. A **device memory object** names fixed physical addresses and owns nothing — unmapping it frees no frames. This is the distinction "owned or not" that entry #8's page-table teardown was built to respect.

## The screen leaves the kernel

Until now the kernel painted the framebuffer itself. Now it does not draw at all. It wraps the framebuffer in a device memory object and sends init a message: the description (`FramebufferInfo`: width, height, pitch, pixel format) plus the handle. init starts a new supervised service, **`display`**, and gives it that. The display service maps the framebuffer and draws from ring 3.

A bug in drawing code — an off-by-one past the end of a row — is now a crash of one restartable process, caught by the guard page, instead of a corrupted kernel.

## A font of my own

To show text, something must turn the character `A` into pixels. I drew a font: **5×8 pixels per character**, all of printable ASCII, in a text file, `console/src/font.txt`, where each glyph is literally drawn with `#` and `.`.

It is compiled into bits **at build time** by a `const fn` — Rust code the compiler itself runs. A malformed row or a glyph out of order *fails the build*. There is no font parsing at run time and no way to ship a broken font.

## The text grid

The `console` crate also holds the **grid**: a rectangle of character cells with a cursor and terminal behaviour — wrap at the right edge, scroll at the bottom, newline, carriage return, backspace (which moves the cursor without erasing; the shell sends backspace-space-backspace), tabs, form feed to clear, and one `?` for each character outside ASCII.

The grid knows nothing about pixels. It is pure logic with a "damage" list — which cells changed — and it is tested on the host. The display service draws the damaged cells at 2× scale below a title band, with an underline cursor.

One small trick: the screen's background is a gradient, and erasing a cell means restoring the background behind it. Rather than keep a copy of the background, the colour is **a function of the row**: erasing just recomputes it.

(Entry #19 returns to this code to make it fast.)

## The display protocol

Clients send the display service tagged messages (`abi::display`): `INFO` and `TEXT`. The tag exists because of restarts: text queued on the channel while the display service is being restarted must never be mistaken for the info message a fresh service expects first.

The shell got a send-only handle to the display's channel, and a prompt.

## Two bugs

**Interleaved lines.** Once, the log showed init's `started:` line split in two by a line from the display service. `println!` was making one system call per formatted fragment, and the scheduler switched processes between two fragments. The runtime's `print!`/`println!` now format into a buffer and write **once**: a line is one message, so it is atomic.

**A boot that never started.** Rarely — once in very many runs — a BIOS boot stops before the kernel's first line. To chase it, `qemu-boot.sh` now tells a hang from a QEMU exit, logs CPU resets, and takes a screenshot of a hang. It has not happened again since, and it is not explained. It stays on the list.

## Testing pixels

How does an automated test know the screen is right? QEMU's monitor can save a **screenshot** (`screendump`). `check-screen.py` reads the image and checks it: the title band is where it should be, the background is the gradient, and there is "ink" — text-coloured pixels — in the console area. It cannot read the text, but it can tell a working console from a black screen, a garbled pitch, or swapped colour channels.

For memory objects: `echoc` lends `echod` three pages; `echod` upper-cases text **in place**; `echoc` sees the change in its own mapping — and still sees it after closing its handle.

## What I learned

- **A mapping is a reference.** Lifetime rules for shared memory must count every way the memory can still be reached.
- **Flush, then free.** Order matters wherever the CPU caches something.
- **Do at build time what can fail at build time.** The font cannot be wrong at run time because it does not exist as data to parse at run time.

---

## Testing note

```
make run64
```

In the QEMU window you should see:

| Look for | Meaning |
|---|---|
| A title band at the top, a smooth gradient background | The display service mapped the framebuffer and painted it from ring 3 |
| A prompt, and crisp text as you type | The font and grid work |
| Type a line longer than the screen is wide | It wraps onto the next line |
| `help` several times, until the screen is full | The text scrolls up; nothing is left behind or smeared |
| Type `abc`, Backspace twice, `xy` | `axy` |
| `clear` | An empty console, prompt at the top |

In the terminal log:

| Line | Meaning |
|---|---|
| `framebuffer: 1024x768, handed to init` | The kernel gave the screen away |
| `[display] painted the 1024x768 screen from ring 3 (framebuffer at 0x100000000000); console 80x33` | Mapped at an address the kernel chose, in user space |
| `[process] N "memro" killed: page fault` and `user write at 0x100000000000: protection violation` | A write through a read-only mapping is refused by the hardware |

Automated: `make kernel64-test` (regions), `cd console && cargo test` (font and grid), `make boot64-test` (takes and checks a screenshot).
