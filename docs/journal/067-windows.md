# Windows

*Journal entry #67 · Era 5, step B. The display service becomes a window server. Programs open windows whose pixels are memory they share with it; the server composes the screen from them, bottom to top, through a buffer of its own, and decides what every click and key is for. The shell's console is now a window too, the terminal, and it can be dragged by its title bar. A new crate, `gui`, holds the protocol, the window manager's rules and a small toolkit, with no machine in it. `counter`, a window with a count and two buttons, is the first program with a window of its own.*

```
[display] window 1 "terminal" opened, 984x632 at (19, 80)
[display] window 2 "counter" opened, 240x110 at (200, 150)
[display] focus: window 2 "counter"
[counter] window 2 open, 240x110
[display] left button at (265, 247)
[counter] clicks: 1
[counter] key +
[counter] clicks: 2
[display] focus: window 1 "terminal"
[display] window 1 moved to (519, 330)
[display] window 2: its program is asked to close it
[counter] closed by its close box
[display] window 2 closed
```

## Why

Step A put a screen and a pointer on both machines, but the pointer had nothing to point at: the whole screen was one console. Era 5's goal is to write a journal entry in a text editor running in a window on TheTaaJ. For that, the system needs windows:

- **somewhere to draw:** a program needs a rectangle of the screen that is its own;
- **somewhere for input to go:** clicks to the window under the pointer, keys to the window the user chose;
- **a way to arrange them:** windows overlap, so something must keep the order, raise the one clicked, move one when its title bar is dragged, and close one when asked;
- **a toolkit**, so that each program does not draw its own buttons from nothing.

This step builds all four, and turns the console into the first window.

## The picture

```
                      ┌──────────────── display: the window server ─────────────────┐
  kbd / hid ── keys ─▶│  keys ──▶ the window with the focus                          │
                      │             ├─ the terminal ──▶ the shell's keys channel ───┼──▶ shell
                      │             └─ a program ─────▶ KEY on its window channel ──┼──▶ counter
  kbd / hid ─ pointer▶│  Desktop (gui::desktop): what does this press mean?          │
                      │             └─ a click in a window ─▶ POINTER on its channel ┼──▶ counter
  shell ──── text ───▶│  the terminal's grid ──▶ the terminal's pixels               │
  counter ── OPEN ───▶│  windows: [terminal, counter, ...]                           │
  counter ── DAMAGE ─▶│                                                              │
                      │  compose: desktop, then each window, bottom to top,         │
                      │           into the back buffer ──▶ copy to the framebuffer   │
                      └──────────────────────────────────────────────────────────────┘
                        counter's pixels: memory counter made, mapped by both
                        (by counter to write, by the server only to read)
```

## Shared memory, not pixels in messages

A message on a TheTaaJ channel holds about a kilobyte. A 240 x 110 window is 105 KB of pixels, and a whole screen 3 MB. Sending pixels through messages would mean hundreds of messages per frame, each copied by the kernel twice.

So the pixels never travel. To open a window, a program:

1. creates a memory object big enough for its pixels, and maps it;
2. creates a channel: one end it keeps, the other it gives away;
3. sends OPEN on the server's channel (the registry's `windows`), with the size and title, and two handles: the channel end, and a **read-only** copy of the memory object.

The server maps the same memory, read-only. From then on the program draws into its pixels whenever it likes, and sends a short DAMAGE message (x, y, width, height) to say which part changed. The server copies that part to the screen.

```rust
// In the program (taaj::window::Window::open):
let memory = taaj::memory_create(size)?;
let pixels = taaj::memory_map(memory, true)?;          // ours, to write
let (ours, theirs) = taaj::channel_create()?;
let seen = taaj::duplicate(memory, READ | TRANSFER)?;  // the server's: read only
taaj::send(server, &open_message, &[theirs, seen])?;
```

Why read-only? The server cannot be harmed by a program writing its own pixels while they are being copied: the worst is a torn frame, fixed at the next damage. But a program must not be able to harm the server, and the server never needs to write a program's pixels. So it does not get the right to. The kernel enforces the rights on the handle, so this is not a promise in the code; it is a fact about the mapping.

A window is closed by closing its channel. That covers the case that matters most: a program that crashes. The kernel closes all its handles, the server's end of the channel reports the peer closed, and the window goes. No program can leave a dead window on the screen.

