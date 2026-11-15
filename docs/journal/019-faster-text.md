# Faster Text: Drawing Less, Waiting With a Limit, and a Log That Does Not Block

*Journal entry #19 · the display service, `object_wait_until`, and the kernel's debug log*

Entry #18 made the disk fast and left one number embarrassing: `cat` of a 270 KB text file was still printing after ten seconds. The data arrived from the disk almost instantly. Getting it in front of my eyes was the slow part.

This entry is about finding out *why* — the answer was not what I assumed — and three changes that brought it to 1.8 seconds.

---

## What happens when a program prints

A program calls `println!`. The runtime library (`taaj::write`) does two things with the bytes:

1. sends them to the **display service**, a ring-3 process that owns the screen, as messages on a channel;
2. writes them to the **kernel's debug log** — the serial line — with the `debug_write` system call. This is what the boot tests read, and what appears in the terminal `make run64` was started from.

I assumed the display was the whole problem. It was a quarter of it.

## Measure, again

The display service looked obviously guilty, so I started there (parts 1 and 2 below). `cat big` went from "more than 10 s" to **4.1 s**. Better, but why still four seconds?

I added temporary counters to the display: how many times it repainted, and how many timer ticks it spent doing so. For the whole 270 KB:

```
250 repaints, ~100 ticks painting
```

One second. The display accounted for 1 of the 4.1 seconds. So I tried the other path: for one run, I made large writes skip the debug log.

```
took 0.92 s
```

**Three of the four seconds were the serial line.** Not drawing — a copy of the text that, on a normal day, nobody reads. If I had kept optimising the drawing code, I could have made it infinitely fast and the command would still have taken more than three seconds.

So there were three problems, and I will take them in the order I found them.

## 1. The display drew far too much

The display service keeps a grid of characters (the `console` crate) and paints it with TheTaaJ's own 5×8 font. The old loop was:

- for each message: write its bytes into the grid; redraw the cells that changed;
- **if the text scrolled: redraw every cell on the screen.**

Once the cursor reaches the bottom line, every newline is a scroll. So every line of output repainted the whole screen — well over a thousand full repaints for that file. And each cell was painted twice: once with the background, pixel by pixel, each pixel bounds-checked, then again with the glyph on top.

### Draw only what differs

The grid says what the screen *should* show. I added a second copy that records what the screen *does* show:

```rust
/// What is on the screen now: the character drawn in each cell, and
/// where the cursor is drawn. 0 is "nothing drawn yet", which no cell of
/// the grid ever holds - so the first paint draws everything.
struct Shown {
    cells: [[u8; MAX_COLUMNS]; MAX_ROWS],
    cursor: (usize, usize),
}
```

Drawing is then one operation — bring the second in line with the first:

```rust
fn sync(&self, grid: &Grid, shown: &mut Shown) -> usize {
    let (columns, rows) = grid.size();
    let cursor = grid.cursor();
    let mut painted = 0;
    for row in 0..rows {
        for column in 0..columns {
            let c = grid.at(column, row);
            let here = (column, row);
            // A cell is stale if its character changed, or if the
            // cursor arrived or left.
            if shown.cells[row][column] != c || (here == cursor) != (here == shown.cursor) {
                self.cell(column, row, c, here == cursor);
                shown.cells[row][column] = c;
                painted += 1;
            }
        }
    }
    shown.cursor = cursor;
    painted
}
```

This replaced three separate pieces of logic (changed cells, scrolls, moving the cursor) with one rule. A scroll moves every line up — but in ordinary output most of a line is blank, before and after, and a blank cell that stays blank is not touched. Comparing two bytes is thousands of times cheaper than painting 240 pixels.

It also removed a class of bug by construction: there is no longer any way for the code to "forget" to redraw something after a scroll or a clear, because nothing is tracked except the difference itself.

### Take all the text first, then draw

The second waste: drawing after *every message*. If a program has already sent twenty messages, drawing after the first is pointless — the screen will be wrong again in a microsecond. A terminal does not draw every line of a fast-scrolling file; lines that scroll past between two frames are simply never drawn.

To do that, the display needs to ask: *is there another message waiting right now?* And TheTaaJ had no way to ask. `receive` waits. `object_wait` waits. That gap is the subject of part 2. With it filled, the loop is:

```rust
loop {
    let mut taken = 0;
    loop {
        let n = match taaj::receive(messages, &mut message, &mut []) { ... };
        if n > 0 && message[0] == abi::display::TEXT {
            grid.write_all(&message[1..n], &mut |_| {});
        }
        taken += 1;
        // More, right now?
        if taken == BATCH || !matches!(taaj::poll(&[messages]), Ok(Some(_))) {
            break;
        }
    }
    screen.sync(grid, shown);
}
```

`BATCH` is 64: a program that never stops printing must not keep the screen frozen forever.

