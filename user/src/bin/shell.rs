//! The first shell - for now a line editor: it reads terminal bytes from
//! the keyboard driver, echoes them, handles Backspace, and prints each
//! finished line.
//!
//! Startup handles, from init:
//!   1  the channel from the keyboard driver (receive only)
//!   2  the channel to the display (send only) - may be absent
//!
//! Output goes to the screen, through the display service, and to the
//! kernel's debug log (the serial line), which is what the boot tests
//! read. No access to any hardware.

#![no_std]
#![no_main]

use taaj::{println, Error, Handle};

taaj::program!(main);

const LINE_MAX: usize = 256;
const PROMPT: &[u8] = b"taaj> ";

/// Where output goes: the serial line, and the screen if there is one.
struct Output {
    screen: Option<Handle>,
}

impl Output {
    fn write(&self, bytes: &[u8]) {
        let _ = taaj::write(bytes);
        self.screen_only(bytes);
    }

    fn screen_only(&self, bytes: &[u8]) {
        let Some(screen) = self.screen else { return };
        let mut message = [0u8; 128];
        for chunk in bytes.chunks(message.len() - 1) {
            message[0] = abi::display::TEXT;
            message[1..=chunk.len()].copy_from_slice(chunk);
            // A display that lags or is restarting loses some text, and
            // the shell carries on.
            let _ = taaj::send(screen, &message[..=chunk.len()], &[]);
        }
    }
}

fn main() -> i64 {
    let keys = taaj::bootstrap();
    let out = Output { screen: taaj::startup_handle(1) };
    println!("[shell] ready - type, and each line is echoed back");
    out.screen_only(b"Welcome to TheTaaJ. Type a line; it is echoed back.\n");
    out.screen_only(b"Every key travels: keyboard -> kbd (ring 3) -> shell -> display (ring 3).\n\n");
    out.screen_only(PROMPT);

    let mut line = [0u8; LINE_MAX];
    let mut length = 0;
    let mut input = [0u8; 16];
    loop {
        let n = match taaj::receive(keys, &mut input, &mut []) {
            Ok((n, _)) => n,
            Err(Error::PeerClosed) => {
                println!("[shell] the keyboard driver is gone");
                return 0;
            }
            Err(_) => return 1,
        };
        // The driver sends one key per message: a character (as UTF-8),
        // a control code, or an escape sequence.
        match &input[..n] {
            b"\n" => {
                out.write(b"\n");
                let text = core::str::from_utf8(&line[..length]).unwrap_or("(not UTF-8)");
                println!("[shell] line: {text:?} (process {})", taaj::process_id());
                if length > 0 {
                    out.screen_only(b"you typed: ");
                    out.screen_only(&line[..length]);
                    out.screen_only(b"\n");
                }
                out.screen_only(PROMPT);
                length = 0;
            }
            b"\x08" => {
                if length > 0 {
                    // Drop the whole last character, however many bytes.
                    length -= 1;
                    while length > 0 && line[length] & 0xC0 == 0x80 {
                        length -= 1;
                    }
                    out.write(b"\x08 \x08"); // back, blank, back
                }
            }
            [first, ..] if *first < 0x20 => {} // other control keys, escapes: not yet
            text if length + text.len() <= LINE_MAX => {
                line[length..length + text.len()].copy_from_slice(text);
                length += text.len();
                out.write(text);
            }
            _ => {} // line full
        }
    }
}