## The protocol: `gui::protocol`

Everything after OPEN travels on the window's channel, both ways:

| From | Message | Meaning |
|---|---|---|
| server | `OPENED id, format` | the window is open; draw in the screen's own pixel format |
| server | `REFUSED why` | it is not; why, in words |
| server | `POINTER x, y, buttons` | a press or release in the window, or a move while a button that went down in it is held (in the window's own coordinates) |
| server | `KEY bytes` | keys typed while the window has the focus, as a terminal sends them |
| server | `FOCUS yes/no` | the window got or lost the focus |
| server | `CLOSE` | its close box was clicked: the program is asked to close |
| program | `DAMAGE x, y, w, h` | this part has new pixels |
| program | `TITLE text` | a new title |

The server tells the program the screen's pixel format, so the program's pixels can be copied to the screen as they are, with no conversion per pixel. On QEMU both machines use 0x00RRGGBB, but a real graphics card may not.

`CLOSE` is a request, not an order. The server does not close the window when the close box is clicked; it asks the program, and the program closes the window by closing its channel. That matters for the next step: an editor with unsaved changes must be able to ask "save first?".

Each message type has an `encode` and a `decode`, and decoding refuses anything malformed: a size out of range, a title too long, a message cut short. These are tested on the host, as are the round trips.

## The window manager's rules: `gui::desktop`

The server's bookkeeping has no pixels and no channels in it, so all of a window manager's behaviour is tested on the host. `Desktop` keeps the windows in stacking order, bottom first, and knows which one has the focus. The server tells it what the pointer did:

```rust
let response = server.desktop.pointer(mouse.x, mouse.y, mouse.buttons);
server.apply(response);
```

and gets back a `Response`:

```rust
pub struct Response {
    pub damage: Damage,                        // parts of the screen to draw again
    pub deliver: Option<(Id, i32, i32, u8)>,   // a click for this window's program
    pub focus: Option<(Option<Id>, Id)>,       // the focus moved, from, to
    pub moved: Option<(Id, i32, i32)>,         // a drag ended, the frame is here now
    pub close: Option<Id>,                     // this window's close box was clicked
}
```

The rules:

- **A press on a window raises it and gives it the focus.** Click-to-focus, not focus-follows-mouse: the keys go where the user last clicked, and moving the mouse away while typing does not send the rest of a word somewhere else.
- **A press on its title bar starts a drag.** The window follows the pointer, keeping the pointer at the same spot of its frame, until the button is let go. Where it may go is limited: the title bar always stays on the screen, with at least 48 pixels of it showing across. A window can be pushed mostly out of sight, but never out of reach.
- **A press on its close box asks the program to close it**, but only if the button is let go over the same box. Pressing, moving off and letting go takes it back, as on every desktop.
- **A press in its content goes to the program, and so does everything the pointer does until every button is up.** This is a *grab*. Without it, dragging the thumb of a scroll bar and leaving the window by a pixel would stop the drag.

Each window is its *content* (what the program draws) inside a *frame* the server draws: a one-pixel border, and a 22-pixel title bar with the title and, if the window may be closed, a close box. The terminal has no close box; programs' windows do.

New windows are placed in a cascade: the first at (200, 150), each next one 32 pixels further down and right, pushed back if it would leave the screen.

## Composing the screen

When something changes, the server draws again only the rectangle that changed. It draws it bottom to top:

1. the desktop: the amber band with the system's name, and the gradient (a function of y alone, so any piece of it can be drawn on its own);
2. every window that reaches into the rectangle, bottom first: its frame, then its content copied from its pixels;

and does all of this into a **back buffer**, a 3 MB copy of the screen in memory, before copying the rectangle to the framebuffer in one go. Drawing straight onto the screen, the desktop would show for a moment where a window is about to be painted over it: visible flicker, worst when dragging. With a back buffer, the screen only ever receives finished pixels.

To draw only inside the rectangle, `gfx` gained `Surface::region`: a view of part of a surface that is itself a surface, with its own (0, 0). Every drawing call already clips to the surface it draws on, so drawing the whole desktop and every frame on a region draws only the part inside it, with no clipping code in the server at all.

There is one shortcut. If the top window that reaches into the rectangle covers all of it with its content, nothing under it can show there, so only that window's pixels are copied. This is the common case: the terminal scrolling, or a program redrawing a button. It makes a full screen of text cost one copy instead of the desktop, every frame and a copy.

The pointer stays as in step A: its arrow is lifted off the framebuffer before anything is copied to it and put back after, so the back buffer never contains it.

## Where keys go

Before this step, the keyboard drivers sent their bytes straight to the shell. Now the drivers' channel ends at the window server, and the server passes each message on to the window with the focus:

```rust
fn keys(&mut self, bytes: &[u8]) {
    match self.desktop.focus().and_then(|id| self.client(id)) {
        Some(client) => client.send(Event::Key(bytes)),
        None => { let _ = taaj::send(self.shell_keys, bytes, &[]); } // the terminal
    }
}
```

The shell still finds "keys" by asking the registry. What it gets by that name is now the far end of a channel the window server writes to, instead of the drivers' channel. It cannot tell the difference, and neither can the programs it gives its keyboard to (the C programs that read standard input, from entry 043). init decides this in one line, and only where there is a screen:

```rust
("keys", if serial_terminal { 4 } else { 40 }),
```

Where the terminal is the serial port, the keys go straight to the shell as before, and there is no window server at all. Its channel's far end is closed, so a program that asks for a window is told at once instead of waiting.

## The toolkit: `gui::toolkit`

Small, on purpose:

- **`Theme`**: the colours, as pixels of the screen's format. None of them is pure white (the pointer's inside) or the terminal's pale grey-blue text, because the boot tests find the arrow and count the terminal's text by those colours. A host test checks that.
- **`draw_frame`**: a window's border, title bar, title (cut before the close box) and close box. The server's code, here so that it is tested.
- **`Label`**: text in a rectangle, cut at its edges.
- **`Button`**: a rectangle with a word on it. `pointer(x, y, buttons)` says whether the report changed how it looks (pressed or not) or completed a click: pressed over it, and let go over it.

