//! The serial port's driver: the keyboard and the screen of a machine
//! that has neither - in user mode.
//!
//! QEMU's AArch64 machine, started without a window, has one way in and
//! out: a serial port, an Arm PL011. What is typed at the terminal on
//! its other end arrives there a byte at a time; what the system says
//! leaves through it. So one program does here what two do on the PC -
//! `kbd` and `display` - and speaks both their protocols, so that init,
//! the shell and every program are the same on both machines.
//!
//! Startup handles, from init:
//!   1  the port's interrupt line (it may wait on it, nothing more)
//!   2  the memory its registers are in
//!   3  a channel to send terminal bytes on - the keys
//!   4  a channel of display messages (abi::display): TEXT to show
//!   5  a channel of pings from init (abi::PING)
//!
//! In: the port interrupts when bytes have arrived. They are read out,
//! turned into what a keyboard would have sent (`input::serial` - Enter
//! is a carriage return on the wire, and escape sequences are kept
//! whole), and sent on. If whoever reads the keys is not keeping up,
//! the bytes are left in the port, which then stops taking more: the
//! far end waits. A keyboard's driver has to drop keys; this one never
//! does.
//!
//! Out: this driver does not write to the port. The kernel does: its
//! log goes out of the same port, and two writers of one transmit
//! register would lose each other's bytes. Text for the screen is
//! handed to the kernel's log instead (`debug_write`). On this machine
//! the log *is* the screen.
//!
//! F12 and F11 are deliberate bugs, as in `kbd`: the driver reads
//! address 0, or loops for ever - so that the crash, the hang and the
//! restart can be tested.

#![no_std]
#![no_main]

use input::serial::{Bytes, Decoder, Input};
use taaj::{println, Error, Handle};

taaj::program!(main);

// The PL011's registers, as byte offsets; each 32 bits.
const DATA: usize = 0x00;
const FLAGS: usize = 0x18;
const INTERRUPT_MASK: usize = 0x38;
const INTERRUPT_CLEAR: usize = 0x44;

/// Flag: nothing has arrived that has not been read.
const RECEIVE_EMPTY: u32 = 1 << 4;
/// Interrupt: the receive queue has filled to its mark.
const RECEIVED: u32 = 1 << 4;
/// Interrupt: bytes have sat in the receive queue for a while without
/// reaching the mark. Without this one a single typed letter would
/// wait for more to arrive.
const RECEIVE_TIMEOUT: u32 = 1 << 6;

/// How long an ESC is held for the rest of its sequence, in ticks.
const SEQUENCE_WAIT: u64 = 3;
/// How soon a key is offered again to a reader that had no room.
const RETRY_WAIT: u64 = 1;

/// After how many interrupts the driver says that they arrive.
const INTERRUPTS_SAID: u32 = 8;

struct Port(*mut u32);

impl Port {
    fn read(&self, register: usize) -> u32 {
        // SAFETY: a register of the PL011 this process was given, inside
        // the page it mapped. Volatile: reading the data register takes
        // a byte out of the queue.
        unsafe { self.0.byte_add(register).read_volatile() }
    }

    fn write(&self, register: usize, value: u32) {
        // SAFETY: as for `read`.
        unsafe { self.0.byte_add(register).write_volatile(value) }
    }

    /// The next byte that has arrived, if any.
    fn next(&self) -> Option<u8> {
        (self.read(FLAGS) & RECEIVE_EMPTY == 0).then(|| self.read(DATA) as u8)
    }
}

/// Shows `text` (terminal bytes, as the display takes them). A form
/// feed clears the PC's screen; a terminal is asked to in its own way.
fn show(text: &[u8]) {
    for piece in text.split_inclusive(|&byte| byte == 0x0C) {
        match piece.split_last() {
            Some((0x0C, before)) => {
                let _ = taaj::debug_write(before);
                let _ = taaj::debug_write(b"\x1b[2J\x1b[H");
            }
            _ => {
                let _ = taaj::debug_write(piece);
            }
        }
    }
}

