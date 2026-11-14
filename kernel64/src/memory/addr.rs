//! Physical and virtual addresses, as different types.
//!
//! Both are just 64-bit numbers, and in C both would be `uint64_t` - so
//! nothing stops you passing a physical address where a virtual one was
//! meant, and that bug compiles fine and corrupts memory at runtime.
//! Wrapping each in its own type (a "newtype") makes mixing them a
//! compile error, at zero runtime cost: `#[repr(transparent)]` means a
//! PhysAddr is laid out exactly like the u64 inside it.

use core::fmt;
use core::sync::atomic::{AtomicU64, Ordering};

pub const PAGE_SIZE: u64 = 4096;

/// Rounds down to a multiple of `align`, which must be a power of two.
pub const fn align_down(value: u64, align: u64) -> u64 {
    value & !(align - 1)
}

/// Rounds up to a multiple of `align`, which must be a power of two.
pub const fn align_up(value: u64, align: u64) -> u64 {
    align_down(value + (align - 1), align)
}

macro_rules! address_type {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
        #[repr(transparent)]
        pub struct $name(u64);

        impl $name {
            pub const fn new(address: u64) -> Self {
                Self(address)
            }
            pub const fn as_u64(self) -> u64 {
                self.0
            }
            pub const fn align_down(self, align: u64) -> Self {
                Self(align_down(self.0, align))
            }
            pub const fn align_up(self, align: u64) -> Self {
                Self(align_up(self.0, align))
            }
            pub const fn is_aligned(self, align: u64) -> bool {
                self.0 & (align - 1) == 0
            }
            /// This address plus `bytes`.
            pub const fn add(self, bytes: u64) -> Self {
                Self(self.0 + bytes)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!(stringify!($name), "({:#x})"), self.0)
            }
        }

        impl fmt::LowerHex for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::LowerHex::fmt(&self.0, f)
            }
        }
    };
}

address_type!(PhysAddr, "An address in physical memory - what the RAM chips see.");
address_type!(VirtAddr, "An address as code sees it, translated by the page tables.");

impl VirtAddr {
    pub fn as_ptr<T>(self) -> *const T {
        self.0 as *const T
    }
    pub fn as_mut_ptr<T>(self) -> *mut T {
        self.0 as *mut T
    }
}

/// Where all of physical memory appears in the virtual address space.
///
/// Zero while the bootloader's identity map is in use (physical address
/// == virtual address), then the start of the kernel's direct map. An
/// atomic only because a plain `static mut` cannot be read safely; on
/// one CPU this is never contended.
static PHYSICAL_OFFSET: AtomicU64 = AtomicU64::new(0);

pub fn set_physical_offset(offset: u64) {
    PHYSICAL_OFFSET.store(offset, Ordering::Release);
}

pub fn physical_offset() -> u64 {
    PHYSICAL_OFFSET.load(Ordering::Acquire)
}

/// The virtual address at which the kernel can reach physical `address`.
pub fn phys_to_virt(address: PhysAddr) -> VirtAddr {
    VirtAddr::new(address.as_u64().wrapping_add(physical_offset()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alignment() {
        assert_eq!(align_down(0x1fff, 0x1000), 0x1000);
        assert_eq!(align_up(0x1001, 0x1000), 0x2000);
        assert_eq!(align_up(0x1000, 0x1000), 0x1000);
        assert!(PhysAddr::new(0x20_0000).is_aligned(0x20_0000));
        assert!(!PhysAddr::new(0x20_1000).is_aligned(0x20_0000));
    }

    #[test]
    fn addresses_print_as_hex() {
        assert_eq!(format!("{:?}", PhysAddr::new(0x1000)), "PhysAddr(0x1000)");
        assert_eq!(format!("{:#x}", VirtAddr::new(0xff)), "0xff");
    }
}