```rust
match button.pointer(x, y, buttons) {
    Press::None => {}
    Press::Changed => counter.draw_button(which),
    Press::Clicked => { counter.draw_button(which); counter.add_one(); }
}
```

## counter

`counter` is a disk program, given "windows" by the shell, as `httpd` is given "net". It opens a 240 x 110 window with a label and two buttons. `+1`, or the `+` key typed while it has the focus, adds one; `quit`, or the close box, ends it. It logs every key it is given to the kernel's log (its standard output is the terminal, a window the test does not read), so a test can see that keys typed into it reach it and nothing else.

## Design decisions

| Decision | Chosen | Rejected, and why |
|---|---|---|
| Who manages windows | The window server itself (compositor and window manager in one process) | A separate window manager process, as in X11: every move and every focus change would be a conversation between three processes |
| A window's pixels | Memory the program creates and shares read-only; DAMAGE says what changed | Pixels in messages: hundreds of kernel copies per frame. Drawing commands sent to the server (as X11 does): a big protocol, and every new kind of drawing a change to the server |
| Composing | Into a back buffer, then one copy to the screen | Straight onto the screen: the desktop flickers through where windows are being drawn |
| The terminal | A window drawn by the server from the grid it already kept | A separate terminal program: the cleaner design, but the grid kept across a crashed display (entry 027) would have to move with it. Later |
| Keys | Drivers to the server, the server to the focused window | Drivers to the focused window: they would have to know about windows and focus |
| Focus | Click to focus | Focus follows the mouse: the rest of a word goes elsewhere when the mouse is nudged |
| Closing | The close box asks; closing the channel closes | The server closes on a click: a program with unsaved work could not ask first. A CLOSE message from the program: a crashed program could not send it |
| Pixel format | The screen's, told in OPENED | A fixed format converted by the server: a conversion per pixel per frame, for screens that QEMU does not have |
| Placement | A cascade | Programs choosing: they do not know what else is on the screen |
| How many windows | 8 (the terminal and 7) | More: the server waits on every window's channel at once, and the kernel's limit is 32 handles per wait. 8 is plenty for this step |
| Where a press goes after it leaves its window | Still to its window, until all buttons are up (a grab) | To whatever is under the pointer: a drag that leaves the window by a pixel would stop |

## Bugs

