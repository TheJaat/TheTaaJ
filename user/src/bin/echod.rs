//! A tiny service: echoes what it is sent, in capitals.
//!
//! It knows only its bootstrap channel. A message on it is one of:
//!   - text: replied to, upper-cased, on the same channel
//!   - a message carrying a channel handle: a private line from a
//!     client. echod says "welcome" on it and serves it until the client
//!     hangs up, then goes back to the bootstrap channel
//!   - "mem" carrying a memory object handle: echod maps it, upper-cases
//!     the text it finds there - in place, in memory the client also
//!     has mapped - and replies "done"
//!   - "crash": echod dereferences null, as a buggy service would. The
//!     kernel kills it; its clients must survive that.
//!
//! When its bootstrap channel's other end closes, it exits.

#![no_std]
#![no_main]

use taaj::{println, Error, Handle};

taaj::program!(main);

/// Replies to every message on `channel`, upper-cased, until the peer
/// closes it.
fn serve(channel: Handle) -> Result<(), Error> {
    let mut buffer = [0u8; 256];
    loop {
        let (length, _) = match taaj::receive(channel, &mut buffer, &mut []) {
            Err(Error::PeerClosed) => return Ok(()),
            other => other?,
        };
        buffer[..length].make_ascii_uppercase();
        taaj::send(channel, &buffer[..length], &[])?;
    }
}

fn main() -> i64 {
    let boot = taaj::bootstrap();
    let mut buffer = [0u8; 256];
    let mut handles = [0u64; 1];
    loop {
        let (length, count) = match taaj::receive(boot, &mut buffer, &mut handles) {
            Ok(sizes) => sizes,
            Err(Error::PeerClosed) => return 0,
            Err(_) => return 1,
        };
        if count == 1 && &buffer[..length] == b"mem" {
            let memory = handles[0];
            let Ok(base) = taaj::memory_map(memory, true) else { return 4 };
            // The text is at the start of the third page, ending at a 0.
            // SAFETY: the object is at least three pages (the client says
            // so); the scan stops inside the third.
            unsafe {
                let text = base.add(2 * 4096);
                let mut n = 0;
                while n < 4096 && text.add(n).read_volatile() != 0 {
                    text.add(n).write_volatile(text.add(n).read_volatile().to_ascii_uppercase());
                    n += 1;
                }
            }
            println!("[echod] upper-cased text in shared memory, in place");
            if taaj::memory_unmap(base).is_err() || taaj::close(memory).is_err() {
                return 5;
            }
            if taaj::send(boot, b"done", &[]).is_err() {
                return 3;
            }
        } else if count == 1 {
            let private = handles[0];
            println!("[echod] a client opened a private channel");
            if taaj::send(private, b"welcome", &[]).and_then(|()| serve(private)).is_err() {
                return 2;
            }
            let _ = taaj::close(private);
        } else if &buffer[..length] == b"crash" {
            println!("[echod] asked to crash: reading address 0");
            // SAFETY: none - this is the bug being simulated.
            let value = unsafe { core::ptr::read_volatile(core::ptr::null::<u64>()) };
            return value as i64;
        } else {
            buffer[..length].make_ascii_uppercase();
            if taaj::send(boot, &buffer[..length], &[]).is_err() {
                return 3;
            }
        }
    }
}
