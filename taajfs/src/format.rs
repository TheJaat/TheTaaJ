//! The TaajFS on-disk format.
//!
//! This mirrors `shared/os/tfs.h` byte for byte. The C side gets its
//! layout from struct definitions and trusts the compiler to lay them
//! out the way the disk expects. Here every field is read and written at
//! an explicit offset, in little-endian order. It is more typing, and in
//! exchange the layout is written down instead of implied - and the code
//! is correct on a big-endian machine too, which the C version is not.
//!
//! Nothing in this file touches a device. It only converts between
//! bytes and values, which is what makes it easy to test.

use alloc::string::String;
use alloc::vec::Vec;

/// "JAFS" when read as bytes on disk.
pub const MAGIC: u32 = 0x5346_414A;
/// Version 1: files, in one directory. What the C implementation
/// reads and writes, and what a new volume is.
pub const VERSION: u32 = 1;
/// Version 2: there may be directories in directories. A volume becomes
/// this when its first directory is made, and stays it. The number is
/// what keeps a version-1 implementation away: it would take a
/// directory's blocks for a file's, and write over them.
pub const VERSION_DIRECTORIES: u32 = 2;
/// Version 3: a file may be bigger than an inode's 1020 blocks reach
/// (see `Inode`), and there may be directories. A volume becomes this
/// before its first such file is written, and stays it. As with
/// version 2, the number is what keeps older code away: it would read
/// the numbers of index blocks as the file's data.
pub const VERSION_BIG_FILES: u32 = 3;
/// Version 4: a file's inode may carry a change stamp (see `Inode`),
/// and there may be big files and directories. A volume becomes this
/// before the first stamped inode is written - which is the first
/// change made by code that knows it - and stays it. Older code would
/// take a stamped inode for a damaged one.
pub const VERSION_STAMPS: u32 = 4;
/// Version 5: a name may be longer than a slot's 28 bytes - up to
/// LONG_NAME_MAX, in a second slot (see `DirEntry`) - and there may be
/// stamps, big files and directories. A volume becomes this before the
/// first long name is written, and stays it. Older code would take the
/// second slot for an entry of its own, a file whose inode number is
/// four letters of the name.
pub const VERSION_LONG_NAMES: u32 = 5;
/// Version 6: a file's inode may carry the time it last changed, as
/// well as its stamp (a timed inode, see `Inode`), and there may be
/// everything before. A volume becomes this before the first timed
/// inode is written, and stays it. Older code would take a timed inode
/// for a damaged one.
///
/// (Volumes bigger than 256 MiB, up to MAX_BLOCKS, came with this
/// version's code too, but need no number of their own: code that
/// knows only 65536 blocks refuses the superblock of a bigger volume -
/// `is_valid` - so it can never mount one and write over it.)
pub const VERSION_TIMES: u32 = 6;
/// The newest version this code reads and writes.
pub const VERSION_NEWEST: u32 = VERSION_TIMES;
pub const BLOCK_SIZE: usize = 4096;
/// The name field of a slot. A name shorter than this has a NUL after
/// it there and takes one slot; one this long or longer takes two.
pub const NAME_MAX: usize = 28;
/// The longest name: the 28 bytes of the first slot and 36 of a second
/// (protocol::COMPONENT_MAX, the file service's own limit, is the same).
pub const LONG_NAME_MAX: usize = NAME_MAX + MORE_NAME;
/// The name bytes a second slot holds: all of it but the type field.
pub const MORE_NAME: usize = 36;
/// The most blocks a volume may have: 4 GiB. Until version 6's code it
/// was 65536, 256 MiB. Block numbers are u32 and could go further; the
/// free map (one byte a block, in memory) and a file's 32-bit size are
/// what this keeps sensible.
pub const MAX_BLOCKS: u32 = 1 << 20;
/// What the code before it allowed: a superblock saying more is refused
/// there (see VERSION_TIMES).
pub const OLD_MAX_BLOCKS: u32 = 65536;

pub const SUPER_A: u32 = 0;
pub const SUPER_B: u32 = 1;
pub const FIRST_DATA: u32 = 2;

pub const TYPE_FILE: u32 = 1;
/// A directory inside a directory. Its entry's `inode` is not an inode
/// but the first block of its own chain of directory blocks, and its
/// `size` is 0. Only on a volume of VERSION_DIRECTORIES.
pub const TYPE_DIR: u32 = 2;
/// The second slot of a long name: 36 more bytes of the name, then this
/// as its type. It follows its entry's slot, in the same block.
pub const TYPE_MORE: u32 = 3;

pub const DIR_HEADER: usize = 16;
pub const DIR_ENTRY_SIZE: usize = 40;
/// 102 entries fit in one directory block.
pub const DIR_ENTRIES: usize = (BLOCK_SIZE - DIR_HEADER) / DIR_ENTRY_SIZE;

