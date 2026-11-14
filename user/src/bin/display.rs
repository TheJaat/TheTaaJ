//! The display service: owns the screen, from ring 3.
//!
//! Startup handles, from init:
//!   1  the framebuffer's memory object (READ and WRITE)
//!   2  a channel on which init sends the framebuffer's description
//!
//! It maps the framebuffer - the same physical pixels the graphics card
//! scans out - and paints them. The kernel drew this screen until now;
//! the code is the same, only it runs in a process that can touch
//! nothing but the screen, and can crash without taking the kernel
//! along. A text console on this screen comes next.

#![no_std]
#![no_main]

use abi::FramebufferInfo;
use taaj::{println, Error};

taaj::program!(main);

/// Builds a pixel value from 8-bit red, green and blue, placing each
/// channel where *this* framebuffer wants it. Hard-coding 0x00RRGGBB
/// would work in QEMU and show wrong colours on hardware that orders the
/// channels differently - the shifts in the description exist for that.
fn pixel(fb: &FramebufferInfo, red: u8, green: u8, blue: u8) -> u32 {
    let channel = |value: u8, size: u8, shift: u8| -> u32 {
        // Keep the top `size` bits of the 8-bit value.
        (u32::from(value) >> (8 - u32::from(size.min(8)))) << shift
    };
    channel(red, fb.red_size, fb.red_shift)
        | channel(green, fb.green_size, fb.green_shift)
        | channel(blue, fb.blue_size, fb.blue_shift)
}

/// A vertical gradient with an amber band a third of the way down.
fn paint_welcome(fb: &FramebufferInfo, base: *mut u8) {
    let band = fb.height / 3..fb.height / 3 + fb.height / 12;
    for y in 0..fb.height {
        // Dark navy at the top shading to deep teal at the bottom.
        let t = (y * 255 / fb.height) as u8;
        let colour = if band.contains(&y) {
            pixel(fb, 0xE8, 0xA8, 0x38) // the band: warm amber
        } else {
            pixel(fb, 0x10, 0x18 + t / 6, 0x30 + t / 4)
        };
        // SAFETY: the mapping covers pitch * height bytes, and every
        // write stays inside line `y`. Volatile: this is device memory,
        // and the compiler must not merge or drop writes nobody reads.
        let line = unsafe { base.add((y * fb.pitch) as usize) } as *mut u32;
        for x in 0..fb.width as usize {
            unsafe { line.add(x).write_volatile(colour) };
        }
    }
}

fn main() -> i64 {
    let (Some(memory), Some(control)) = (taaj::startup_handle(0), taaj::startup_handle(1)) else {
        return 1;
    };
    let mut message = [0u8; FramebufferInfo::SIZE];
    let fb = match taaj::receive(control, &mut message, &mut []) {
        Ok((FramebufferInfo::SIZE, _)) => FramebufferInfo::from_bytes(&message).expect("sized"),
        _ => return 2,
    };
    if fb.bpp != 32 || fb.width == 0 || fb.height == 0 {
        println!("[display] {} bits per pixel: not supported yet", fb.bpp);
        return 3;
    }
    let base = match taaj::memory_map(memory, true) {
        Ok(base) => base,
        Err(error) => {
            println!("[display] cannot map the framebuffer: {error:?}");
            return 4;
        }
    };
    paint_welcome(&fb, base);
    println!("[display] painted the {}x{} screen from ring 3 (framebuffer mapped at {base:p})", fb.width, fb.height);

    // Nothing more to draw yet: wait for requests, which will come.
    let mut request = [0u8; 64];
    loop {
        match taaj::receive(control, &mut request, &mut []) {
            Err(Error::PeerClosed) => return 0,
            Err(_) => return 5,
            Ok(_) => {}
        }
    }
}
