//! fs - the file service: files in memory, for whoever holds a channel
//! to it.
//!
//! Startup handles, from init:
//!   1  the request channel (receive only). Every client sends on the
//!      other end; each request carries the end of a channel to reply on.
//!
//! All the logic - the file table, the limits, the message layout - is
//! in the `ramfs` crate, tested on the host. This loop only receives a
//! request, hands it to `Files::serve`, and sends back what it returns.
//!
//! The files live in this process's heap. If the service crashes, init
//! restarts it with the same request channel - waiting requests are
//! still queued there and get answered - but the files are gone: this
//! is a RAM disk. Files on a disk come with the disk driver.

#![no_std]
#![no_main]

use ramfs::protocol::MESSAGE_MAX;
use ramfs::Files;
use taaj::{println, Error};

taaj::program!(main);

fn main() -> i64 {
    let Some(requests) = taaj::startup_handle(0) else {
        println!("[fs] started without a request channel");
        return 1;
    };
    let mut files = Files::new();
    println!(
        "[fs] ready: files in memory, up to {} of {} KiB each, {} MiB in all",
        ramfs::FILES_MAX,
        ramfs::FILE_MAX / 1024,
        ramfs::BYTES_MAX >> 20
    );
    let mut message = [0u8; MESSAGE_MAX];
    let mut reply = [0u8; MESSAGE_MAX];
    loop {
        // Room for the largest message and the most handles one can
        // carry: a message that did not fit would stay at the head of
        // the queue, and be refused again, forever.
        let mut handles = [0u64; abi::MESSAGE_HANDLES_MAX as usize];
        let (n, count) = match taaj::receive(requests, &mut message, &mut handles) {
            Ok(received) => received,
            Err(Error::PeerClosed) => return 0,
            Err(error) => {
                println!("[fs] cannot receive: {error:?}");
                return 2;
            }
        };
        // One handle, the reply channel. Anything else: not a request.
        if count != 1 {
            for &handle in &handles[..count] {
                let _ = taaj::close(handle);
            }
            continue;
        }
        let length = files.serve(&message[..n], &mut reply);
        // A client that gave up has closed its end: nothing to do.
        let _ = taaj::send(handles[0], &reply[..length], &[]);
        let _ = taaj::close(handles[0]);
    }
}
