//! The kernel's ELF reader (kernel/kernel64/src/elf.rs): every program
//! a process starts is a file anyone could have written.
#![no_main]
fuzz::fuzz_target!(|data: &[u8]| {
    // The user half's end, as the kernel has it; both architectures.
    for machine in [62u16, 183] {
        if let Ok(image) = kernel64::elf::parse_for(data, 0x0000_8000_0000_0000, machine) {
            for segment in &image.segments {
                let _ = segment;
            }
        }
    }
});
