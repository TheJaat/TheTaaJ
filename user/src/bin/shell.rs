//! The first shell - for now a line editor: it reads terminal bytes from
//! the keyboard driver, echoes them, handles Backspace, and prints each
//! finished line. It holds one handle, the channel from the driver, and
//! no access to any hardware.

#![no_std]
#![no_main]

use taaj::{print, println, Error};

taaj::program!(main);

const LINE_MAX: usize = 256;

fn main() -> i64 {
    let keys = taaj::bootstrap();
    println!("[shell] ready - type, and each line is echoed back");
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
                println!();
                let text = core::str::from_utf8(&line[..length]).unwrap_or("(not UTF-8)");
                println!("[shell] line: {text:?} (process {})", taaj::process_id());
                length = 0;
            }
            b"\x08" => {
                if length > 0 {
                    // Drop the whole last character, however many bytes.
                    length -= 1;
                    while length > 0 && line[length] & 0xC0 == 0x80 {
                        length -= 1;
                    }
                    print!("\x08 \x08"); // back, blank, back
                }
            }
            [first, ..] if *first < 0x20 => {} // other control keys, escapes: not yet
            text if length + text.len() <= LINE_MAX => {
                line[length..length + text.len()].copy_from_slice(text);
                length += text.len();
                let _ = taaj::write(text);
            }
            _ => {} // line full
        }
    }
}
