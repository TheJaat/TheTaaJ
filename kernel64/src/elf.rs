//! Reading user programs: ELF64 executables.
//!
//! A program file is untrusted input, possibly broken, possibly hostile,
//! so every number in it is checked before it is used: offsets stay
//! inside the file, segments stay inside user space and do not overlap,
//! sizes do not overflow. A bad file is an error, never a kernel crash.
//!
//! Two rules go beyond the format:
//!   - W^X: no segment may be both writable and executable. Memory that
//!     can be written and then run is how injected code runs; refusing
//!     it at load time closes that door for every program.
//!   - nothing below 64 KiB: page 0 stays unmapped so null pointers
//!     fault, with a margin for null + small offsets.
//!   - at most MEMORY_MAX in all: a few bytes of file can ask for
//!     gigabytes of zeroed memory, and loading it would take every frame
//!     the machine has before failing.
//!
//! This only parses and validates; the kernel does the copying and
//! mapping (process.rs).

use alloc::vec::Vec;
use core::fmt;
use core::ops::Range;

pub const PAGE: u64 = 4096;
/// The lowest address a segment may use.
pub const LOWEST_ADDRESS: u64 = 0x1_0000;
/// The most memory one program's segments may take, in whole pages.
pub const MEMORY_MAX: u64 = 64 << 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub virtual_range: Range<u64>,
    /// The bytes to copy from the file; the rest of the range is zeroed.
    pub file_range: Range<usize>,
    pub writable: bool,
    pub executable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub entry: u64,
    pub segments: Vec<Segment>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElfError {
    TooShort,
    NotElf,
    Not64BitLittleEndian,
    NotExecutable,
    WrongMachine,
    BadProgramHeaders,
    SegmentOutsideFile,
    SegmentOutsideUserSpace,
    SegmentsOverlap,
    WritableAndExecutable,
    NothingToLoad,
    EntryNotExecutable,
    TooBig,
}

impl fmt::Display for ElfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

const PT_LOAD: u32 = 1;
const PF_X: u32 = 1;
const PF_W: u32 = 2;
const EM_X86_64: u16 = 62;
const ET_EXEC: u16 = 2;

fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn u64_at(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

/// Checks `file` and describes what to load where. Segments must end at
/// or below `user_end`.
pub fn parse(file: &[u8], user_end: u64) -> Result<Image, ElfError> {
    if file.len() < 64 {
        return Err(ElfError::TooShort);
    }
    if &file[0..4] != b"\x7FELF" {
        return Err(ElfError::NotElf);
    }
    if file[4] != 2 || file[5] != 1 {
        return Err(ElfError::Not64BitLittleEndian);
    }
    if u16_at(file, 16) != ET_EXEC {
        return Err(ElfError::NotExecutable);
    }
    if u16_at(file, 18) != EM_X86_64 {
        return Err(ElfError::WrongMachine);
    }
    let entry = u64_at(file, 24);
    let header_offset = u64_at(file, 32);
    let header_size = u16_at(file, 54) as u64;
    let header_count = u16_at(file, 56) as u64;
    if header_size != 56
        || header_offset.checked_add(header_size * header_count).is_none_or(|end| end > file.len() as u64)
    {
        return Err(ElfError::BadProgramHeaders);
    }

    let mut segments: Vec<Segment> = Vec::new();
    let mut memory = 0;
    for n in 0..header_count {
        let h = &file[(header_offset + n * header_size) as usize..][..56];
        if u32_at(h, 0) != PT_LOAD {
            continue;
        }
        let flags = u32_at(h, 4);
        let offset = u64_at(h, 8);
        let address = u64_at(h, 16);
        let file_size = u64_at(h, 32);
        let memory_size = u64_at(h, 40);
        if memory_size == 0 {
            continue;
        }
        if file_size > memory_size
            || offset.checked_add(file_size).is_none_or(|end| end > file.len() as u64)
        {
            return Err(ElfError::SegmentOutsideFile);
        }
        let end = address.checked_add(memory_size).ok_or(ElfError::SegmentOutsideUserSpace)?;
        if address < LOWEST_ADDRESS || end > user_end {
            return Err(ElfError::SegmentOutsideUserSpace);
        }
        let (writable, executable) = (flags & PF_W != 0, flags & PF_X != 0);
        if writable && executable {
            return Err(ElfError::WritableAndExecutable);
        }
        // Permissions are per page, so two segments sharing a page would
        // need two permissions at once. Compare whole pages.
        let pages = address & !(PAGE - 1)..(end + PAGE - 1) & !(PAGE - 1);
        for other in &segments {
            let other_pages =
                other.virtual_range.start & !(PAGE - 1)..(other.virtual_range.end + PAGE - 1) & !(PAGE - 1);
            if pages.start < other_pages.end && other_pages.start < pages.end {
                return Err(ElfError::SegmentsOverlap);
            }
        }
        memory += pages.end - pages.start;
        if memory > MEMORY_MAX {
            return Err(ElfError::TooBig);
        }
        segments.push(Segment {
            virtual_range: address..end,
            file_range: offset as usize..(offset + file_size) as usize,
            writable,
            executable,
        });
    }

    if segments.is_empty() {
        return Err(ElfError::NothingToLoad);
    }
    if !segments.iter().any(|s| s.executable && s.virtual_range.contains(&entry)) {
        return Err(ElfError::EntryNotExecutable);
    }
    Ok(Image { entry, segments })
}

#[cfg(test)]
mod tests {
    use super::*;

    const END: u64 = 0x0000_8000_0000_0000;

    /// (flags, file offset, address, file size, memory size)
    type Header = (u32, u64, u64, u64, u64);

    /// A minimal executable: the ELF header, program headers, and
    /// `body` bytes of contents starting at offset 0x1000.
    fn elf(entry: u64, headers: &[Header], body: usize) -> Vec<u8> {
        let mut f = vec![0u8; 0x1000 + body];
        f[0..4].copy_from_slice(b"\x7FELF");
        f[4] = 2; // 64-bit
        f[5] = 1; // little-endian
        f[6] = 1;
        f[16..18].copy_from_slice(&ET_EXEC.to_le_bytes());
        f[18..20].copy_from_slice(&EM_X86_64.to_le_bytes());
        f[24..32].copy_from_slice(&entry.to_le_bytes());
        f[32..40].copy_from_slice(&64u64.to_le_bytes());
        f[54..56].copy_from_slice(&56u16.to_le_bytes());
        f[56..58].copy_from_slice(&(headers.len() as u16).to_le_bytes());
        for (n, &(flags, offset, address, file_size, memory_size)) in headers.iter().enumerate() {
            let h = &mut f[64 + 56 * n..][..56];
            h[0..4].copy_from_slice(&PT_LOAD.to_le_bytes());
            h[4..8].copy_from_slice(&flags.to_le_bytes());
            h[8..16].copy_from_slice(&offset.to_le_bytes());
            h[16..24].copy_from_slice(&address.to_le_bytes());
            h[32..40].copy_from_slice(&file_size.to_le_bytes());
            h[40..48].copy_from_slice(&memory_size.to_le_bytes());
        }
        f
    }

    const RX: u32 = 5;
    const R: u32 = 4;
    const RW: u32 = 6;

    fn typical() -> Vec<u8> {
        elf(
            0x40_0010,
            &[
                (RX, 0x1000, 0x40_0000, 0x100, 0x100),
                (R, 0x1100, 0x40_1000, 0x80, 0x80),
                (RW, 0x1180, 0x40_2000, 0x40, 0x2000), // .data + .bss
            ],
            0x200,
        )
    }

    #[test]
    fn accepts_a_typical_program() {
        let image = parse(&typical(), END).unwrap();
        assert_eq!(image.entry, 0x40_0010);
        assert_eq!(image.segments.len(), 3);
        let data = &image.segments[2];
        assert_eq!(data.virtual_range, 0x40_2000..0x40_4000);
        assert_eq!(data.file_range, 0x1180..0x11C0);
        assert!(data.writable && !data.executable);
    }

    #[test]
    fn rejects_damaged_headers() {
        assert_eq!(parse(&[0; 10], END), Err(ElfError::TooShort));
        let mut f = typical();
        f[0] = 0;
        assert_eq!(parse(&f, END), Err(ElfError::NotElf));
        let mut f = typical();
        f[4] = 1;
        assert_eq!(parse(&f, END), Err(ElfError::Not64BitLittleEndian));
        let mut f = typical();
        f[18] = 0xB7; // AArch64
        assert_eq!(parse(&f, END), Err(ElfError::WrongMachine));
        let mut f = typical();
        f[56..58].copy_from_slice(&0xFFFFu16.to_le_bytes()); // headers past the end
        assert_eq!(parse(&f, END), Err(ElfError::BadProgramHeaders));
    }

    #[test]
    fn rejects_segments_pointing_outside_the_file() {
        let f = elf(0x40_0000, &[(RX, 0x1000, 0x40_0000, 0x10_0000, 0x10_0000)], 0x100);
        assert_eq!(parse(&f, END), Err(ElfError::SegmentOutsideFile));
        let f = elf(0x40_0000, &[(RX, u64::MAX, 0x40_0000, 2, 2)], 0x100);
        assert_eq!(parse(&f, END), Err(ElfError::SegmentOutsideFile));
    }

    #[test]
    fn keeps_programs_in_user_space_and_off_the_null_page() {
        for (address, size) in [(0, 0x100), (0x1000, 0x100), (0xFFFF_FFFF_8000_0000, 0x100), (END - 0x10, 0x100)]
        {
            let f = elf(address, &[(RX, 0x1000, address, 0x10, size)], 0x100);
            assert_eq!(parse(&f, END), Err(ElfError::SegmentOutsideUserSpace), "{address:#x}");
        }
        let f = elf(0x40_0000, &[(RX, 0x1000, u64::MAX - 0x10, 0x10, 0x100)], 0x100);
        assert_eq!(parse(&f, END), Err(ElfError::SegmentOutsideUserSpace), "wraps around");
    }

    #[test]
    fn enforces_w_xor_x() {
        let f = elf(0x40_0000, &[(7, 0x1000, 0x40_0000, 0x10, 0x10)], 0x100);
        assert_eq!(parse(&f, END), Err(ElfError::WritableAndExecutable));
    }

    #[test]
    fn rejects_segments_sharing_a_page() {
        // Code ends at 0x400100; data starts in the same page.
        let f = elf(
            0x40_0000,
            &[(RX, 0x1000, 0x40_0000, 0x100, 0x100), (RW, 0x1100, 0x40_0800, 0x10, 0x10)],
            0x200,
        );
        assert_eq!(parse(&f, END), Err(ElfError::SegmentsOverlap));
    }

    #[test]
    fn limits_the_memory_a_program_may_ask_for() {
        // 16 bytes of file, 64 MiB and a page of zeros: refused.
        let f = elf(0x40_0000, &[(RX, 0x1000, 0x40_0000, 0x10, 0x10), (RW, 0x1010, 0x80_0000, 0x10, MEMORY_MAX)], 0x100);
        assert_eq!(parse(&f, END), Err(ElfError::TooBig));
        // The limit is on the total, so many medium segments do not get around it.
        let many: Vec<Header> = (0..5).map(|n| (RW, 0x1000, 0x1000_0000 * (n + 1), 0x10, 16 << 20)).collect();
        assert_eq!(parse(&elf(0x1000_0000, &many, 0x100), END), Err(ElfError::TooBig));
        let f = elf(0x40_0000, &[(RX, 0x1000, 0x40_0000, 0x10, 0x10), (RW, 0x1010, 0x80_0000, 0x10, MEMORY_MAX - 0x1000)], 0x100);
        assert!(parse(&f, END).is_ok(), "exactly the limit is fine");
    }

    #[test]
    fn no_damaged_file_panics() {
        // Every truncation of a good file, and every single byte of its
        // headers flipped: an error or a (still valid) image, never a
        // panic - this runs in the kernel, on files from the disk.
        let good = typical();
        for length in 0..good.len() {
            let _ = parse(&good[..length], END);
        }
        for position in 0..64 + 3 * 56 {
            for flip in [0x01, 0x80, 0xFF] {
                let mut f = good.clone();
                f[position] ^= flip;
                if let Ok(image) = parse(&f, END) {
                    for s in &image.segments {
                        assert!(s.file_range.end <= f.len() && s.file_range.len() as u64 <= s.virtual_range.end - s.virtual_range.start);
                        assert!(s.virtual_range.start >= LOWEST_ADDRESS && s.virtual_range.end <= END);
                    }
                }
            }
        }
    }

    #[test]
    fn the_entry_point_must_be_executable() {
        let f = elf(0x40_2000, &[(RX, 0x1000, 0x40_0000, 0x100, 0x100), (RW, 0x1100, 0x40_2000, 0x10, 0x10)], 0x200);
        assert_eq!(parse(&f, END), Err(ElfError::EntryNotExecutable));
        let f = elf(0x40_0000, &[], 0);
        assert_eq!(parse(&f, END), Err(ElfError::NothingToLoad));
    }
}
