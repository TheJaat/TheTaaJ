//! The TaajBoot protocol - the kernel's side of it.
//!
//! The canonical definition is `bootloader/headers/boot/taajboot.h`.
//! This mirrors it with `#[repr(C)]`, which tells Rust to lay the fields
//! out exactly as C would, in the order written. The compile-time checks
//! at the bottom make any drift between the two a build failure rather
//! than a kernel that reads the wrong field.

use core::ffi::CStr;
use core::fmt;

use crate::memory::addr::{phys_to_virt, PhysAddr};

/// The boot info's addresses are physical; this is where the kernel can
/// read them right now (identity map at first, the direct map later).
fn reach<T>(address: u64) -> *const T {
    phys_to_virt(PhysAddr::new(address)).as_ptr()
}

pub const MAGIC: u64 = 0x544F_4F42_4A41_4154; // "TAAJBOOT"
pub const VERSION: u32 = 1;

pub const BOOT_BIOS: u32 = 1;
pub const BOOT_UEFI: u32 = 2;

pub const MEMORY_USABLE: u32 = 1;
pub const MEMORY_RESERVED: u32 = 2;
pub const MEMORY_ACPI_RECLAIMABLE: u32 = 3;
pub const MEMORY_ACPI_NVS: u32 = 4;
pub const MEMORY_BAD: u32 = 5;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct MemoryRegion {
    pub base: u64,
    pub length: u64,
    pub kind: u32,
    reserved: u32,
}

impl MemoryRegion {
    pub fn is_usable(&self) -> bool {
        self.kind == MEMORY_USABLE
    }

    pub fn kind_name(&self) -> &'static str {
        match self.kind {
            MEMORY_USABLE => "usable",
            MEMORY_RESERVED => "reserved",
            MEMORY_ACPI_RECLAIMABLE => "ACPI reclaimable",
            MEMORY_ACPI_NVS => "ACPI NVS",
            MEMORY_BAD => "bad",
            _ => "unknown",
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Framebuffer {
    pub address: u64,
    pub width: u32,
    pub height: u32,
    /// Bytes per line. Often more than width * 4: lines can be padded.
    pub pitch: u32,
    pub bpp: u8,
    pub red_size: u8,
    pub red_shift: u8,
    pub green_size: u8,
    pub green_shift: u8,
    pub blue_size: u8,
    pub blue_shift: u8,
    reserved: u8,
    padding: u32,
}

/// Fields that are pointers on the C side are kept private here and
/// exposed through methods that turn them into safe Rust values - a
/// slice, a `&str`, an `Option`. Plain numbers are simply `pub`.
#[repr(C)]
pub struct BootInfo {
    magic: u64,
    version: u32,
    size: u32,

    pub kernel_physical: u64,
    pub kernel_virtual: u64,
    pub kernel_size: u64,

    pub ramdisk_address: u64,
    pub ramdisk_size: u64,

    memory_map: u64,
    memory_map_count: u32,
    pub boot_method: u32,

    pub acpi_rsdp: u64,
    pub device_tree: u64,
    command_line: u64,

    framebuffer: Framebuffer,

    loader_name: u64,
}

#[derive(Debug)]
pub enum BootInfoError {
    Null,
    Misaligned,
    BadMagic(u64),
    UnsupportedVersion(u32),
    TooSmall(u32),
}

impl fmt::Display for BootInfoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BootInfoError::Null => f.write_str("null pointer"),
            BootInfoError::Misaligned => f.write_str("not 8-byte aligned"),
            BootInfoError::BadMagic(m) => write!(f, "bad magic {m:#x}"),
            BootInfoError::UnsupportedVersion(v) => write!(f, "unsupported version {v}"),
            BootInfoError::TooSmall(s) => write!(f, "only {s} bytes, need {}", size_of::<BootInfo>()),
        }
    }
}