pub const INODE_HEADER: usize = 16;
/// 1020 block pointers fit in one inode block.
pub const INODE_BLOCKS: usize = (BLOCK_SIZE - INODE_HEADER) / 4;
/// The most a file reaches with its blocks named in the inode itself:
/// just under 4 MB. Anything bigger is a big file (version 3).
pub const DIRECT_MAX: usize = INODE_BLOCKS * BLOCK_SIZE;
/// A stamped inode's magic, "JAFT" when read as bytes on disk.
pub const MAGIC_STAMPED: u32 = 0x5446_414A;
/// A stamped inode's header: the four fields, then the stamp, u64.
pub const STAMPED_HEADER: usize = 24;
/// 1018 block pointers fit after it.
pub const STAMPED_BLOCKS: usize = (BLOCK_SIZE - STAMPED_HEADER) / 4;
/// The most a stamped inode reaches with its own block list.
pub const STAMPED_DIRECT_MAX: usize = STAMPED_BLOCKS * BLOCK_SIZE;
/// A timed inode's magic, "JAFU" when read as bytes on disk.
pub const MAGIC_TIMED: u32 = 0x5546_414A;
/// A timed inode's header: the four fields, the stamp, then the time,
/// u64 each.
pub const TIMED_HEADER: usize = 32;
/// 1016 block pointers fit after it.
pub const TIMED_BLOCKS: usize = (BLOCK_SIZE - TIMED_HEADER) / 4;
/// 1024 block numbers fit in one index block.
pub const INDEX_BLOCKS: usize = BLOCK_SIZE / 4;
/// The biggest file: what a timed inode's index blocks reach, 1016 *
/// 1024 blocks - just under 4 GiB, so that the 32-bit size can say it
/// (the other layouts reach a little further; this is the least). On a
/// volume smaller than that, the volume is the limit.
pub const MAX_FILE_SIZE: usize = TIMED_BLOCKS * INDEX_BLOCKS * BLOCK_SIZE;

/// One block, as it sits on disk.
pub type Block = [u8; BLOCK_SIZE];

/// A zeroed block. A function rather than a constant so every caller
/// gets its own fresh 4 KB, never a shared one.
pub fn empty_block() -> Block {
    [0u8; BLOCK_SIZE]
}

/// Reads the little-endian u32 at byte offset `at`.
///
/// `bytes[at..at + 4]` is a slice - a view into the buffer, not a copy.
/// `try_into()` turns it into a fixed `[u8; 4]`, which can only fail if
/// the slice is not exactly 4 long; it always is, so `expect` documents
/// that rather than handling a case that cannot happen.
pub fn get_u32(bytes: &[u8], at: usize) -> u32 {
    let field: [u8; 4] = bytes[at..at + 4].try_into().expect("a 4-byte slice");
    u32::from_le_bytes(field)
}

/// Writes `value` as a little-endian u32 at byte offset `at`.
///
/// `&mut [u8]` is a mutable borrow: this function may change the buffer
/// but does not own it, and while it runs nothing else can touch it.
pub fn put_u32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

// ---- the superblock -------------------------------------------------

/// Size of the superblock on disk. It must stay inside one 512-byte
/// sector - that is what makes a commit atomic (see `tfs.h`).
pub const SUPER_SIZE: usize = 36;
const CHECKSUM_AT: usize = 32;

/// The root of the whole volume. Two copies live in blocks 0 and 1;
/// the valid one with the higher generation is the live one.
///
/// `derive` asks the compiler to write some code for us:
///   Debug        - `{:?}` printing, for tests and error messages
///   Clone, Copy  - it is plain data, so copying it is cheap and implicit
///   PartialEq, Eq - `==`, so tests can compare whole superblocks
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Superblock {
    pub magic: u32,
    pub version: u32,
    pub block_size: u32,
    pub total_blocks: u32,
    pub generation: u32,
    pub root_dir: u32,
    /// Unused, but covered by the checksum - so it is kept exactly as
    /// read, or a volume with anything here would fail to verify.
    pub reserved: [u32; 2],
    pub checksum: u32,
}

impl Superblock {
    /// A new superblock, already sealed with its checksum.
    pub fn new(total_blocks: u32, generation: u32, root_dir: u32) -> Superblock {
        Superblock {
            magic: MAGIC,
            version: VERSION,
            block_size: BLOCK_SIZE as u32,
            total_blocks, // shorthand for `total_blocks: total_blocks`
            generation,
            root_dir,
            reserved: [0, 0],
            checksum: 0,
        }
        .sealed()
    }