### Paint each pixel once

`cell` now writes each pixel exactly once — text colour where the glyph has a dot, background elsewhere — a row at a time through a pointer to the start of the row, after one bounds check for the whole cell instead of one per pixel.

### And do not lose text

The old sender dropped text when the display's queue (32 messages) was full: "a display that lags loses some text". With a fast disk feeding `cat`, that would have meant a file with holes in it on the screen. Now a full queue makes the *program* step aside:

```rust
Err(Error::QueueFull) if ticks() - started < PATIENCE && !STALLED.load(Ordering::Relaxed) => {
    // A full queue means the display has work, so it is
    // ready to run: giving up the CPU lets it.
    yield_now();
}
```

This is **backpressure**: the fast side is slowed to the speed of the slow side, instead of data being lost or memory growing without bound. But it must have a limit. If the display service is gone for good, a program that prints must not hang with it. After about a second without room the message is dropped, and a flag (`STALLED`) records that this display is not keeping up — from then on nothing waits for it, until a message gets through again.

## 2. Waiting with a limit

"Is anything ready *right now*?" is a wait with a time limit of zero. So rather than add a special "peek" call, I added the general thing, which the system needed anyway: a wait with a **deadline**.

```rust
/// object_wait_until(handles, count, deadline) - as object_wait,
/// but gives up with TimedOut once the tick count (TICKS) reaches
/// `deadline`. A deadline that has passed - 0, say - asks without
/// waiting: "is any of these ready now?"
pub const OBJECT_WAIT_UNTIL: u64 = 24;
```

The deadline is absolute (a tick count), not a duration. An absolute deadline can be passed down unchanged through several calls; a duration has to be recomputed at every step or the waits add up.

### How it works in the kernel

The scheduler already knew two ways for a thread to be off the CPU:

- **Blocked** — until someone calls `wake`;
- **Sleeping(until)** — until the timer reaches that tick.

And `wake` already worked on both. So a wait with a deadline needed no new scheduler state at all. It is *a sleep that sits in wait queues*:

```rust
suspend(deadline.map_or(State::Blocked, State::Sleeping));
```

The thread joins the queues of what it is waiting for, then sleeps until the deadline. If a message arrives, the queue wakes it early. If nothing arrives, the timer wakes it. Either way it wakes up, looks at the condition and the clock again, and returns one of three answers:

```rust
pub enum Waited {
    Done,      // what was waited for happened
    Killed,    // the thread's process is being killed (entry #17)
    TimedOut,  // the deadline came first
}
```

The old `wait_any` is now this function with no deadline. One loop, one set of race conditions to get right — the same "check with interrupts off, then block" reasoning as in entry #17 covers the deadline check too.

In the runtime library, a timeout is not an error — it is one of the expected answers — so it comes back as `None`:

```rust
pub fn object_wait_until(handles: &[Handle], deadline: u64) -> Result<Option<usize>, Error>
pub fn poll(handles: &[Handle]) -> Result<Option<usize>, Error>   // deadline 0
```

Timed waits will matter well beyond the display: a shell that should not hang forever on a file service that stopped answering needs exactly this.

## 3. The log that made everyone wait

Now the three seconds. `debug_write` did this:

```rust
kprint!("{}", String::from_utf8_lossy(&bytes));
```

and `kprint!` took the console lock — which turns interrupts off — and pushed every byte out of the serial port, waiting for the port to be ready before each one. A serial port at 115200 baud carries one byte every 87 microseconds. So:

- **The program waited** for the serial line, byte by byte. 270 KB is 270,000 waits.
- **The kernel kept interrupts off for the length of each message.** For a 1 KB message at real serial speed that is 89 milliseconds with no timer ticks and no keyboard — eight ticks lost. I had not noticed because QEMU's serial port is much faster than a real one. On real hardware this would have been a serious bug: time itself running slow whenever a program printed.

### A queue, emptied when idle

What programs print now goes into a ring buffer in the kernel and comes out when the CPU has nothing better to do:

```rust
/// Queues a program's output for the log. Never waits: if there is no
/// room the message is dropped, and counted.
pub fn write_later(bytes: &[u8]) {
    QUEUE.lock().push(bytes);
}
```

and the idle thread — the thread that runs only when nothing else can — does the writing:

```rust
fn idle_loop() {
    loop {
        while crate::console::write_queued() {}
        arch::wait_for_interrupt();
    }
}
```

`write_queued` writes 16 bytes at a time and releases the lock between chunks, so interrupts are never off for long, and an interrupt that makes a real thread ready takes the CPU away from the idle thread immediately.

### Three decisions

**The kernel's own messages are not queued.** `kprintln!` still writes at once. A panic message that waited in a queue would never be seen — nothing runs after a panic. To keep the order right, the kernel first writes out everything queued before its own message:

