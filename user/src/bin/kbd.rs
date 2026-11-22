//! The PS/2 keyboard and mouse driver - in user mode.
//!
//! init starts it with exactly what it needs, as startup handles:
//!   1  the controller's interrupt line (it may wait on it, nothing
//!      more): the keyboard's and the mouse's, which share it
//!   2  I/O port 0x60, the 8042 controller's data port
//!   3  I/O port 0x64, its status/command port
//!   4  a channel to send terminal bytes on (send only)
//!   5  pings from init (abi::PING)
//!   6  the display's channel, to send the mouse's movements on (send
//!      only; abi::display::POINTER)
//! None of them can be duplicated or passed on.
//!
//! It sets the controller up, then loops: wait for the interrupt, read
//! the waiting bytes, and sort them by the controller's status - bit 5
//! says a byte is the mouse's. The keyboard's are decoded into key
//! presses (the `input` crate) and each sent as terminal bytes; the
//! mouse's into movements (`input::pointer`), each sent to the display.
//! If it crashes, the kernel survives, and init starts a new one.
//!
//! F12 is a deliberate bug - the driver reads address 0 - so that the
//! crash and the restart can be tested (and seen).
//!
//! The 8042: status bit 0 says a byte is waiting at 0x60; bit 1 says the
//! controller has not yet taken the last byte written.

#![no_std]
#![no_main]

use abi::display::Pointer;
use input::pointer::{Ps2Mouse, Report};
use input::{terminal_bytes, Decoder, Key};
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
const CONFIG_PORT_2_CLOCK_OFF: u8 = 1 << 5;
const CONFIG_TRANSLATION: u8 = 1 << 6;

/// In the status: the byte waiting at 0x60 is from the second port -
/// the mouse.
const FROM_MOUSE: u8 = 1 << 5;
const ENABLE_PORT_2: u8 = 0xA8;
/// "The next byte written to 0x60 is for the mouse, not the keyboard."
const TO_MOUSE: u8 = 0xD4;
/// Mouse commands, and its answer to each.
const MOUSE_DEFAULTS: u8 = 0xF6;
const MOUSE_REPORT: u8 = 0xF4;
const MOUSE_ACK: u8 = 0xFA;

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

/// Sets the controller up for keyboard interrupts in scancode set 1,
/// and the mouse's if `mouse`. False if no controller answers.
fn init_controller(mouse: bool) -> bool {
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
    if mouse {
        config |= CONFIG_PORT_2_INTERRUPT;
        config &= !CONFIG_PORT_2_CLOCK_OFF;
    }
    if !command(WRITE_CONFIG) || !wait_writable() {
        return false;
    }
    // SAFETY: the configuration byte WRITE_CONFIG asks for.
    unsafe { out8(DATA, config) };
    command(ENABLE_PORT_1) && (!mouse || command(ENABLE_PORT_2))
}

/// Keys pressed while the mouse was being set up: kept, to be decoded
/// once the driver's loop runs. A few at most - setting up takes
/// milliseconds - and any beyond that are lost.
struct Early {
    bytes: [u8; 16],
    count: usize,
}

impl Early {
    fn keep(&mut self, byte: u8) {
        if let Some(slot) = self.bytes.get_mut(self.count) {
            *slot = byte;
            self.count += 1;
        }
    }
}

/// One command to the mouse, and its acknowledgement. False if it does
/// not answer - there is no mouse.
///
/// The controller has one byte to give at a time, from either device.
/// A key pressed just now is a byte in front of the mouse's answer, and
/// the answer comes only once that byte is read: so every byte is read,
/// and the keyboard's are kept (`early`), not mistaken for no answer.
fn tell_mouse(byte: u8, early: &mut Early) -> bool {
    if !command(TO_MOUSE) || !wait_writable() {
        return false;
    }
    // SAFETY: the byte TO_MOUSE announced, sent when the controller is ready.
    unsafe { out8(DATA, byte) };
    for _ in 0..32 {
        if !wait_readable() {
            return false;
        }
        // SAFETY: the status says a byte is there; reading it takes it.
        let (status, data) = unsafe { (in8(STATUS), in8(DATA)) };
        if status & FROM_MOUSE == 0 {
            early.keep(data);
        } else {
            return data == MOUSE_ACK;
        }
    }
    false
}