    /// The same superblock with its checksum recomputed. Takes `self` by
    /// value and returns a new one: the type is `Copy`, so this is cheap,
    /// and it reads naturally at the end of a chain of changes.
    pub fn sealed(mut self) -> Superblock {
        self.checksum = self.compute_checksum();
        self
    }

    /// Decodes a superblock from the start of a block.
    ///
    /// Panics if `bytes` is shorter than `SUPER_SIZE`. Callers always
    /// pass a whole block, so a short slice is a bug in the caller, and
    /// a panic is the right response to a bug.
    pub fn parse(bytes: &[u8]) -> Superblock {
        Superblock {
            magic: get_u32(bytes, 0),
            version: get_u32(bytes, 4),
            block_size: get_u32(bytes, 8),
            total_blocks: get_u32(bytes, 12),
            generation: get_u32(bytes, 16),
            root_dir: get_u32(bytes, 20),
            reserved: [get_u32(bytes, 24), get_u32(bytes, 28)],
            checksum: get_u32(bytes, CHECKSUM_AT),
        }
    }

    /// Encodes the superblock exactly as it goes on disk.
    pub fn to_bytes(&self) -> [u8; SUPER_SIZE] {
        let mut bytes = [0u8; SUPER_SIZE];
        put_u32(&mut bytes, 0, self.magic);
        put_u32(&mut bytes, 4, self.version);
        put_u32(&mut bytes, 8, self.block_size);
        put_u32(&mut bytes, 12, self.total_blocks);
        put_u32(&mut bytes, 16, self.generation);
        put_u32(&mut bytes, 20, self.root_dir);
        put_u32(&mut bytes, 24, self.reserved[0]);
        put_u32(&mut bytes, 28, self.reserved[1]);
        put_u32(&mut bytes, CHECKSUM_AT, self.checksum);
        bytes
    }

    /// A whole block holding this superblock, the rest zero.
    pub fn to_block(&self) -> Block {
        let mut block = empty_block();
        block[..SUPER_SIZE].copy_from_slice(&self.to_bytes());
        block
    }

    /// The same checksum as `TfsChecksum` in tfscore.c: add each byte,
    /// then rotate left by one, skipping the checksum field itself.
    ///
    /// In C, `unsigned` arithmetic silently wraps on overflow. In Rust,
    /// `+` on a u32 that overflows is treated as a bug and panics in
    /// debug builds. Wrapping is what we want here, so we have to say so
    /// with `wrapping_add` - the intent is visible in the code.
    pub fn compute_checksum(&self) -> u32 {
        let bytes = self.to_bytes();
        let mut sum: u32 = 0;

        for (i, &byte) in bytes.iter().enumerate() {
            if (CHECKSUM_AT..CHECKSUM_AT + 4).contains(&i) {
                continue;
            }
            sum = sum.wrapping_add(u32::from(byte));
            sum = sum.rotate_left(1);
        }
        sum
    }

    /// The same checks `TfsMount` applies before trusting a slot.
    pub fn is_valid(&self) -> bool {
        self.magic == MAGIC
            && (VERSION..=VERSION_NEWEST).contains(&self.version)
            && self.block_size == BLOCK_SIZE as u32
            && self.total_blocks != 0
            && self.total_blocks <= MAX_BLOCKS
            && self.checksum == self.compute_checksum()
    }
}

// ---- directory entries and blocks -----------------------------------

/// One name in a directory. On disk, a slot of 40 bytes: 28 bytes of
/// name, then inode, size, type. A name of 28 bytes or more (version 5)
/// goes on in a second slot right after: 36 more bytes of the name, NUL
/// padded, and the type TYPE_MORE.
///
/// ```text
///  slot:   [ name 0..28            ][ inode ][ size ][ type 1|2 ]
///  second: [ name 28..64                              ][ type 3  ]
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirEntry {
    /// NUL-padded. Not a `String`: on disk it is just bytes, and
    /// keeping it as bytes means a name round-trips exactly, even one
    /// that is not valid UTF-8.
    pub name: [u8; LONG_NAME_MAX],
    /// Block holding this file's inode. 0 marks an unused slot.
    pub inode: u32,
    pub size: u32,
    /// `type` is a reserved word in Rust, so the field is `kind`.
    pub kind: u32,
}

impl DirEntry {
    /// A new entry. The caller has already checked that `name` fits
    /// (`Volume` does, in `check_name`), so a too-long name here is a
    /// bug and is cut rather than overrunning anything.
    pub fn new(name: &str, inode: u32, size: u32, kind: u32) -> DirEntry {
        let mut field = [0u8; LONG_NAME_MAX];
        let bytes = name.as_bytes();
        let len = bytes.len().min(LONG_NAME_MAX);
        field[..len].copy_from_slice(&bytes[..len]);
        DirEntry {
            name: field,
            inode,
            size,
            kind,
        }
    }