impl BootInfo {
    /// Checks and borrows the structure at physical `address`.
    ///
    /// The returned reference is `'static` because the loader's memory
    /// below 1 MiB is never reused - but it is only valid while the
    /// current physical offset holds. After the kernel switches to its
    /// own page tables, read it again.
    ///
    /// # Safety
    /// `address` must be reachable through the current physical offset
    /// and, if it holds the magic number, be a TaajBootInfo the loader
    /// filled in.
    pub unsafe fn from_physical(address: u64) -> Result<&'static BootInfo, BootInfoError> {
        if address == 0 {
            return Err(BootInfoError::Null);
        }
        if !address.is_multiple_of(8) {
            return Err(BootInfoError::Misaligned);
        }

        // Check the header before trusting anything after it.
        let info = &*reach::<BootInfo>(address);
        if info.magic != MAGIC {
            return Err(BootInfoError::BadMagic(info.magic));
        }
        if info.version != VERSION {
            return Err(BootInfoError::UnsupportedVersion(info.version));
        }
        if (info.size as usize) < size_of::<BootInfo>() {
            return Err(BootInfoError::TooSmall(info.size));
        }
        Ok(info)
    }

    pub fn boot_method_name(&self) -> &'static str {
        match self.boot_method {
            BOOT_BIOS => "BIOS",
            BOOT_UEFI => "UEFI",
            _ => "unknown",
        }
    }

    /// The firmware's memory map, as a slice.
    pub fn memory_map(&self) -> &'static [MemoryRegion] {
        if self.memory_map == 0 || self.memory_map_count == 0 {
            return &[];
        }
        // SAFETY: the loader wrote `memory_map_count` regions there,
        // below 1 MiB, which is never reused.
        unsafe {
            core::slice::from_raw_parts(
                reach::<MemoryRegion>(self.memory_map),
                self.memory_map_count as usize,
            )
        }
    }

    pub fn framebuffer(&self) -> Option<&Framebuffer> {
        (self.framebuffer.address != 0).then_some(&self.framebuffer)
    }

    pub fn loader_name(&self) -> &'static str {
        // SAFETY: as for memory_map - a C string the loader wrote.
        unsafe { c_string(self.loader_name) }.unwrap_or("(unnamed)")
    }

    pub fn command_line(&self) -> Option<&'static str> {
        // SAFETY: as above.
        unsafe { c_string(self.command_line) }.filter(|line| !line.is_empty())
    }
}

/// A NUL-terminated string at a physical address, if there is one and
/// it is valid UTF-8.
unsafe fn c_string(address: u64) -> Option<&'static str> {
    if address == 0 {
        return None;
    }
    CStr::from_ptr(reach::<core::ffi::c_char>(address)).to_str().ok()
}

// ---- layout checks --------------------------------------------------
//
// Evaluated by the compiler. If any of these is false, the kernel does
// not build - which is exactly when you want to find out.

const _: () = {
    use core::mem::offset_of;

    assert!(size_of::<MemoryRegion>() == 24);
    assert!(size_of::<Framebuffer>() == 32);
    assert!(size_of::<BootInfo>() == 136);

    assert!(offset_of!(BootInfo, version) == 8);
    assert!(offset_of!(BootInfo, kernel_physical) == 16);
    assert!(offset_of!(BootInfo, ramdisk_address) == 40);
    assert!(offset_of!(BootInfo, memory_map) == 56);
    assert!(offset_of!(BootInfo, memory_map_count) == 64);
    assert!(offset_of!(BootInfo, boot_method) == 68);
    assert!(offset_of!(BootInfo, acpi_rsdp) == 72);
    assert!(offset_of!(BootInfo, command_line) == 88);
    assert!(offset_of!(BootInfo, framebuffer) == 96);
    assert!(offset_of!(BootInfo, loader_name) == 128);

    assert!(offset_of!(Framebuffer, pitch) == 16);
    assert!(offset_of!(Framebuffer, bpp) == 20);
    assert!(offset_of!(Framebuffer, blue_shift) == 26);
};