/// Sets the mouse to its defaults and asks it to report movement.
fn start_mouse(early: &mut Early) -> bool {
    tell_mouse(MOUSE_DEFAULTS, early) && tell_mouse(MOUSE_REPORT, early)
}

fn main() -> i64 {
    let (Some(interrupt), Some(data_port), Some(status_port), Some(output)) =
        (taaj::startup_handle(0), taaj::startup_handle(1), taaj::startup_handle(2), taaj::startup_handle(3))
    else {
        println!("[kbd] no capabilities - nothing to drive");
        return 1;
    };
    if taaj::ioports_enable(data_port).is_err() || taaj::ioports_enable(status_port).is_err() {
        return 2;
    }
    // Pings from init, if it watches this driver (abi::PING); and where
    // the mouse's movements go, if anywhere.
    let mut pings = taaj::startup_handle(4);
    let pointer_out = taaj::startup_handle(5);
    // The mouse is set up only for someone to tell; and the controller
    // set up again without it if it does not answer.
    let mut mouse = pointer_out.is_some();
    if !init_controller(mouse) {
        println!("[kbd] no PS/2 controller answered");
        return 3;
    }
    let mut early = Early { bytes: [0; 16], count: 0 };
    if mouse && !start_mouse(&mut early) {
        mouse = false;
        if !init_controller(false) {
            println!("[kbd] no PS/2 controller answered");
            return 3;
        }
    }
    // (The test scripts wait for this line's start before typing.)
    println!(
        "[kbd] PS/2 keyboard ready (driver in ring 3, process {}){}",
        taaj::process_id(),
        if mouse { "; the mouse too" } else { "" }
    );

    let mut decoder = Decoder::new();
    let mut packets = Ps2Mouse::new();
    let mut early = early.bytes.into_iter().take(early.count);
    loop {
        // Everything waiting first: the interrupt may have fired before
        // this driver started, or several times while it was busy. (And
        // before that, the keys pressed while the mouse was set up.)
        // SAFETY: status has no side effects; data is read only when full.
        loop {
            let (status, byte) = match early.next() {
                Some(byte) => (OUTPUT_FULL, byte),
                None => {
                    let status = unsafe { in8(STATUS) };
                    if status & OUTPUT_FULL == 0 {
                        break;
                    }
                    (status, unsafe { in8(DATA) })
                }
            };
            if status & FROM_MOUSE != 0 {
                if let (Some(Report::Relative { dx, dy, buttons }), Some(out)) = (packets.feed(byte), pointer_out) {
                    let message = Pointer { absolute: false, x: dx, y: dy, buttons }.to_message();
                    // A display that lags loses movements, not the driver.
                    let _ = taaj::send(out, &message, &[]);
                }
                continue;
            }
            let Some(event) = decoder.feed(byte) else { continue };
            if event.pressed && event.key == Key::Function(12) {
                println!("[kbd] F12: simulating a driver bug - reading address 0");
                // SAFETY: none - this is the bug.
                return unsafe { core::ptr::read_volatile(core::ptr::null::<i64>()) };
            }
            if event.pressed && event.key == Key::Function(11) {
                println!("[kbd] F11: simulating a driver bug - looping forever");
                loop {
                    core::hint::spin_loop();
                }
            }
            let Some(bytes) = terminal_bytes(event, decoder.control_held()) else { continue };
            match taaj::send(output, bytes.as_bytes(), &[]) {
                Ok(()) | Err(Error::QueueFull) => {} // a reader that lags loses keys, not the driver
                Err(Error::PeerClosed) => return 0, // nobody listening any more
                Err(_) => return 4,
            }
        }
        // A key, or init asking whether this loop still turns.
        if let Some(channel) = pings {
            match taaj::object_wait(&[interrupt, channel]) {
                Ok(0) => {}
                Ok(_) => {
                    if !taaj::answer_pings(channel) {
                        pings = None;
                    }
                    continue;
                }
                Err(_) => return 5,
            }
        }
        // Pending (or there is nothing else to wait for): takes it.
        if taaj::interrupt_wait(interrupt).is_err() {
            return 5;
        }
    }
}