    /// One slot's entry; a second slot's bytes are added by `DirBlock`.
    pub fn parse(bytes: &[u8]) -> DirEntry {
        let mut name = [0u8; LONG_NAME_MAX];
        name[..NAME_MAX].copy_from_slice(&bytes[..NAME_MAX]);
        DirEntry {
            name,
            inode: get_u32(bytes, 28),
            size: get_u32(bytes, 32),
            kind: get_u32(bytes, 36),
        }
    }

    /// Writes the entry's slots: `bytes` is room for `slots()` of them.
    pub fn write_to(&self, bytes: &mut [u8]) {
        bytes[..NAME_MAX].copy_from_slice(&self.name[..NAME_MAX]);
        put_u32(bytes, 28, self.inode);
        put_u32(bytes, 32, self.size);
        put_u32(bytes, 36, self.kind);
        if self.slots() == 2 {
            let more = &mut bytes[DIR_ENTRY_SIZE..2 * DIR_ENTRY_SIZE];
            more[..MORE_NAME].copy_from_slice(&self.name[NAME_MAX..]);
            put_u32(more, MORE_NAME, TYPE_MORE);
        }
    }

    /// How many slots it takes on disk: 2 for a name that does not fit
    /// the first slot with a NUL after it.
    pub fn slots(&self) -> usize {
        if self.name_bytes().len() >= NAME_MAX { 2 } else { 1 }
    }

    /// The name up to its first NUL, as raw bytes.
    pub fn name_bytes(&self) -> &[u8] {
        let end = self.name.iter().position(|&b| b == 0).unwrap_or(LONG_NAME_MAX);
        &self.name[..end]
    }

    /// The name for display. Invalid UTF-8 shows as U+FFFD rather than
    /// failing - a dump must never refuse to print what is on disk.
    pub fn name_lossy(&self) -> String {
        String::from_utf8_lossy(self.name_bytes()).into_owned()
    }

    /// Whether this entry is called `name`. Equivalent to `NameEqual`.
    pub fn is_named(&self, name: &str) -> bool {
        self.name_bytes() == name.as_bytes()
    }
}

/// One block of a directory. A directory is a chain of these, linked
/// through `next`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirBlock {
    pub magic: u32,
    /// The count of slots, as stored. Kept separately from
    /// `entries.len()` so a dump can report a corrupt count instead of
    /// hiding it.
    pub count: u32,
    /// 0 = this is the last block of the directory.
    pub next: u32,
    /// The entries in the first `min(count, DIR_ENTRIES)` slots - a
    /// long name's two slots one entry - including unused ones (inode
    /// 0), which callers skip.
    pub entries: Vec<DirEntry>,
}

impl DirBlock {
    pub fn new(entries: Vec<DirEntry>, next: u32) -> DirBlock {
        DirBlock {
            magic: MAGIC,
            count: entries.iter().map(DirEntry::slots).sum::<usize>() as u32,
            next,
            entries,
        }
    }

    pub fn parse(block: &Block) -> DirBlock {
        let count = get_u32(block, 4);
        let usable = (count as usize).min(DIR_ENTRIES);

        let slot = |i: usize| &block[DIR_HEADER + i * DIR_ENTRY_SIZE..DIR_HEADER + (i + 1) * DIR_ENTRY_SIZE];
        let is_more = |i: usize| i < usable && get_u32(slot(i), MORE_NAME) == TYPE_MORE;
        let mut entries = Vec::with_capacity(usable);
        let mut i = 0;
        while i < usable {
            if is_more(i) {
                // A second slot with no first before it: nothing a
                // volume writes, so nothing to trust - an unused slot.
                entries.push(DirEntry::parse(&[0u8; DIR_ENTRY_SIZE]));
                i += 1;
                continue;
            }
            let mut entry = DirEntry::parse(slot(i));
            if is_more(i + 1) {
                entry.name[NAME_MAX..].copy_from_slice(&slot(i + 1)[..MORE_NAME]);
                i += 1;
            }
            entries.push(entry);
            i += 1;
        }

        DirBlock {
            magic: get_u32(block, 0),
            count,
            next: get_u32(block, 8),
            entries,
        }
    }

    /// Encodes the block. Panics if there are more entries than fit:
    /// `Volume` splits directories into chunks of `DIR_ENTRIES`, so
    /// overfilling is a bug, not a condition to handle.
    pub fn to_block(&self) -> Block {
        let slots: usize = self.entries.iter().map(DirEntry::slots).sum();
        assert!(slots <= DIR_ENTRIES, "directory block overfilled");

        let mut block = empty_block();
        put_u32(&mut block, 0, self.magic);
        put_u32(&mut block, 4, self.count);
        put_u32(&mut block, 8, self.next);
        let mut at = DIR_HEADER;
        for entry in &self.entries {
            let length = entry.slots() * DIR_ENTRY_SIZE;
            entry.write_to(&mut block[at..at + length]);
            at += length;
        }
        block
    }
}

