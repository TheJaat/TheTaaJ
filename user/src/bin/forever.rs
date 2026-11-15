//! forever: a program that does not end - until someone ends it.
//!
//! What a kill has to cope with depends on what the program is doing
//! when it comes, so this does whichever its argument says, forever:
//!
//!   (nothing), spin   computes, in user mode, with no system call -
//!                     only the timer interrupt gets the kernel back
//!   yield             makes system calls in a loop
//!   sleep             sleeps
//!   wait              waits for a message that will never come, on a
//!                     channel it made itself
//!
//! In the shell: run it, then press Ctrl+C. The kernel's kill self-test
//! runs it in every mode.

#![no_std]
#![no_main]

use taaj::println;

taaj::program!(main);

fn main() -> i64 {
    let arguments = taaj::arguments();
    let mode = arguments.trim();
    println!("forever: {} until stopped (Ctrl+C)", if mode.is_empty() { "spin" } else { mode });
    match mode {
        "yield" => loop {
            taaj::yield_now();
        },
        "sleep" => loop {
            taaj::sleep(1000);
        },
        "wait" => {
            // Both ends stay open, so the wait cannot end by itself.
            let Ok((ours, _theirs)) = taaj::channel_create() else { return 1 };
            let error = taaj::receive(ours, &mut [0u8; 8], &mut []);
            println!("forever: the wait ended: {error:?}");
            1
        }
        _ => loop {
            core::hint::spin_loop();
        },
    }
}
