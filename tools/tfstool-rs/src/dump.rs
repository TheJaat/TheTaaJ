//! `tfstool dump`: print what is actually on disk.
//!
//! Deliberately independent of `Volume`. It walks the raw blocks itself
//! and reports what it finds - including damage - instead of refusing to
//! mount. A bug in `Volume` then shows up as a disagreement between the
//! dump and `ls`, rather than being reproduced identically by both.
//!
//! For a volume without directories, the output matches the C tool's
//! `dump` line for line, so the two can be compared with `diff`. A
//! directory in a directory is shown indented under its name.

use std::collections::BTreeSet;
use std::io::{self, Write};

use crate::device::BlockDevice;
use crate::format::{
    empty_block, DirBlock, Inode, Superblock, MAGIC, SUPER_A, SUPER_B, TYPE_DIR, VERSION,
};
use crate::volume::MAX_DEPTH;

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
        device.read_block(slot, &mut block).map_err(io::Error::other)?;
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

    if live.version != VERSION {
        writeln!(out, "format version {}: may hold directories", live.version)?;
    }

    writeln!(out, "\ntree:")?;
    let mut walk = Walk { files: 0, directories: 0, blocks: 0, seen: BTreeSet::new() };
    if !walk.directory(device, out, live.root_dir, 0)? {
        return Ok(false);
    }

    let reachable = walk.blocks.saturating_add(2); // plus the two superblocks
    if walk.directories > 0 {
        write!(out, "\n{} directories, ", walk.directories)?;
    } else {
        writeln!(out)?;
    }
    writeln!(
        out,
        "{} files, {reachable} blocks reachable, {} of {} free",
        walk.files,
        live.total_blocks.saturating_sub(reachable),
        live.total_blocks
    )?;
    Ok(true)
}

/// What a walk of the tree has counted so far.
struct Walk {
    files: u32,
    directories: u32,
    blocks: u32,
    /// Directory blocks already visited: one seen twice is a loop, or
    /// two names for one directory.
    seen: BTreeSet<u32>,
}

impl Walk {
    /// Prints the directory that starts at block `first`, and everything
    /// under it, each level indented two more. Returns false - after
    /// saying why - if the tree cannot be followed.
    ///
    /// It calls itself for a directory in a directory. How deep that
    /// goes is up to the disk, so it is counted, and stopped at the
    /// depth the format allows.
    fn directory<D: BlockDevice, W: Write>(
        &mut self,
        device: &mut D,
        out: &mut W,
        first: u32,
        depth: usize,
    ) -> io::Result<bool> {
        let pad = "  ".repeat(depth);
        if depth > MAX_DEPTH {
            writeln!(out, "{pad}  (directories nest deeper than {MAX_DEPTH})")?;
            return Ok(false);
        }
        let mut block = empty_block();
        let mut chain: u32 = 0;
        let mut next = first;

        while next != 0 {
            chain += 1;
            if chain > MAX_CHAIN {
                writeln!(out, "{pad}  (directory chain loops)")?;
                return Ok(false);
            }
            if !self.seen.insert(next) {
                writeln!(out, "{pad}  (directory block {next} is reached twice)")?;
                return Ok(false);
            }

            device.read_block(next, &mut block).map_err(io::Error::other)?;
            let dir = DirBlock::parse(&block);
            if dir.magic != MAGIC {
                writeln!(out, "{pad}  block {next} is not a directory block")?;
                return Ok(false);
            }
            writeln!(out, "{pad}  dir block {next}: {} entries, next {}", dir.count, dir.next)?;
            self.blocks = self.blocks.saturating_add(1);

            for entry in dir.entries.iter().filter(|e| e.inode != 0) {
                if entry.kind == TYPE_DIR {
                    writeln!(out, "{pad}    {}/", entry.name_lossy())?;
                    self.directories += 1;
                    if !self.directory(device, out, entry.inode, depth + 1)? {
                        return Ok(false);
                    }
                    continue;
                }

                let mut raw = empty_block();
                device.read_block(entry.inode, &mut raw).map_err(io::Error::other)?;
                let inode = Inode::parse(&raw);

                writeln!(
                    out,
                    "{pad}    {:<20} inode {:<5} size {:<7} blocks {}{}",
                    entry.name_lossy(),
                    entry.inode,
                    entry.size,
                    inode.block_count,
                    if inode.magic == MAGIC { "" } else { "   [BAD INODE MAGIC]" }
                )?;
                if inode.size != entry.size {
                    writeln!(
                        out,
                        "{pad}      [MISMATCH] inode says {}, directory says {}",
                        inode.size, entry.size
                    )?;
                }
                self.files += 1;
                // Saturating: a corrupt count must not crash the one tool
                // whose job is to look at corrupt volumes.
                self.blocks = self.blocks.saturating_add(1).saturating_add(inode.block_count);
            }
            next = dir.next;
        }
        Ok(true)
    }
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
    fn dump_shows_directories_indented() {
        let mut device = MemDevice::new(64);
        format(&mut device, 64).unwrap();
        let mut v = Volume::mount(device).unwrap();
        v.put("top", b"t").unwrap();
        v.make_directory("docs").unwrap();
        v.make_directory("docs/notes").unwrap();
        v.put("docs/notes/today", b"deep").unwrap();
        let mut device = v.into_device();

        let text = dump_text(&mut device);
        let lines: Vec<&str> = text.lines().collect();
        let tree = lines.iter().position(|l| *l == "tree:").unwrap();
        assert!(text.contains("format version 2: may hold directories"), "{text}");
        assert!(lines[tree + 1].starts_with("  dir block "), "{text}");
        assert!(lines[tree + 2].starts_with("    top "), "{text}");
        assert_eq!(lines[tree + 3], "    docs/", "{text}");
        assert!(lines[tree + 4].starts_with("    dir block "), "{text}");
        assert_eq!(lines[tree + 5], "      notes/", "{text}");
        assert!(lines[tree + 6].starts_with("      dir block "), "{text}");
        assert!(lines[tree + 7].starts_with("        today "), "{text}");
        // Blocks: 3 directory blocks, 2 inodes, 2 of data, 2 superblocks.
        assert!(text.ends_with("\n2 directories, 2 files, 9 blocks reachable, 55 of 64 free\n"), "{text}");
        // What the dump counts as free is what the volume does.
        let v = Volume::mount(device).unwrap();
        assert_eq!(v.free_blocks(), 55);
    }

    #[test]
    fn dump_refuses_to_go_round_a_loop() {
        let mut device = MemDevice::new(64);
        format(&mut device, 64).unwrap();
        let mut v = Volume::mount(device).unwrap();
        v.make_directory("d").unwrap();
        let inside = v.lookup("d").unwrap().inode;
        let root = v.superblock().root_dir;
        let mut device = v.into_device();
        let back = crate::format::DirEntry::new("back", root, 0, TYPE_DIR);
        device.blocks[inside as usize] = DirBlock::new(vec![back], 0).to_block();

        let mut out = Vec::new();
        assert!(!dump(&mut device, &mut out).unwrap(), "not mountable");
        assert!(String::from_utf8(out).unwrap().contains("is reached twice"));
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
