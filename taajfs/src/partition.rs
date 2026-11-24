//! Where on a disk the TaajFS volume is: at its start, or in a
//! partition.
//!
//! A disk TheTaaJ made for itself (the run disk, the test disks) is one
//! TaajFS volume from sector 0. A USB stick made to boot from cannot
//! be: its first sector is the master boot record the BIOS runs, and it
//! holds the boot image (an ISO 9660 file system) and the EFI system
//! partition too. So the volume goes in a partition of its own, and the
//! MBR's partition table says where:
//!
//! ```text
//!   sector 0, bytes 446..510: four entries of 16 bytes
//!     +0  bootable flag (0x80) or 0
//!     +4  type: 0x7F for TaajFS ("for experimental operating systems")
//!     +8  first sector (u32, little-endian)
//!     +12 sector count (u32)
//!   bytes 510, 511: 0x55 0xAA, "this is a boot record"
//! ```
//!
//! (GPT, the newer partition table, starts with an MBR too: one entry
//! of type 0xEE covering the disk. Such a disk has no 0x7F entry here.)

/// The MBR partition type TaajFS volumes are given.
pub const TAAJFS_TYPE: u8 = 0x7F;

/// Bytes in a sector, as the MBR counts them.
pub const SECTOR: usize = 512;

/// The first TaajFS partition in a master boot record (sector 0):
/// (first sector, sector count). None when sector 0 is not an MBR, or
/// lists no partition of type 0x7F, or lists one that does not fit on a
/// disk of `disk_sectors` sectors.
pub fn find_taajfs(sector0: &[u8; SECTOR], disk_sectors: u64) -> Option<(u64, u64)> {
    if sector0[510] != 0x55 || sector0[511] != 0xAA {
        return None;
    }
    (0..4).find_map(|n| {
        let entry = &sector0[446 + 16 * n..446 + 16 * (n + 1)];
        let first = u64::from(u32::from_le_bytes([entry[8], entry[9], entry[10], entry[11]]));
        let count = u64::from(u32::from_le_bytes([entry[12], entry[13], entry[14], entry[15]]));
        let fits = first > 0 && count > 0 && first + count <= disk_sectors;
        (entry[4] == TAAJFS_TYPE && fits).then_some((first, count))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mbr(entries: &[(u8, u32, u32)]) -> [u8; SECTOR] {
        let mut s = [0u8; SECTOR];
        for (n, &(kind, first, count)) in entries.iter().enumerate() {
            let e = &mut s[446 + 16 * n..446 + 16 * (n + 1)];
            e[4] = kind;
            e[8..12].copy_from_slice(&first.to_le_bytes());
            e[12..16].copy_from_slice(&count.to_le_bytes());
        }
        s[510] = 0x55;
        s[511] = 0xAA;
        s
    }

    #[test]
    fn the_stick_layout_is_found() {
        // As xorriso lays out build/stick64.img: the ISO, the EFI
        // system partition, TaajFS.
        let s = mbr(&[(0x17, 64, 4032), (0xEF, 4096, 8192), (TAAJFS_TYPE, 12288, 32768)]);
        assert_eq!(find_taajfs(&s, 12288 + 32768), Some((12288, 32768)));
    }

    #[test]
    fn anything_else_is_not() {
        // No signature: a TaajFS volume from sector 0, or nothing.
        let mut s = mbr(&[(TAAJFS_TYPE, 2048, 2048)]);
        s[511] = 0;
        assert_eq!(find_taajfs(&s, 1 << 20), None);
        // No 0x7F entry; a GPT's protective MBR.
        assert_eq!(find_taajfs(&mbr(&[(0x0C, 2048, 2048)]), 1 << 20), None);
        assert_eq!(find_taajfs(&mbr(&[(0xEE, 1, u32::MAX)]), 1 << 20), None);
        // One past the end of the disk, one at sector 0, an empty one.
        assert_eq!(find_taajfs(&mbr(&[(TAAJFS_TYPE, 2048, 2048)]), 4095), None);
        assert_eq!(find_taajfs(&mbr(&[(TAAJFS_TYPE, 0, 2048)]), 1 << 20), None);
        assert_eq!(find_taajfs(&mbr(&[(TAAJFS_TYPE, 2048, 0)]), 1 << 20), None);
        // The first that fits wins.
        let s = mbr(&[(TAAJFS_TYPE, 1 << 30, 8), (TAAJFS_TYPE, 100, 8)]);
        assert_eq!(find_taajfs(&s, 1 << 20), Some((100, 8)));
    }
}