```rust
pub fn print(args: fmt::Arguments) {
    let mut queue = QUEUE.lock();
    // What programs printed before this comes before it.
    flush(&mut queue, usize::MAX);
    let _ = Console.write_fmt(args);
}
```

So the log still reads in the order things happened, and a crash report is still preceded by everything the programs said before it.

**The queue has a fixed size — 64 KiB — and can overflow.** It is kernel memory; a program must not be able to grow it. When a message does not fit, there are two choices: make the program wait (which is the problem I was removing) or drop the message. I chose to drop — *whole messages only*, never half a line — **and to say so**:

```
[log] 12345 bytes of program output are missing here: the serial line could not keep up
```

A log that silently loses lines is worse than no log, because you trust it. A log that tells you it has a gap is still a tool. In practice the gap never appeared in my tests: even the 270 KB flood got through complete, because the pipeline has idle moments in which the queue drains.

**The bookkeeping is tested on the host.** `LogRing` (in the kernel's library crate) is plain data: push all-or-nothing, pop in order, wrap around, count what was dropped. Four tests, including one that wraps the ring's end fifty times.

## Results

All in QEMU, same file, same commands:

| | Before | After |
|---|---|---|
| `cat big` (270 KB, 3,500 lines) | not finished after 10 s | **1.82 s** |
| `ls` | 0.07 s | 0.02 s |
| `help` | — | under 0.01 s |

And the steps on the way, for `cat big`:

| Change | Time |
|---|---|
| Start | > 10 s |
| Display: draw differences, in batches, one write per pixel | 4.1 s |
| Sender yields instead of sleeping when the queue is full | 4.1 s (no change — an honest null result) |
| Debug log asynchronous | 1.8 s |

The middle row is worth keeping in the table. I was sure the sender's 10 ms sleeps were costing seconds. They were not. Only measuring said so.

## How I checked it

- **Timed waits** — added to `echoc`, the program that tests every IPC rule at each boot: `poll` on an empty channel returns at once with "nothing"; a wait with a deadline 3 ticks away returns "nothing" after 3 to 5 ticks; with a message waiting, both return its index, and neither consumes the message.
- **The log queue** — four host tests.
- **A flood** — the boot test now `cat`s a 300-line file, far more than a screen, and the last line (`line 300 of 300`) must be in the log: nothing lost under backpressure.
- **No stale cells** — the risk of "draw only what changed" is a cell wrongly believed unchanged. So after the 300 lines the test types `clear` and then `echo screen`, takes a screenshot, and counts text-coloured pixels. A correct screen has 311 (sampled); the test fails above 700. A single leftover line from the flood would exceed that many times over.
- **Crash reports** — the five deliberate-fault boots still pass, so a panic or CPU exception still reaches the serial line with the queue in front of it.
- BIOS and UEFI, several boots each.

## What is still not right

- **Dense text is the worst case.** A screen full of characters that all change on every scroll gets no benefit from "draw only what differs". The real fix for scrolling is copying pixels up instead of redrawing — which needs the background to stop being a gradient, or to be drawn separately from the text.
- **The log can now have gaps** under a flood, by design.
- **Output order between programs** is as before: each `debug_write` is whole, but two programs' lines interleave by time.
- **The display still redraws from the grid only.** No scrollback: what scrolls off the top is gone.

---

## Testing note

```
make run64
```

Type in the QEMU window.

| Type | You should see |
|---|---|
| `help` | The whole list at once, no visible line-by-line drawing |
| `ls` several times until the screen scrolls | Smooth scrolling; old lines move up, nothing left behind, no flicker of the whole screen |
| `time ls` | `took 0.0N s` — a few hundredths |
| `write a one`, then `append a two` five or six times, then `cat a` | All lines, in order |
| `clear` | An empty screen with just the prompt — **no leftover characters anywhere** |
| Type a long line, then hold Backspace | Characters disappear one by one; the cursor underline follows; nothing left behind |
| `forever`, then Ctrl+C | Still works (it uses the wait that now has a deadline version) |

A bigger test, if you want to see the flood case. On the host, before starting, put a large text file on the disk:

```
seq 5000 | sed 's/.*/this is line & of the big file/' > /tmp/big.txt
tools/tfstool-rs/target/release/tfstool put build/taaj-disk.img big /tmp/big.txt
make run64
```

then in the OS:

| Type | You should see |
|---|---|
| `time cat big` | Text streaming past for a second or two, ending with `this is line 5000 of the big file`, then `took N.NN s` |
| (the terminal log) | The same 5,000 lines, complete. If the serial line could not keep up you would see a line starting `[log] … bytes of program output are missing here` — that is the designed behaviour, but tell me if it appears, since it did not in my runs |
| `clear` | A clean screen |

Automated: `make test64`.
