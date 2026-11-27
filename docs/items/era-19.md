# Era 19 · Desktop: applications people can use daily

*Part of the work list. The rules, who is doing what now, and what waits for the user are in [`docs/who-does-what.md`](../who-does-what.md); the lanes in [`docs/lanes.md`](../lanes.md).*


**Done when**: a day's ordinary work can be done on TheTaaJ's desktop with no terminal - open, edit and save files in applications, copy and paste between them, type in a language other than English, find and start programs from a panel, resize and minimise windows, lock the screen and unlock it as one's own user - with key-to-screen time under the limit the benchmark sets. Whose it is, and when, is for the user to say. The Desktop items above (K39-K46) are pointed to, not moved; K108 (Era 18) comes before the screen lock.

What is right, and close to Wayland with capabilities in place of a socket: a program opens a window on the registry's `windows` with a channel and a memory object, draws into its own memory and sends DAMAGE; pixels never travel in messages; closing the channel closes the window, so a crash cleans up by itself. The server draws every frame and title, so no program can fake another's title bar, and keys go only to the window with the focus. The screen is composed in a back buffer, only damaged rectangles again. The window manager (`gui::desktop`), drawing (`gfx`), the editor's logic (`gui::text`) and the protocol (`gui::protocol`) are all tested on the host.

What it is not yet: a desktop. `display` is four things in one process - compositor, window manager, terminal and console - so a bug in one takes the screen. Keys reach programs as terminal bytes (no key codes, no key up, no lone Shift). The font is the console's ASCII bitmap at one size. No resize, minimise, panel, launcher or clipboard. Two programs open windows: `counter` and `edit`. The toolkit is a frame, text and buttons. Composition is the CPU's alone, at the boot framebuffer's resolution, with no scaling. Any program granted `windows` may open as many as it likes, anywhere and full-screen - a fake login screen included - and nothing on the keyboard always reaches the system. No notifications, no accessibility, no image formats, and key-to-screen time never measured.

All of it stays out of the kernel: the compositor, window manager, toolkit, fonts and applications are user-space programs and libraries; the kernel's part is memory objects and channels, which exist.

| # | Item | Size | Notes |
|---|---|---|---|
| K114 | **A desktop benchmark**: key to screen with four busy programs, frames per second dragging a window, the cost of one composition at the screen's size | S | **Done (entry 147)**: measured inside `display` (`display bench`, `display keys N`), on 1 CPU and 4 by `deskbench64-test`. Left: AArch64; the keyboard driver's part of key to screen (with K116) |
| K115 | **`display` split**: the compositor, the window manager, the terminal (K44) and the boot console as programs of their own; a crash in the terminal leaves the screen | M | Early: everything else builds on it |
| K116 | **Real key events** through the window protocol: key codes, down and up, modifiers, repeat, a keyboard layout chosen in user space; the terminal turns them into bytes for programs that want bytes | M | With K42; Era 11's push-to-talk key needs it |
| K117 | **Text for every language**: UTF-8 drawn with a TrueType rasteriser (our own, or a port), several sizes, fallback fonts; left-to-right first | L | With K43 |
| K118 | **A toolkit to build applications with**: layout, text fields, lists, menus, scroll bars, dialogs, keyboard focus between widgets; its logic tested on the host as `gui` is | L | After K116 and K117 |
| K119 | **A panel**: a launcher, the open windows, a clock; windows resized, minimised and maximised | M | With K40 |
| K120 | **Copy and paste**: a clipboard service that hands a memory object from the copying program to the pasting one, only on the user's keys | S | With K41 |
| K121 | **The first applications**: a terminal window, a file manager, settings, an image viewer, a calculator; `edit` on the new toolkit | L | After K118; the file manager after Era 14's handles |
| K122 | **PNG and JPEG** decoders as libraries in user space, fuzzed (K104) | M | For K121' image viewer and icons |
| K123 | **virtio-gpu**: 2D first, scanout and vsync; the compositor using it where it is there | M | After K81. Real GPUs much later |
| K124 | **The screen's native resolution and scaling** for big and HiDPI screens | M | With K39 |
| K125 | **A trusted path**: a key combination that always reaches the system, whatever has the focus; a screen lock; limits on how many windows a program opens and whether it may cover the screen | M | The lock after K108 |
| K126 | **Notifications**, and a window asking for attention without taking the focus | S | |
| K127 | **Accessibility**: everything reachable from the keyboard, high contrast, zoom; a screen reader with Era 11's voice | M | |