- **A press that began beside a button, moved onto it, and was let go there counted as a click.** The button kept whether it was "held", but not whether the left button had *just* gone down, so every report with the button down could start a press. The host test `a_button_clicks_only_if_let_go_over_it` failed on that case before anything ran on a machine. The button now remembers the left button's previous state, and only the report where it goes down can start a press.
- **The `services` check of three boot tests failed.** It expects the exact list of names the shell may ask for, and that list now ends with `windows`. The test was right to fail: the policy changed, and it says so.
- **Found by reading, not seen: a screen with no keyboard would have had two readers of the screen's text.** Where AArch64 has `ramfb` but no virtio keyboard, the terminal is the serial port, and the serial driver reads the display's channel to print the shell's text. Since step A the display service ran there too, and read the same channel; each message would have gone to one or the other. The display is now not started where the terminal is the serial port. I did not boot that combination to see it go wrong.
- **In my test script: the pointer's position kept in a subshell.** The PC's mouse moves are relative, so the script keeps where the pointer is and works out each move. My first version called that function inside `$( )`, a subshell, so every position it updated was thrown away when the subshell ended. I saw it while reading the script, before the first run. The function now leaves its moves in a variable, and runs in the script's own shell.

## How I checked it

Automated, on the host:

- `gui`: 22 tests.
  - The protocol: every message there and back; every malformed one refused.
  - The desktop: a window's parts; the cascade, and windows pushed back onto the screen; the limit of 8; a click that raises, focuses and is delivered; the grab; dragging, with damage where the window was and is; the title bar kept within reach; the close box only when let go over it; the focus handed on when a window closes; a drag or grab ended by a close; titles.
  - The toolkit: no colour like the pointer's or the terminal's text; a frame drawn round the content and never on it; a long title stopping before the close box; the button's click rules; labels cut at their edge.
- `gfx`: 10 tests (2 new: `Rect::union`, and `Surface::region` never drawing outside itself).

