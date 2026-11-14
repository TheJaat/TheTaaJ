//! Reads the keyboard controller's data port without being allowed to.
//! The I/O permission bitmap refuses it: #GP, and the kernel kills this
//! process. Only a process given the ports - the keyboard driver - may
//! touch the device.

#![no_std]
#![no_main]

use taaj::println;

taaj::program!(main);

fn main() -> i64 {
    println!("[port] reading port 0x60 without a capability...");
    // SAFETY: none - this is the point.
    let byte = unsafe { taaj::port::in8(0x60) };
    println!("[port] FAILED: read {byte:#x}");
    1
}