// ---- inodes ---------------------------------------------------------

/// A file: its size, and the blocks holding its data, in order.
///
/// Up to INODE_BLOCKS blocks, the inode names them itself. A bigger
/// file (only on a version-3 volume) is told apart by its block count,
/// which is then more than INODE_BLOCKS: its slots name index blocks
/// instead, each holding the numbers of the next INDEX_BLOCKS of its
/// data blocks, the last one only as many as are left. So a small file
/// is laid out exactly as before, and a big one costs one block in
/// 1025 more.
///
/// A stamped inode (magic MAGIC_STAMPED, version 4) also says when the
/// file last changed: the generation of the commit that made the change
/// live. Generations only grow, on TheTaaJ and in the host's tools
/// alike, so of two files the one with the higher stamp was changed
/// later - which is what `make` asks. Its header is 8 bytes longer, so
/// it names STAMPED_BLOCKS blocks itself, two fewer. An inode written
/// before version 4 keeps its layout and has stamp 0: older than
/// anything changed since.
///
/// A timed inode (magic MAGIC_TIMED, version 6) carries a stamp and,
/// after it, the time of the change: nanoseconds since 1 January 1970,
/// UTC, as the clock of whoever wrote it said - TheTaaJ's or the host
/// tool's. That is what `ls -l` and `stat` show. Order still comes from
/// the stamp; and the volume never writes a time older than one it
/// already holds (see `Volume`), so times order changes too. Its header
/// is 32 bytes: TIMED_BLOCKS slots, four fewer than the oldest layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inode {
    pub magic: u32,
    pub size: u32,
    pub kind: u32,
    /// The block count as stored, kept for the same reason as
    /// `DirBlock::count`.
    pub block_count: u32,
    /// Only the slots in use: `block_count` data blocks for a small
    /// file, or `block_count.div_ceil(INDEX_BLOCKS)` index blocks for a
    /// big one (`is_big`).
    ///
    /// The C struct always carries all 1020 slots. Keeping just the
    /// used ones means `blocks.len()` *is* the count when writing, so
    /// the two can never disagree - and the slots past the end, which C
    /// could leave holding stale numbers, simply do not exist.
    pub blocks: Vec<u32>,
    /// When it last changed (see above); 0 for an unstamped inode.
    pub stamp: u64,
    /// The time of that change, in nanoseconds since 1970; 0 for an
    /// inode that is not timed.
    pub time: u64,
}

impl Inode {
    /// An unstamped inode, as before version 4.
    pub fn new(size: u32, kind: u32, blocks: Vec<u32>) -> Inode {
        Inode {
            magic: MAGIC,
            size,
            kind,
            block_count: blocks.len() as u32,
            blocks,
            stamp: 0,
            time: 0,
        }
    }

    /// A big file's inode: `block_count` data blocks, named by the index
    /// blocks `index`.
    pub fn big(size: u32, kind: u32, block_count: u32, index: Vec<u32>) -> Inode {
        Inode { magic: MAGIC, size, kind, block_count, blocks: index, stamp: 0, time: 0 }
    }

    /// The same inode, stamped. Its blocks must fit a stamped inode: a
    /// small file of more than STAMPED_BLOCKS is a big one when stamped.
    pub fn stamped(self, stamp: u64) -> Inode {
        Inode { magic: MAGIC_STAMPED, stamp, time: 0, ..self }
    }

    /// The same inode, stamped and timed. Its blocks must fit a timed
    /// inode: more than TIMED_BLOCKS make it a big one.
    pub fn timed(self, stamp: u64, time: u64) -> Inode {
        Inode { magic: MAGIC_TIMED, stamp, time, ..self }
    }

    /// Whether it carries a stamp (and has a longer header): a stamped
    /// inode or a timed one.
    pub fn is_stamped(&self) -> bool {
        self.magic == MAGIC_STAMPED || self.magic == MAGIC_TIMED
    }

    /// Whether it carries a time.
    pub fn is_timed(&self) -> bool {
        self.magic == MAGIC_TIMED
    }

    /// Whether it is an inode at all: any of the three magics.
    pub fn is_valid(&self) -> bool {
        self.magic == MAGIC || self.magic == MAGIC_STAMPED || self.magic == MAGIC_TIMED
    }

    /// How many block numbers it holds itself.
    pub fn slots(&self) -> usize {
        Self::slots_for(self.magic)
    }

    pub fn slots_for(magic: u32) -> usize {
        match magic {
            MAGIC_TIMED => TIMED_BLOCKS,
            MAGIC_STAMPED => STAMPED_BLOCKS,
            _ => INODE_BLOCKS,
        }
    }

