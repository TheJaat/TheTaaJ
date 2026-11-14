//! COM1, the first serial port - a 16550-compatible UART at port 0x3F8.
//!
//! The simplest output device there is, and the most useful one this
//! early: QEMU (`-serial stdio`), VMware and VirtualBox can all connect
//! it to a terminal or a file, so the kernel can talk before it can draw.

use super::port::{inb, outb};

const COM1: u16 = 0x3F8;

// Registers, as offsets from the base port.
const DATA: u16 = 0; // send/receive; divisor low byte while DLAB is set
const INTERRUPTS: u16 = 1; // interrupt enable; divisor high byte while DLAB is set
const FIFO: u16 = 2;
const LINE_CONTROL: u16 = 3;
const MODEM_CONTROL: u16 = 4;
const LINE_STATUS: u16 = 5;

const LINE_STATUS_TX_EMPTY: u8 = 0x20;

/// 115200 baud, 8 data bits, no parity, one stop bit ("8N1").
pub fn init() {
    // SAFETY: COM1 is a standard, well-known device; these are its
    // documented setup steps and touch nothing else.
    unsafe {
        outb(COM1 + INTERRUPTS, 0x00); // no interrupts: we poll
        outb(COM1 + LINE_CONTROL, 0x80); // DLAB on, to set the divisor
        outb(COM1 + DATA, 0x01); // divisor 1 = 115200 baud (low byte)
        outb(COM1 + INTERRUPTS, 0x00); //                    (high byte)
        outb(COM1 + LINE_CONTROL, 0x03); // DLAB off; 8N1
        outb(COM1 + FIFO, 0xC7); // enable and clear FIFOs
        outb(COM1 + MODEM_CONTROL, 0x0B); // DTR, RTS, OUT2
    }
}

fn write_byte(byte: u8) {
    // SAFETY: as for init - reading the status and writing the data
    // register are the UART's normal operation.
    unsafe {
        // Wait until the UART can take another byte. With no UART
        // present the port reads 0xFF, so this falls straight through
        // rather than hanging a machine without a serial port.
        while inb(COM1 + LINE_STATUS) & LINE_STATUS_TX_EMPTY == 0 {
            core::hint::spin_loop();
        }
        outb(COM1 + DATA, byte);
    }
}

pub fn write_str(text: &str) {
    for byte in text.bytes() {
        // Terminals expect "\r\n"; Rust strings use "\n".
        if byte == b'\n' {
            write_byte(b'\r');
        }
        write_byte(byte);
    }
}
