//! The screen, as the bootloader left it: a linear framebuffer, one
//! 32-bit value per pixel, `pitch` bytes per line.
//!
//! For now this only proves the TaajBootInfo describes it correctly, by
//! painting something recognisable. A real console comes later.

use kernel64::bootinfo::Framebuffer;
use kernel64::memory::addr::{phys_to_virt, PhysAddr};

/// Builds a pixel value from 8-bit red, green and blue, placing each
/// channel where *this* framebuffer wants it. Hard-coding 0x00RRGGBB
/// would work in QEMU and show wrong colours on hardware that orders the
/// channels differently - the shifts in the boot info exist for that.
fn pixel(fb: &Framebuffer, red: u8, green: u8, blue: u8) -> u32 {
    let channel = |value: u8, size: u8, shift: u8| -> u32 {
        // Keep the top `size` bits of the 8-bit value.
        (u32::from(value) >> (8 - u32::from(size.min(8)))) << shift
    };
    channel(red, fb.red_size, fb.red_shift)
        | channel(green, fb.green_size, fb.green_shift)
        | channel(blue, fb.blue_size, fb.blue_shift)
}

/// Paints a vertical gradient with a band across it. Returns false, and
/// draws nothing, for any format other than 32 bits per pixel.
pub fn paint_welcome(fb: &Framebuffer) -> bool {
    if fb.bpp != 32 || fb.width == 0 || fb.height == 0 {
        return false;
    }

    let base = phys_to_virt(PhysAddr::new(fb.address)).as_mut_ptr::<u8>();
    let band = fb.height / 3..fb.height / 3 + fb.height / 12;

    for y in 0..fb.height {
        // Dark navy at the top shading to deep teal at the bottom.
        let t = (y * 255 / fb.height) as u8;
        let colour = if band.contains(&y) {
            pixel(fb, 0xE8, 0xA8, 0x38) // the band: warm amber
        } else {
            pixel(fb, 0x10, 0x18 + t / 6, 0x30 + t / 4)
        };

        // SAFETY: the kernel maps the framebuffer in its direct map, and
        // every write stays inside line `y`, which starts at
        // base + y * pitch and holds `width` 4-byte pixels.
        //
        // `write_volatile`: this is device memory. Without it the
        // compiler may merge or drop writes it believes nobody reads.
        let line = unsafe { base.add((y * fb.pitch) as usize) } as *mut u32;
        for x in 0..fb.width as usize {
            unsafe { line.add(x).write_volatile(colour) };
        }
    }
    true
}
