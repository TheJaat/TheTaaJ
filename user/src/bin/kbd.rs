//! The PS/2 keyboard driver - in user mode.
//!
//! The kernel gives it, in its bootstrap message, exactly what it needs:
//!   0  the keyboard's interrupt line (it may wait on it, nothing more)
//!   1  I/O port 0x60, the 8042 controller's data port
//!   2  I/O port 0x64, its status/command port
//!   3  a channel to send terminal bytes on (send only)
//!
//! It sets the controller up, then loops: wait for the interrupt, read
//! the waiting bytes, decode them into key presses (the `input` crate),
//! and send each press as terminal bytes. If it crashes, the kernel
//! survives - the keyboard just stops, until something restarts it.
//!
//! The 8042: status bit 0 says a byte is waiting at 0x60; bit 1 says the
//! controller has not yet taken the last byte written.

#![no_std]
#![no_main]

use input::{terminal_bytes, Decoder};
use taaj::port::{in8, out8};
use taaj::{println, Error};

taaj::program!(main);

const DATA: u16 = 0x60;
const STATUS: u16 = 0x64;
const COMMAND: u16 = 0x64;

const OUTPUT_FULL: u8 = 1 << 0;
const INPUT_FULL: u8 = 1 << 1;

const READ_CONFIG: u8 = 0x20;
const WRITE_CONFIG: u8 = 0x60;
const DISABLE_PORT_2: u8 = 0xA7;
const DISABLE_PORT_1: u8 = 0xAD;
const ENABLE_PORT_1: u8 = 0xAE;

const CONFIG_PORT_1_INTERRUPT: u8 = 1 << 0;
const CONFIG_PORT_2_INTERRUPT: u8 = 1 << 1;
const CONFIG_PORT_1_CLOCK_OFF: u8 = 1 << 4;
const CONFIG_TRANSLATION: u8 = 1 << 6;

/// A missing controller never sets its status bits; give up after this
/// many polls instead of hanging.
const PATIENCE: u32 = 100_000;

fn wait_writable() -> bool {
    // SAFETY: reading the status port has no side effects.
    (0..PATIENCE).any(|_| unsafe { in8(STATUS) } & INPUT_FULL == 0)
}

fn wait_readable() -> bool {
    // SAFETY: as above.
    (0..PATIENCE).any(|_| unsafe { in8(STATUS) } & OUTPUT_FULL != 0)
}

fn command(byte: u8) -> bool {
    if !wait_writable() {
        return false;
    }
    // SAFETY: a documented 8042 command, sent when it is ready.
    unsafe { out8(COMMAND, byte) };
    true
}

/// Sets the controller up for keyboard interrupts in scancode set 1.
/// False if no controller answers.
fn init_controller() -> bool {
    // Quiet both ports while reconfiguring, and drop any stale byte.
    if !command(DISABLE_PORT_1) || !command(DISABLE_PORT_2) {
        return false;
    }
    // SAFETY: draining the data port only discards bytes.
    while unsafe { in8(STATUS) } & OUTPUT_FULL != 0 {
        let _ = unsafe { in8(DATA) };
    }
    if !command(READ_CONFIG) || !wait_readable() {
        return false;
    }
    // SAFETY: the controller has answered READ_CONFIG.
    let mut config = unsafe { in8(DATA) };
    config |= CONFIG_PORT_1_INTERRUPT | CONFIG_TRANSLATION;
    config &= !(CONFIG_PORT_2_INTERRUPT | CONFIG_PORT_1_CLOCK_OFF);
    if !command(WRITE_CONFIG) || !wait_writable() {
        return false;
    }
    // SAFETY: the configuration byte WRITE_CONFIG asks for.
    unsafe { out8(DATA, config) };
    command(ENABLE_PORT_1)
}

fn main() -> i64 {
    let mut name = [0u8; 32];
    let mut handles = [0u64; 4];
    match taaj::receive(taaj::bootstrap(), &mut name, &mut handles) {
        Ok((_, 4)) => {}
        _ => {
            println!("[kbd] no capabilities - nothing to drive");
            return 1;
        }
    }
    let [interrupt, data_port, status_port, output] = handles;
    if taaj::ioports_enable(data_port).is_err() || taaj::ioports_enable(status_port).is_err() {
        return 2;
    }
    if !init_controller() {
        println!("[kbd] no PS/2 controller answered");
        return 3;
    }
    println!("[kbd] PS/2 keyboard ready (driver in ring 3, process {})", taaj::process_id());

    let mut decoder = Decoder::new();
    loop {
        // Everything waiting first: the interrupt may have fired before
        // this driver started, or several times while it was busy.
        // SAFETY: status has no side effects; data is read only when full.
        while unsafe { in8(STATUS) } & OUTPUT_FULL != 0 {
            let byte = unsafe { in8(DATA) };
            let Some(event) = decoder.feed(byte) else { continue };
            let Some(bytes) = terminal_bytes(event, decoder.control_held()) else { continue };
            match taaj::send(output, bytes.as_bytes(), &[]) {
                Ok(()) | Err(Error::QueueFull) => {} // a reader that lags loses keys, not the driver
                Err(Error::PeerClosed) => return 0, // nobody listening any more
                Err(_) => return 4,
            }
        }
        if taaj::interrupt_wait(interrupt).is_err() {
            return 5;
        }
    }
}