    fn header_for(magic: u32) -> usize {
        match magic {
            MAGIC_TIMED => TIMED_HEADER,
            MAGIC_STAMPED => STAMPED_HEADER,
            _ => INODE_HEADER,
        }
    }

    fn header(&self) -> usize {
        Self::header_for(self.magic)
    }

    /// Whether the slots name index blocks, not data blocks.
    pub fn is_big(&self) -> bool {
        self.block_count as usize > self.slots()
    }

    pub fn parse(block: &Block) -> Inode {
        let magic = get_u32(block, 0);
        let slots = Self::slots_for(magic);
        let block_count = get_u32(block, 12);
        let usable = if block_count as usize > slots {
            // A count past what its index blocks reach is damage; the
            // slots that exist are read, and the volume finds the rest
            // missing.
            (block_count as usize).div_ceil(INDEX_BLOCKS).min(slots)
        } else {
            block_count as usize
        };
        let u64_at = |at: usize| u64::from(get_u32(block, at)) | u64::from(get_u32(block, at + 4)) << 32;
        let header = Self::header_for(magic);
        let stamp = if magic == MAGIC_STAMPED || magic == MAGIC_TIMED { u64_at(16) } else { 0 };
        let time = if magic == MAGIC_TIMED { u64_at(24) } else { 0 };
        let blocks = (0..usable)
            .map(|i| get_u32(block, header + i * 4))
            .collect();

        Inode {
            magic,
            size: get_u32(block, 4),
            kind: get_u32(block, 8),
            block_count,
            blocks,
            stamp,
            time,
        }
    }

    /// Encodes the inode. For a small file `block_count` is written
    /// from `blocks.len()`, so changing the list is enough - there is no
    /// count to forget. A big file's count is its own (`Inode::big`).
    pub fn to_block(&self) -> Block {
        assert!(self.blocks.len() <= self.slots(), "inode overfilled");
        let count = if self.is_big() {
            assert_eq!(self.blocks.len(), (self.block_count as usize).div_ceil(INDEX_BLOCKS), "index blocks miscounted");
            self.block_count
        } else {
            self.blocks.len() as u32
        };

        let mut block = empty_block();
        put_u32(&mut block, 0, self.magic);
        put_u32(&mut block, 4, self.size);
        put_u32(&mut block, 8, self.kind);
        put_u32(&mut block, 12, count);
        if self.is_stamped() {
            put_u32(&mut block, 16, self.stamp as u32);
            put_u32(&mut block, 20, (self.stamp >> 32) as u32);
        }
        if self.is_timed() {
            put_u32(&mut block, 24, self.time as u32);
            put_u32(&mut block, 28, (self.time >> 32) as u32);
        }
        let header = self.header();
        for (i, &number) in self.blocks.iter().enumerate() {
            put_u32(&mut block, header + i * 4, number);
        }
        block
    }
}

// ---- tests ----------------------------------------------------------
//
// `cargo test` compiles this module and runs every `#[test]` function.
// `#[cfg(test)]` means none of it is in the normal build.

#[cfg(test)]
mod tests {
    use super::*;

    /// Slot A of a volume made by the C tool: `tfstool mkfs img 4096`.
    /// These are the real bytes from disk, so if the Rust and C
    /// implementations ever disagree, these tests fail.
    const C_FRESH_SUPER: [u8; SUPER_SIZE] = [
        0x4a, 0x41, 0x46, 0x53, // magic "JAFS"
        0x01, 0x00, 0x00, 0x00, // version 1
        0x00, 0x10, 0x00, 0x00, // block size 4096
        0x00, 0x10, 0x00, 0x00, // total blocks 4096
        0x01, 0x00, 0x00, 0x00, // generation 1
        0x02, 0x00, 0x00, 0x00, // root directory in block 2
        0x00, 0x00, 0x00, 0x00, // reserved
        0x00, 0x00, 0x00, 0x00, // reserved
        0x86, 0x20, 0x81, 0x78, // checksum
    ];

    #[test]
    fn layout_constants_match_tfs_h() {
        assert_eq!(DIR_ENTRIES, 102);
        assert_eq!(INODE_BLOCKS, 1020);
        assert_eq!(DIRECT_MAX, 1020 * 4096);
        assert_eq!(MAX_FILE_SIZE, 1016 * 1024 * 4096);
        assert!(MAX_FILE_SIZE <= u32::MAX as usize, "a file's size is 32 bits");
        assert_eq!(MAX_BLOCKS as usize * BLOCK_SIZE, 4 << 30);
    }