Automated, by booting: `make gui64-test` and `make aarch64-gui-test`, both added to `make test64`. One boot each does everything in the log at the top. counter's window opens where expected and gets the focus. A click on `+1` counts. `+` and `a` reach counter and not the shell. A click on the terminal gives it the focus back, and the shell then gets exactly `echo back`. The terminal is dragged by its title bar to (519, 330). counter's close box asks it to close, it ends with code 0, and its window goes. Then the shell gets `echo done`. The screenshot is checked at three pixels (the terminal's amber title bar where it was dragged, the desktop where counter was, the desktop where the terminal was) and for the arrow on the close box, and nowhere else.

Every older boot test passes with the window server in place: the watchdog tests stall the display, restart it and find the terminal's text kept; the screen checks still find the band, the gradient and the text; the UEFI boot's 1280 x 800 screen gets a terminal sized to it.

Mutation experiments, host: 9 mutants, all caught, each by a named test:
- a focus change not redrawing the old title bar;
- a press in the content not grabbing;
- a dragged window not kept within reach;
- closing the focused window handing the focus to no one;
- the close box firing wherever the button is let go;
- raising a window without redrawing it;
- DAMAGE read with width and height swapped;
- the button forgetting the left button's last state (the bug above);
- `region` starting at the row instead of the pixel.

Mutation experiments, by booting: 4 mutants of the window server, all caught by `make gui64-test`.
- Keys always sent to the shell: `+` never reached counter.
- No focus events: counter was never told it lost the focus.
- A closed window left in the desktop's list: every line in the log was still right - the keys even reached the shell, because the server had no program left to send them to - and only the screenshot caught it: the terminal's title bar never turned back to the focused colour.
- The compositor's shortcut taken when the top window only touches the rectangle: the terminal's old title bar was left on the desktop where it had been dragged from.

By hand: the screenshots of both test boots, looked at (the terminal dragged half off the screen, the desktop clean where both windows were).

Not tested:
- A real hand on a mouse, in QEMU's window: everything was driven through QEMU's monitor and QMP.
- More than one program's window at once (the rules for several windows are tested on the host only).
- A program that never reads its events: the server's sends to it fail when its queue is full, and the events are dropped; not tried.
- Opening a window with too little memory, or refused because 8 are open: the server's checks exist, and the refusal path is tested on the host, but no boot does it.
- A display restart with a program's window open: the program should see its channel closed and end.

## What is still missing

- The terminal as a program of its own, so that the shell's output is one more client.
- Resizing windows, minimising, and any way to bring back a window that is completely hidden behind another (there is no task bar).
- Moving the pointer over a window tells its program nothing: no hover, and no cursor shapes.
- Keys go to a window as terminal bytes. An editor will want key presses and releases, and Shift and Control on their own.
- Copy and paste between windows.
- Everything is redrawn by copying: no hardware acceleration, and no double buffering per window (a program drawing while the server copies can show a torn frame for one update).

## What I learned

- A window system is mostly bookkeeping. The drawing is a few loops; deciding what a press means (raise, focus, drag, close, or the program's, and for how long) is where the rules are, and they fit in one struct tested on the host.
- Shared memory with rights is the right shape for pixels: the program writes, the server reads, and the kernel makes "only reads" a fact rather than a promise.
- Closing the channel as the way to close a window makes crashes clean for free.
- A back buffer trades 3 MB of memory for no flicker. And drawing through a clipped view (`region`) meant the compositor needed no clipping code of its own.
- Routing keys through the server needed no change in the drivers or the shell, only in what init hands out by name. The registry from entry 034 paid for itself again.
- A host test is cheapest when it finds a bug before anything boots: the button's click rule was wrong on its first run.

## Testing note

**1. The PC: windows**

```
make run64
```

Click inside the QEMU window so that it takes the mouse (**Ctrl+Alt+G** gives it back to the host).

| Type / do | You should see |
|---|---|
| (at boot) | the amber band, and below it a window with the title bar "terminal", grey-blue text on navy, the shell's prompt in it; in the terminal you started QEMU from: `[display] window 1 "terminal" opened, 984x632 at (19, 80)` |
| `counter &` | `[1] counter`; a small window "counter" opens over the terminal, at (200, 150), with an amber title bar and a red close box, `clicks: 0` and two buttons, `+1` and `quit`; the terminal's title bar turns grey-blue: the new window has the focus |
| click `+1` | the button looks pushed in while held; `clicks: 1` when let go |
| press on `+1`, move off it, let go | nothing counts |
| type `+` | `clicks: 2`: keys go to the window with the focus. In the log: `[counter] key +` |
| type `a` | nothing on the screen; in the log: `[counter] key a`, and the shell did not get it |
| click anywhere on the terminal | the terminal comes to the front and its title bar turns amber; counter's turns grey-blue (it is behind the terminal now) |
| `echo back` | `back`: the keys are the shell's again |
| drag the terminal by its title bar | the terminal follows the pointer; where it was, the desktop shows again. It stops with part of its title bar still on the screen, however far you drag. In the log, when you let go: `[display] window 1 moved to (x, y)` |
| click counter's title bar (drag the terminal aside first if it covers it) | counter comes to the front |
| click counter's close box | `[display] window 2: its program is asked to close it`, `[counter] closed by its close box`; the window goes, and the terminal has the focus again |
| `counter &`, then click `quit` | `[counter] quit`, and the window goes |

**2. AArch64: the same, in a window of QEMU's**

```
make run-aarch64-screen
```

The arrow follows the host's pointer over QEMU's window with nothing to grab (a tablet). Every row of 1 applies, from `counter &` on.

(I drove these through QEMU's monitor and QMP, as the automated tests do, and looked at the screenshots; not with a hand on a mouse in a window.)

**3. The tests by themselves**

```
make gui64-test
make aarch64-gui-test
```

| Look for | Meaning |
|---|---|
| `check-pixels: 3 pixels as expected` | the terminal is drawn where it was dragged, and the desktop where it and counter were |
| `check-pointer: the arrow is at (430, 162), 114 pixels of it on the screen, and nowhere else` | the arrow is on what was counter's close box |
| `gui64-test passed` / `aarch64-gui-test passed` | every step: the window opened, clicked, typed into, the focus given back, the terminal dragged, the window closed |

The log of one is the story of the test: `grep -a 'display\]\|counter\]\|shell\] line' build/gui64.log`.

Host checks:

```
cd gui && cargo test           # 22 tests: the protocol, the desktop's rules, the toolkit
cd gfx && cargo test           # 10 tests, Rect::union and Surface::region among them
```

Automated: `make test64`
