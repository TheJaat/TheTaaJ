//! Last words: what the kernel puts on the screen when it stops.
//!
//! The kernel does not draw. The screen belongs to the display service,
//! and the kernel's own messages go to the serial port. That is right
//! while the system runs - and useless at the one moment it matters
//! most: when the kernel stops, on a machine with nobody listening to a
//! serial port, the screen would simply freeze, showing whatever the
//! display drew last, with no word of why.
//!
//! So for that moment only, the kernel takes the screen back. `begin`
//! paints it over, and from then on everything printed to the kernel
//! console is drawn there as well: the panic message, the exception and
//! its registers.
//!
//! It has to work when little else does - in a panic, in a double fault,
//! with a lock held by the code that died. So everything it needs is
//! got ready at boot (`prepare` maps the framebuffer once), and drawing
//! itself takes no lock, allocates nothing and cannot fail: it writes
//! pixels through a pointer, and stops at the bottom of the screen.

use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use console::font;

use kernel64::bootinfo::Framebuffer;

/// The framebuffer, as mapped by `prepare`. Zero: there is none to use.
static BASE: AtomicU64 = AtomicU64::new(0);
static WIDTH: AtomicUsize = AtomicUsize::new(0);
static HEIGHT: AtomicUsize = AtomicUsize::new(0);
static PITCH: AtomicUsize = AtomicUsize::new(0);
/// The two colours, already in the screen's pixel format.
static PAPER: AtomicU64 = AtomicU64::new(0);
static INK: AtomicU64 = AtomicU64::new(0);

/// Whether the screen has been taken over.
static ACTIVE: AtomicBool = AtomicBool::new(false);
/// Where the next character goes, in cells.
static COLUMN: AtomicUsize = AtomicUsize::new(0);
static ROW: AtomicUsize = AtomicUsize::new(0);

const MARGIN: usize = 16;

/// Maps the framebuffer, for later. Called once, early, while the
/// kernel is well: mapping needs the page tables and their lock.
pub fn prepare(fb: &Framebuffer) {
    // Only the format every machine so far has given: 32 bits a pixel.
    if fb.bpp != 32 || fb.width == 0 || fb.height == 0 || fb.pitch < fb.width * 4 {
        return;
    }
    let pixel = |red: u8, green: u8, blue: u8| -> u64 {
        let channel = |value: u8, size: u8, shift: u8| (u64::from(value) >> (8 - u32::from(size.min(8)))) << shift;
        channel(red, fb.red_size, fb.red_shift) | channel(green, fb.green_size, fb.green_shift) | channel(blue, fb.blue_size, fb.blue_shift)
    };
    PAPER.store(pixel(0x58, 0x10, 0x18), Ordering::Relaxed); // a dark red: not the display's blue
    INK.store(pixel(0xF0, 0xF0, 0xF0), Ordering::Relaxed);
    WIDTH.store(fb.width as usize, Ordering::Relaxed);
    HEIGHT.store(fb.height as usize, Ordering::Relaxed);
    PITCH.store(fb.pitch as usize, Ordering::Relaxed);
    let size = u64::from(fb.pitch) * u64::from(fb.height);
    // Last: a non-zero BASE says all of the above is there.
    BASE.store(crate::mm::map_mmio(fb.address, size), Ordering::Release);
}

fn scale() -> usize {
    if WIDTH.load(Ordering::Relaxed) >= 800 { 2 } else { 1 }
}

fn put(x: usize, y: usize, colour: u64) {
    let base = BASE.load(Ordering::Acquire);
    if base != 0 && x < WIDTH.load(Ordering::Relaxed) && y < HEIGHT.load(Ordering::Relaxed) {
        let address = base as usize + y * PITCH.load(Ordering::Relaxed) + x * 4;
        // SAFETY: inside the framebuffer mapped by `prepare` (checked
        // above). Volatile: device memory.
        unsafe { (address as *mut u32).write_volatile(colour as u32) };
    }
}

/// Takes the screen: paints it over and writes the heading. From now on
/// `echo` draws. Does nothing the second time, or with no screen.
pub fn begin() {
    if BASE.load(Ordering::Acquire) == 0 || ACTIVE.swap(true, Ordering::AcqRel) {
        return;
    }
    let paper = PAPER.load(Ordering::Relaxed);
    for y in 0..HEIGHT.load(Ordering::Relaxed) {
        for x in 0..WIDTH.load(Ordering::Relaxed) {
            put(x, y, paper);
        }
    }
    echo("TheTaaJ has stopped. What the kernel said last:\n");
}

/// Draws `text` at the cursor, if the screen has been taken. Lines
/// wrap; what does not fit below the last line is not drawn.
pub fn echo(text: &str) {
    if !ACTIVE.load(Ordering::Acquire) {
        return;
    }
    let scale = scale();
    let (cell_width, cell_height) = ((font::WIDTH + 1) * scale, (font::HEIGHT + 2) * scale);
    let columns = WIDTH.load(Ordering::Relaxed).saturating_sub(2 * MARGIN) / cell_width;
    let rows = HEIGHT.load(Ordering::Relaxed).saturating_sub(2 * MARGIN) / cell_height;
    let ink = INK.load(Ordering::Relaxed);
    let (mut column, mut row) = (COLUMN.load(Ordering::Relaxed), ROW.load(Ordering::Relaxed));
    for byte in text.bytes() {
        if byte == b'\n' || column >= columns {
            column = 0;
            row += 1;
        }
        if byte == b'\n' || byte == b'\r' || row >= rows {
            continue;
        }
        for y in 0..font::HEIGHT {
            for x in 0..font::WIDTH {
                if font::pixel(byte, x, y) {
                    for (dx, dy) in (0..scale).flat_map(|dx| (0..scale).map(move |dy| (dx, dy))) {
                        put(MARGIN + column * cell_width + x * scale + dx, MARGIN + row * cell_height + y * scale + dy, ink);
                    }
                }
            }
        }
        column += 1;
    }
    COLUMN.store(column, Ordering::Relaxed);
    ROW.store(row, Ordering::Relaxed);
}
