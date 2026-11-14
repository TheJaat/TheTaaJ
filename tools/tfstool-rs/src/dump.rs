//! `tfstool dump`: print what is actually on disk.
//!
//! Deliberately independent of `Volume`. It walks the raw blocks itself
//! and reports what it finds - including damage - instead of refusing to
//! mount. A bug in `Volume` then shows up as a disagreement between the
//! dump and `ls`, rather than being reproduced identically by both.
//!
//! The output matches the C tool's `dump` line for line, so the two can
//! be compared with `diff`.

use std::io::{self, Write};

use crate::device::BlockDevice;
use crate::format::{empty_block, DirBlock, Inode, Superblock, MAGIC, SUPER_A, SUPER_B};

/// The C dump gives up after this many directory blocks.
const MAX_CHAIN: u32 = 64;

/// Writes the report to `out` - stdout in the tool, a Vec<u8> in tests.
/// Returns whether the volume is mountable.
///
/// `W: Write` is the same idea as `BlockDevice`: anything that accepts
/// bytes will do.
pub fn dump<D: BlockDevice, W: Write>(device: &mut D, out: &mut W) -> io::Result<bool> {
    let mut block = empty_block();
    let mut live: Option<(char, Superblock)> = None;

    writeln!(out, "superblocks:")?;
    for (slot, letter) in [(SUPER_A, 'A'), (SUPER_B, 'B')] {
        device.read_block(slot, &mut block)?;
        let sb = Superblock::parse(&block);
        let valid = sb.is_valid();

        write!(
            out,
            "  slot {letter}: {}",
            if valid { "valid  " } else { "INVALID" }
        )?;
        if sb.magic == MAGIC {
            write!(
                out,
                "  generation {:<6} root block {:<6} total {}",
                sb.generation, sb.root_dir, sb.total_blocks
            )?;
            if !valid {
                write!(
                    out,
                    "   (checksum {:08x}, computed {:08x})",
                    sb.checksum,
                    sb.compute_checksum()
                )?;
            }
        }
        writeln!(out)?;

        if valid && live.is_none_or(|(_, current)| sb.generation > current.generation) {
            live = Some((letter, sb));
        }
    }

    let Some((letter, live)) = live else {
        writeln!(out, "\nno valid superblock - the volume is unmountable")?;
        return Ok(false);
    };
    writeln!(out, "\nlive: slot {letter}, generation {}", live.generation)?;

    writeln!(out, "\ntree:")?;
    let mut files: u32 = 0;
    let mut blocks: u32 = 0;
    let mut chain: u32 = 0;
    let mut next = live.root_dir;

    while next != 0 {
        chain += 1;
        if chain > MAX_CHAIN {
            writeln!(out, "  (directory chain loops)")?;
            return Ok(false);
        }

        device.read_block(next, &mut block)?;
        let dir = DirBlock::parse(&block);
        if dir.magic != MAGIC {
            writeln!(out, "  block {next} is not a directory block")?;
            return Ok(false);
        }
        writeln!(
            out,
            "  dir block {next}: {} entries, next {}",
            dir.count, dir.next
        )?;
        blocks += 1;

        for entry in dir.entries.iter().filter(|e| e.inode != 0) {
            let mut raw = empty_block();
            device.read_block(entry.inode, &mut raw)?;
            let inode = Inode::parse(&raw);

            writeln!(
                out,
                "    {:<20} inode {:<5} size {:<7} blocks {}{}",
                entry.name_lossy(),
                entry.inode,
                entry.size,
                inode.block_count,
                if inode.magic == MAGIC {
                    ""
                } else {
                    "   [BAD INODE MAGIC]"
                }
            )?;
            if inode.size != entry.size {
                writeln!(
                    out,
                    "      [MISMATCH] inode says {}, directory says {}",
                    inode.size, entry.size
                )?;
            }
            files += 1;
            // Saturating: a corrupt count must not crash the one tool
            // whose job is to look at corrupt volumes.
            blocks = blocks.saturating_add(1).saturating_add(inode.block_count);
        }
        next = dir.next;
    }

    let reachable = blocks.saturating_add(2); // plus the two superblocks
    writeln!(
        out,
        "\n{files} files, {reachable} blocks reachable, {} of {} free",
        live.total_blocks.saturating_sub(reachable),
        live.total_blocks
    )?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::MemDevice;
    use crate::volume::{format, Volume};

    fn dump_text(device: &mut MemDevice) -> String {
        let mut out = Vec::new();
        dump(device, &mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn dump_matches_the_c_tool_layout() {
        // The same steps as the C tool's `mkfs` + `put hello.txt`
        // (create, then write), so the text must match the C dump.
        let mut device = MemDevice::new(4096);
        format(&mut device, 4096).unwrap();
        let mut v = Volume::mount(device).unwrap();
        v.create("hello.txt").unwrap();
        v.write_at("hello.txt", 0, b"hi\n").unwrap();
        let mut device = v.into_device();

        let expected = "\
superblocks:
  slot A: valid    generation 3      root block 6      total 4096
  slot B: valid    generation 2      root block 4      total 4096

live: slot A, generation 3

tree:
  dir block 6: 1 entries, next 0
    hello.txt            inode 5     size 3       blocks 1

1 files, 5 blocks reachable, 4091 of 4096 free
";
        assert_eq!(dump_text(&mut device), expected);
    }

    #[test]
    fn dump_reports_a_damaged_superblock() {
        let mut device = MemDevice::new(64);
        format(&mut device, 64).unwrap();
        device.blocks[0][20] ^= 1; // flip a bit in slot A's root pointer

        let text = dump_text(&mut device);
        assert!(text.contains("slot A: INVALID"));
        assert!(text.contains("no valid superblock"));
    }
}