fn main() -> i64 {
    let (Some(interrupt), Some(registers), Some(keys), Some(text)) =
        (taaj::startup_handle(0), taaj::startup_handle(1), taaj::startup_handle(2), taaj::startup_handle(3))
    else {
        println!("[serial] no capabilities - nothing to drive");
        return 1;
    };
    let mut pings = taaj::startup_handle(4);
    let port = match taaj::memory_map(registers, true) {
        Ok(page) => Port(page.cast()),
        Err(error) => {
            println!("[serial] cannot map the port's registers: {error:?}");
            return 2;
        }
    };
    // Interrupts for bytes arriving, and for nothing else: the kernel
    // set the line up (speed, queues) and still sends on it.
    // (Only the timeout is ever cleared by hand: the other clears
    // itself when the queue has been read empty, and must stay up
    // while it has not.)
    port.write(INTERRUPT_CLEAR, RECEIVE_TIMEOUT);
    port.write(INTERRUPT_MASK, RECEIVED | RECEIVE_TIMEOUT);
    println!("[serial] serial port ready: keys in, text out (driver in user mode, process {})", taaj::process_id());

    // How often the port's interrupt was taken. Said once, a little
    // way in: a driver that only ever looked at the port when something
    // else woke it would still work, slowly, and nothing would show it.
    let mut interrupts = 0u32;
    let mut decoder = Decoder::new();
    let mut message = [0u8; abi::MESSAGE_MAX as usize];
    // A key that could not be sent yet: its reader's queue was full.
    let mut unsent: Option<Bytes> = None;
    // Sends one key. Ok(false): no room - the reader is busy.
    let send = |bytes: &Bytes| match taaj::send(keys, bytes.as_bytes(), &[]) {
        Ok(()) => Ok(true),
        Err(Error::QueueFull) => Ok(false),
        Err(Error::PeerClosed) => Err(0), // nobody listening any more
        Err(_) => Err(4),
    };
    loop {
        // Everything that has arrived, first: before the driver waits
        // again the port must have nothing left to interrupt about -
        // its line stays raised for as long as it has.
        //
        // Unless the reader is not keeping up. `kbd` then drops keys: a
        // keyboard cannot be told to wait. A serial line can. What is
        // not read here stays in the port's queue; when that is full
        // the port stops taking bytes, and the other end holds on to
        // them. Nothing typed is lost, however long the shell is busy.
        port.write(INTERRUPT_CLEAR, RECEIVE_TIMEOUT);
        if let Some(bytes) = unsent.take() {
            match send(&bytes) {
                Ok(true) => {}
                Ok(false) => unsent = Some(bytes),
                Err(code) => return code,
            }
        }
        while unsent.is_none() {
            let Some(byte) = port.next() else { break };
            match decoder.feed(byte) {
                Input::Nothing => {}
                Input::Bytes(bytes) => match send(&bytes) {
                    Ok(true) => {}
                    Ok(false) => unsent = Some(bytes),
                    Err(code) => return code,
                },
                Input::Function(12) => {
                    println!("[serial] F12: simulating a driver bug - reading address 0");
                    // SAFETY: none - this is the bug.
                    return unsafe { core::ptr::read_volatile(core::ptr::null::<i64>()) };
                }
                Input::Function(11) => {
                    println!("[serial] F11: simulating a driver bug - looping forever");
                    loop {
                        core::hint::spin_loop();
                    }
                }
                Input::Function(_) => {}
            }
        }

        // A key, text to show, or init asking whether this loop still
        // turns. With half an escape sequence in hand, not for long.
        // And with a key unsent, not for the port at all - it has more,
        // and would say so at once, again and again: for the other two
        // only, and soon to try the reader again.
        let others: [Handle; 2] = [text, pings.unwrap_or(text)];
        let all: [Handle; 3] = [interrupt, others[0], others[1]];
        let now = taaj::ticks();
        let ready = match unsent {
            Some(_) => taaj::object_wait_until(&others, now + RETRY_WAIT).map(|ready| ready.map(|index| index + 1)),
            None => taaj::object_wait_until(&all, if decoder.holding() { now + SEQUENCE_WAIT } else { u64::MAX }),
        };
        match ready {
            Ok(Some(0)) => {
                // Taken, so that the next one can come. The bytes are
                // read at the top of the loop.
                if taaj::interrupt_wait(interrupt).is_err() {
                    return 5;
                }
                interrupts += 1;
                if interrupts == INTERRUPTS_SAID {
                    println!("[serial] {interrupts} interrupts taken: the port says when it has something");
                }
            }
            Ok(Some(1)) => match taaj::receive(text, &mut message, &mut []) {
                Ok((n, _)) if n >= 1 && message[0] == abi::display::TEXT => show(&message[1..n]),
                Ok((n, _)) if n == 9 && message[0] == abi::display::STALL => {
                    let ticks = u64::from_le_bytes(message[1..9].try_into().unwrap_or([0; 8]));
                    if (1..=abi::STALL_MAX).contains(&ticks) {
                        println!("[serial] asked to stall: doing nothing for {ticks} ticks");
                        taaj::sleep(ticks);
                        println!("[serial] working again");
                    }
                }
                Ok(_) => {} // nothing this driver knows: dropped
                Err(Error::PeerClosed) => return 0,
                Err(_) => return 6,
            },
            Ok(Some(_)) => {
                if let Some(channel) = pings {
                    if !taaj::answer_pings(channel) {
                        pings = None;
                    }
                }
            }
            // Nothing more came: an ESC alone was the Escape key. (Or
            // it is only time to try the reader again.)
            Ok(None) if unsent.is_none() => {
                if let Some(bytes) = decoder.flush() {
                    match send(&bytes) {
                        Ok(true) => {}
                        Ok(false) => unsent = Some(bytes),
                        Err(code) => return code,
                    }
                }
            }
            Ok(None) => {}
            Err(_) => return 5,
        }
    }
}