    #[test]
    fn a_timed_inode_round_trips_with_stamp_and_time() {
        let inode = Inode::new(9000, TYPE_FILE, vec![10, 11, 12]).timed(0x1_0000_0007, 0x1880_0000_0000_0042);
        let block = inode.to_block();

        assert_eq!(&block[0..4], b"JAFU");
        assert_eq!(&block[16..24], &[7, 0, 0, 0, 1, 0, 0, 0]);
        assert_eq!(&block[24..32], &[0x42, 0, 0, 0, 0, 0, 0x80, 0x18]);
        assert_eq!(&block[32..36], &[10, 0, 0, 0]);
        let back = Inode::parse(&block);
        assert_eq!(back, inode);
        assert!(back.is_valid() && back.is_stamped() && back.is_timed() && !back.is_big());
        assert_eq!((TIMED_BLOCKS, back.slots()), (1016, 1016));
        let big = Inode::big(0, TYPE_FILE, TIMED_BLOCKS as u32 + 1, vec![40]).timed(3, 4);
        assert!(Inode::parse(&big.to_block()).is_big());
    }

    #[test]
    fn a_superblock_of_a_volume_over_256_mib_is_new_codes_only() {
        let sb = Superblock::new(OLD_MAX_BLOCKS + 1, 1, FIRST_DATA);
        assert!(sb.is_valid(), "this code mounts it");
        assert!(sb.total_blocks > OLD_MAX_BLOCKS, "older code, checking against 65536, does not");
        assert!(!Superblock::new(MAX_BLOCKS + 1, 1, FIRST_DATA).is_valid());
        let mut six = Superblock::new(4096, 1, FIRST_DATA);
        six.version = VERSION_TIMES;
        assert!(six.sealed().is_valid());
        six.version = VERSION_TIMES + 1;
        assert!(!six.sealed().is_valid());
    }

    #[test]
    fn parses_a_superblock_written_by_the_c_tool() {
        let sb = Superblock::parse(&C_FRESH_SUPER);

        assert_eq!(sb.magic, MAGIC);
        assert_eq!(sb.total_blocks, 4096);
        assert_eq!(sb.generation, 1);
        assert_eq!(sb.root_dir, FIRST_DATA);
        assert!(sb.is_valid());
    }

    #[test]
    fn writes_the_same_superblock_bytes_as_the_c_tool() {
        assert_eq!(
            Superblock::new(4096, 1, FIRST_DATA).to_bytes(),
            C_FRESH_SUPER
        );
    }

    #[test]
    fn checksum_matches_the_c_tool_after_a_commit() {
        // Slot B after one `put`: generation 2, root moved to block 4.
        assert_eq!(Superblock::new(4096, 2, 4).checksum, 0x7882_4086);
    }

    #[test]
    fn any_damaged_superblock_byte_is_detected() {
        // Each step of the checksum is reversible (add, then rotate), so
        // changing any single byte must change the result. This checks
        // that claim for every byte rather than trusting it.
        for i in 0..SUPER_SIZE {
            let mut damaged = C_FRESH_SUPER;
            damaged[i] ^= 0xFF;
            assert!(
                !Superblock::parse(&damaged).is_valid(),
                "damage to byte {i} went unnoticed"
            );
        }
    }

    #[test]
    fn an_all_zero_slot_is_invalid() {
        // What mkfs leaves in slot B on a fresh volume.
        assert!(!Superblock::parse(&empty_block()).is_valid());
    }

    #[test]
    fn directory_block_round_trips() {
        let dir = DirBlock::new(
            vec![
                DirEntry::new("hello.txt", 5, 3, TYPE_FILE),
                DirEntry::new("b", 9, 0, TYPE_FILE),
            ],
            7,
        );
        let back = DirBlock::parse(&dir.to_block());

        assert_eq!(back, dir);
        assert!(back.entries[0].is_named("hello.txt"));
        assert!(!back.entries[0].is_named("hello"));
        assert_eq!(back.entries[1].name_lossy(), "b");
    }

    #[test]
    fn directory_entry_layout_matches_tfs_h() {
        let mut bytes = [0u8; DIR_ENTRY_SIZE];
        DirEntry::new("ab", 5, 3, TYPE_FILE).write_to(&mut bytes);

        assert_eq!(&bytes[..3], b"ab\0");
        assert_eq!(&bytes[28..40], &[5, 0, 0, 0, 3, 0, 0, 0, 1, 0, 0, 0]);
    }

    #[test]
    fn inode_round_trips_and_ignores_stale_slots() {
        let inode = Inode::new(5000, TYPE_FILE, vec![10, 11]);
        let mut block = inode.to_block();

        // Garbage past block_count - what the old C TfsCreate could leave.
        put_u32(&mut block, INODE_HEADER + 2 * 4, 0xDEAD);

        assert_eq!(Inode::parse(&block), inode);
    }

    #[test]
    fn a_stamped_inode_round_trips_with_its_longer_header() {
        let inode = Inode::new(9000, TYPE_FILE, vec![10, 11, 12]).stamped(0x1_0000_0007);
        let block = inode.to_block();

        assert_eq!(&block[0..4], b"JAFT");
        assert_eq!(&block[16..24], &[7, 0, 0, 0, 1, 0, 0, 0]);
        assert_eq!(&block[24..28], &[10, 0, 0, 0]);
        let back = Inode::parse(&block);
        assert_eq!(back, inode);
        assert!(back.is_valid() && back.is_stamped() && !back.is_big());
        assert_eq!(STAMPED_BLOCKS, 1018);
    }

    #[test]
    fn an_unstamped_inode_reads_as_stamp_zero_and_keeps_its_slots() {
        let inode = Inode::new(0, TYPE_FILE, (1..=INODE_BLOCKS as u32).collect());
        let back = Inode::parse(&inode.to_block());

        assert_eq!((back.stamp, back.slots(), back.is_big()), (0, INODE_BLOCKS, false));
        assert_eq!(back.blocks.len(), INODE_BLOCKS);
    }

    #[test]
    fn a_stamped_inode_past_its_slots_is_a_big_one() {
        let inode = Inode::big(0, TYPE_FILE, STAMPED_BLOCKS as u32 + 1, vec![40]).stamped(3);
        let back = Inode::parse(&inode.to_block());

        assert!(back.is_big());
        assert_eq!(back.blocks, [40]);
    }

    #[test]
    fn a_long_name_takes_a_second_slot() {
        let long = "a-name-longer-than-twenty-seven-bytes-up-to-sixty-four.c";
        let full = "x".repeat(LONG_NAME_MAX);
        let entries = vec![
            DirEntry::new("short", 5, 1, TYPE_FILE),
            DirEntry::new(long, 6, 2, TYPE_FILE),
            DirEntry::new(&"y".repeat(NAME_MAX), 7, 0, TYPE_DIR),
            DirEntry::new(&full, 8, 3, TYPE_FILE),
            DirEntry::new(&"z".repeat(NAME_MAX - 1), 9, 4, TYPE_FILE),
        ];
        assert_eq!(entries.iter().map(DirEntry::slots).collect::<Vec<_>>(), [1, 2, 2, 2, 1]);
        let dir = DirBlock::new(entries.clone(), 0);
        assert_eq!(dir.count, 8, "slots, not entries");
        let block = dir.to_block();
        let second = DIR_HEADER + 2 * DIR_ENTRY_SIZE;
        assert_eq!(&block[second..second + 4], &long.as_bytes()[NAME_MAX..NAME_MAX + 4]);
        assert_eq!(get_u32(&block, second + MORE_NAME), TYPE_MORE);
        let back = DirBlock::parse(&block);
        assert_eq!(back.entries, entries);
        assert_eq!(back.entries[1].name_lossy(), long);
        assert!(back.entries[3].is_named(&full));
        assert!(!back.entries[3].is_named(&full[..NAME_MAX]), "the first slot alone is not the name");
    }

    #[test]
    fn short_names_are_laid_out_as_before() {
        let entries: Vec<DirEntry> = (0..DIR_ENTRIES as u32).map(|n| DirEntry::new(&format!("f{n}"), n + 3, n, TYPE_FILE)).collect();
        let block = DirBlock::new(entries, 0).to_block();
        for n in 0..DIR_ENTRIES {
            let at = DIR_HEADER + n * DIR_ENTRY_SIZE;
            let name = format!("f{n}");
            assert_eq!(&block[at..at + name.len() + 1], format!("{name}\0").as_bytes());
            assert_eq!(get_u32(&block, at + 28), n as u32 + 3);
        }
    }

    #[test]
    fn a_second_slot_alone_is_an_unused_one() {
        let mut block = DirBlock::new(vec![DirEntry::new("a", 5, 0, TYPE_FILE), DirEntry::new("b", 6, 0, TYPE_FILE)], 0).to_block();
        put_u32(&mut block, DIR_HEADER + MORE_NAME, TYPE_MORE);
        let dir = DirBlock::parse(&block);
        assert_eq!(dir.entries.len(), 2);
        assert_eq!(dir.entries[0].inode, 0);
        assert!(dir.entries[1].is_named("b"));
    }

    #[test]
    fn a_corrupt_count_is_clamped_not_trusted() {
        let mut block = DirBlock::new(vec![], 0).to_block();
        put_u32(&mut block, 4, 1_000_000);

        let dir = DirBlock::parse(&block);
        assert_eq!(dir.count, 1_000_000);
        assert_eq!(dir.entries.len(), DIR_ENTRIES);
    }
}
