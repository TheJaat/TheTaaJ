//! The filesystem: mount, read, and copy-on-write changes.
//!
//! This is the Rust counterpart of `servers/tfs/tfscore.c`, and it makes
//! the same decisions in the same order, so both produce identical
//! images for the same sequence of operations. `crosscheck.sh` holds
//! them to that.
//!
//! The copy-on-write rules, as in C:
//!   - a block reachable from the committed superblock is never written
//!   - a change writes new blocks, then a new directory, then commits by
//!     writing ONE superblock into the slot that is not live
//!   - there is no free map on disk; it is rebuilt from the tree

use alloc::collections::BTreeSet;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use core::fmt;

use crate::device::{BlockDevice, DeviceError};
use crate::format::{
    empty_block, Block, DirBlock, DirEntry, Inode, Superblock, BLOCK_SIZE, DIR_ENTRIES, FIRST_DATA,
    MAGIC, MAX_BLOCKS, MAX_FILE_SIZE, NAME_MAX, SUPER_A, SUPER_B, TYPE_FILE,
};

/// The most blocks a directory may span. The same limit as the guard in
/// `TfsRewriteDirectory`: 64 blocks of 102 entries.
const MAX_DIR_BLOCKS: usize = 64;

// ---- errors ---------------------------------------------------------

/// Everything that can go wrong.
///
/// In C this is a set of negative numbers, and nothing stops a caller
/// from ignoring one or from returning -4 where -5 was meant. A Rust
/// enum is a closed set of cases: a `match` on it must handle every
/// one, and a `Device` error carries what the device said instead of
/// flattening it to -1.
#[derive(Debug)]
pub enum Error {
    Device(DeviceError),
    Magic,
    NoSuper,
    Full,
    NotFound,
    Exists,
    Name,
    TooBig,
    BadSize,
    /// The tree is damaged in a way that would make blind trust
    /// dangerous: a loop, or a block number outside the volume.
    Corrupt(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Device(e) => write!(f, "device error: {e}"),
            Error::Magic => f.write_str("not a TaajFS volume"),
            Error::NoSuper => f.write_str("no valid superblock in either slot"),
            Error::Full => f.write_str("no free blocks"),
            Error::NotFound => f.write_str("not found"),
            Error::Exists => f.write_str("already exists"),
            Error::Name => write!(f, "name must be 1 to {} bytes", NAME_MAX - 1),
            Error::TooBig => f.write_str("file would exceed the maximum size"),
            Error::BadSize => write!(f, "a volume must be 8 to {MAX_BLOCKS} blocks"),
            Error::Corrupt(what) => write!(f, "volume is damaged: {what}"),
        }
    }
}

impl core::error::Error for Error {}

/// Lets `?` turn a `DeviceError` into our `Error` automatically.
impl From<DeviceError> for Error {
    fn from(e: DeviceError) -> Error {
        Error::Device(e)
    }
}

/// Shorthand used everywhere below.
pub type Result<T> = core::result::Result<T, Error>;

// ---- what callers see -----------------------------------------------

/// A file as listed in the directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileInfo {
    pub name: String,
    pub inode: u32,
    pub size: u32,
    pub kind: u32,
}

impl From<&DirEntry> for FileInfo {
    fn from(entry: &DirEntry) -> FileInfo {
        FileInfo {
            name: entry.name_lossy(),
            inode: entry.inode,
            size: entry.size,
            kind: entry.kind,
        }
    }
}

/// Leading slashes are accepted and ignored: TaajFS is flat, so "/a"
/// and "a" are the same file.
///
/// The lifetime is inferred: the result borrows from `name`, so it is
/// valid exactly as long as `name` is. No copy is made.
fn trim_slashes(name: &str) -> &str {
    name.trim_start_matches('/')
}

/// The name rules `TfsCreate` enforces: not empty, and short enough to
/// leave room for the terminating NUL in the 28-byte field.
fn check_name(name: &str) -> Result<&str> {
    let name = trim_slashes(name);
    if name.is_empty() || name.len() >= NAME_MAX || name.contains('\0') {
        return Err(Error::Name);
    }
    Ok(name)
}

// ---- formatting -----------------------------------------------------

/// Writes an empty volume: an empty root directory in block 2, a
/// superblock in slot A, and slot B deliberately invalid.
///
/// `D: BlockDevice` is a *generic*: this works for any device type, and
/// the compiler generates a version for each one used. No function
/// pointers, no `void *Context`.
pub fn format<D: BlockDevice>(device: &mut D, total_blocks: u32) -> Result<()> {
    if !(8..=MAX_BLOCKS).contains(&total_blocks) {
        return Err(Error::BadSize);
    }
    device.write_block(FIRST_DATA, &DirBlock::new(Vec::new(), 0).to_block())?;
    device.write_block(
        SUPER_A,
        &Superblock::new(total_blocks, 1, FIRST_DATA).to_block(),
    )?;
    device.write_block(SUPER_B, &empty_block())?;
    Ok(())
}

// ---- the mounted volume ---------------------------------------------

pub struct Volume<D: BlockDevice> {
    device: D,
    /// The live superblock, and which slot it came from.
    sb: Superblock,
    slot: u32,
    /// Which blocks the committed tree uses. Derived, never stored.
    used: Vec<bool>,
    used_count: u32,
}

impl<D: BlockDevice> Volume<D> {
    /// Picks the newest valid superblock and rebuilds the free map.
    ///
    /// Takes the device by value: the volume owns it from now on, so
    /// nothing else can write to the disk behind the volume's back.
    pub fn mount(mut device: D) -> Result<Volume<D>> {
        let mut best: Option<(u32, Superblock)> = None;
        let mut block = empty_block();

        for slot in [SUPER_A, SUPER_B] {
            // A slot that cannot be read is treated like an invalid one.
            if device.read_block(slot, &mut block).is_err() {
                continue;
            }
            let sb = Superblock::parse(&block);
            if !sb.is_valid() {
                continue;
            }
            // Strictly greater: on a tie, slot A wins, as in C.
            let newer = match best {
                None => true,
                Some((_, current)) => sb.generation > current.generation,
            };
            if newer {
                best = Some((slot, sb));
            }
        }

        let (slot, sb) = best.ok_or(Error::NoSuper)?;
        let mut volume = Volume {
            device,
            sb,
            slot,
            used: Vec::new(),
            used_count: 0,
        };
        volume.rebuild_free_map()?;
        Ok(volume)
    }

    pub fn superblock(&self) -> &Superblock {
        &self.sb
    }

    pub fn free_blocks(&self) -> u32 {
        self.sb.total_blocks - self.used_count
    }

    /// Unmounts and hands the device back.
    pub fn into_device(self) -> D {
        self.device
    }

    // ---- block helpers ----------------------------------------------

    fn read(&mut self, number: u32) -> Result<Block> {
        let mut block = empty_block();
        self.device.read_block(number, &mut block)?;
        Ok(block)
    }

    fn write(&mut self, number: u32, block: &Block) -> Result<()> {
        Ok(self.device.write_block(number, block)?)
    }

    fn read_dir_block(&mut self, number: u32) -> Result<DirBlock> {
        let dir = DirBlock::parse(&self.read(number)?);
        if dir.magic != MAGIC {
            return Err(Error::Magic);
        }
        Ok(dir)
    }

    fn read_inode(&mut self, number: u32) -> Result<Inode> {
        let inode = Inode::parse(&self.read(number)?);
        if inode.magic != MAGIC {
            return Err(Error::Magic);
        }
        Ok(inode)
    }

    /// Every block of the root directory, in chain order.
    ///
    /// C guards against a looping chain with a counter. A set of the
    /// blocks already visited catches a loop on its first repeat, and
    /// says exactly where.
    fn directory_chain(&mut self) -> Result<Vec<(u32, DirBlock)>> {
        let mut chain = Vec::new();
        let mut seen = BTreeSet::new();
        let mut next = self.sb.root_dir;

        while next != 0 {
            if next >= self.sb.total_blocks {
                return Err(Error::Corrupt(format!(
                    "directory block {next} is outside the volume"
                )));
            }
            // `insert` returns false if the value was already there.
            if !seen.insert(next) {
                return Err(Error::Corrupt(format!(
                    "directory chain loops at block {next}"
                )));
            }
            let dir = self.read_dir_block(next)?;
            let following = dir.next;
            chain.push((next, dir));
            next = following;
        }
        Ok(chain)
    }

    /// Every live entry in the root directory (unused slots skipped).
    fn entries(&mut self) -> Result<Vec<DirEntry>> {
        Ok(self
            .directory_chain()?
            .into_iter()
            .flat_map(|(_, dir)| dir.entries)
            .filter(|entry| entry.inode != 0)
            .collect())
    }

    // ---- the free map -----------------------------------------------

    fn mark_used(&mut self, number: u32) -> Result<()> {
        // `get_mut` returns None for an index past the end, instead of
        // writing past the array as an unchecked C index would.
        match self.used.get_mut(number as usize) {
            Some(slot) => {
                if !*slot {
                    *slot = true;
                    self.used_count += 1;
                }
                Ok(())
            }
            None => Err(Error::Corrupt(format!(
                "block {number} is outside the volume"
            ))),
        }
    }

    /// Walks the committed tree and marks everything reachable. This is
    /// the whole allocator: whatever the tree does not reach is free.
    fn rebuild_free_map(&mut self) -> Result<()> {
        self.used = vec![false; self.sb.total_blocks as usize];
        self.used_count = 0;
        self.mark_used(SUPER_A)?;
        self.mark_used(SUPER_B)?;

        // `directory_chain` returns an owned Vec, so the loop below owns
        // the blocks and is free to call `&mut self` methods while
        // iterating. Iterating something borrowed from `self` here would
        // not compile - the borrow checker forbids changing a thing
        // while you are reading through it.
        for (number, dir) in self.directory_chain()? {
            self.mark_used(number)?;
            for entry in dir.entries.iter().filter(|e| e.inode != 0) {
                self.mark_used(entry.inode)?;
                let inode = self.read_inode(entry.inode)?;
                for &block in inode.blocks.iter().filter(|&&b| b != 0) {
                    self.mark_used(block)?;
                }
            }
        }
        Ok(())
    }

    /// Hands out the lowest free block. Nothing is written: a block
    /// allocated and then not committed is free again at the next
    /// rebuild, because the committed tree does not reach it.
    fn allocate(&mut self) -> Result<u32> {
        let start = FIRST_DATA as usize;
        let index = self.used[start..]
            .iter()
            .position(|&used| !used)
            .map(|i| i + start)
            .ok_or(Error::Full)?;

        self.used[index] = true;
        self.used_count += 1;
        Ok(index as u32)
    }

    // ---- committing -------------------------------------------------

    /// Publishes a new root with one block write, into the slot that is
    /// NOT live. Until it lands the volume reads as before; after it
    /// lands every change is visible at once.
    fn commit(&mut self, new_root: u32) -> Result<()> {
        let target = if self.slot == SUPER_A {
            SUPER_B
        } else {
            SUPER_A
        };

        // A wrapped generation would make the new superblock look OLDER
        // than the one it replaces, and mount would pick the old one.
        let generation = self
            .sb
            .generation
            .checked_add(1)
            .ok_or_else(|| Error::Corrupt("generation counter exhausted".to_string()))?;

        // `..self.sb` copies every field not named - struct update syntax.
        let sb = Superblock {
            root_dir: new_root,
            generation,
            ..self.sb
        }
        .sealed();
        self.write(target, &sb.to_block())?;

        self.sb = sb;
        self.slot = target;

        // The old tree is unreachable now; rebuilding is what frees it.
        self.rebuild_free_map()
    }

    /// Runs a change. If it fails partway, the blocks it allocated were
    /// never committed, so the free map is rebuilt to return them.
    ///
    /// `impl FnOnce(&mut Self) -> Result<T>` accepts any closure that
    /// takes the volume. The change itself is written inline at each
    /// call site; this wrapper supplies the cleanup, once.
    fn change<T>(&mut self, operation: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        let result = operation(self);
        if result.is_err() {
            // The original error is the one worth reporting.
            let _ = self.rebuild_free_map();
        }
        result
    }

    /// Writes a complete new copy of the directory with one entry
    /// replaced, added (`replacement` named something new) or removed
    /// (`replacement` None), and returns its first block.
    fn rewrite_directory(&mut self, name: &str, replacement: Option<DirEntry>) -> Result<u32> {
        let mut entries = self.entries()?;
        let position = entries.iter().position(|e| e.is_named(name));

        // Matching on a pair covers all four cases, and the compiler
        // checks none was forgotten.
        match (position, replacement) {
            (Some(i), Some(entry)) => entries[i] = entry,
            (Some(i), None) => {
                entries.remove(i);
            }
            (None, Some(entry)) => entries.push(entry),
            (None, None) => return Err(Error::NotFound),
        }

        // Always at least one block: an empty directory still needs a
        // block for the superblock to point at.
        let chunks: Vec<&[DirEntry]> = if entries.is_empty() {
            vec![&entries[..]]
        } else {
            entries.chunks(DIR_ENTRIES).collect()
        };
        if chunks.len() > MAX_DIR_BLOCKS {
            return Err(Error::Full);
        }

        // Allocate every block first, so each one can be written once,
        // already pointing at the next. C streams instead, patching the
        // previous block's `next` afterwards - to keep a 4 KB-per-entry
        // array off a small ring-3 stack. The host has room, and the
        // blocks come out identical either way.
        let mut numbers = Vec::with_capacity(chunks.len());
        for _ in 0..chunks.len() {
            numbers.push(self.allocate()?);
        }

        for (i, chunk) in chunks.iter().enumerate() {
            let next = numbers.get(i + 1).copied().unwrap_or(0);
            let block = DirBlock::new(chunk.to_vec(), next).to_block();
            self.write(numbers[i], &block)?;
        }
        Ok(numbers[0])
    }

    // ---- reading ----------------------------------------------------

    pub fn list(&mut self) -> Result<Vec<FileInfo>> {
        Ok(self.entries()?.iter().map(FileInfo::from).collect())
    }

    pub fn lookup(&mut self, name: &str) -> Result<FileInfo> {
        let name = trim_slashes(name);
        self.entries()?
            .iter()
            .find(|entry| entry.is_named(name))
            .map(FileInfo::from)
            .ok_or(Error::NotFound)
    }

    /// Reads from `offset` into `buffer`; returns how many bytes it got.
    pub fn read_at(&mut self, file: &FileInfo, offset: u32, buffer: &mut [u8]) -> Result<usize> {
        if offset >= file.size {
            return Ok(0);
        }
        let length = buffer.len().min((file.size - offset) as usize);
        let inode = self.read_inode(file.inode)?;
        let mut done = 0;

        while done < length {
            let at = offset as usize + done;
            let index = at / BLOCK_SIZE;
            let within = at % BLOCK_SIZE;
            let chunk = (BLOCK_SIZE - within).min(length - done);

            // `let ... else`: take the block number if there is one,
            // otherwise leave the loop. A file shorter than its size
            // claims ends early rather than reading garbage.
            let Some(&number) = inode.blocks.get(index) else {
                break;
            };

            let block = self.read(number)?;
            buffer[done..done + chunk].copy_from_slice(&block[within..within + chunk]);
            done += chunk;
        }
        Ok(done)
    }

    /// The whole file.
    pub fn read_all(&mut self, file: &FileInfo) -> Result<Vec<u8>> {
        let mut data = vec![0u8; file.size as usize];
        let got = self.read_at(file, 0, &mut data)?;
        data.truncate(got);
        Ok(data)
    }

    // ---- changing ---------------------------------------------------
    //
    // Each of these commits before returning: success means it reached
    // the disk, and failure - at ANY point - leaves the previous state.

    /// An empty file. The same as `TfsCreate`.
    pub fn create(&mut self, name: &str) -> Result<()> {
        let name = check_name(name)?;
        match self.lookup(name) {
            Ok(_) => return Err(Error::Exists),
            Err(Error::NotFound) => {}
            Err(other) => return Err(other),
        }

        self.change(|v| {
            let inode = v.allocate()?;
            v.write(inode, &Inode::new(0, TYPE_FILE, Vec::new()).to_block())?;
            let root = v.rewrite_directory(name, Some(DirEntry::new(name, inode, 0, TYPE_FILE)))?;
            v.commit(root)
        })
    }

    /// Writes `data` at `offset` into an existing file, growing it if
    /// needed. The same as `TfsWriteFile`, block for block.
    pub fn write_at(&mut self, name: &str, offset: u32, data: &[u8]) -> Result<usize> {
        let end = offset as usize + data.len();
        if end > MAX_FILE_SIZE {
            return Err(Error::TooBig);
        }
        let file = self.lookup(name)?;

        self.change(|v| {
            let mut inode = v.read_inode(file.inode)?;
            let new_size = file.size.max(end as u32);
            let mut done = 0;

            while done < data.len() {
                let at = offset as usize + done;
                let index = at / BLOCK_SIZE;
                let within = at % BLOCK_SIZE;
                let chunk = (BLOCK_SIZE - within).min(data.len() - done);

                // Start from the old contents, so the bytes of the block
                // this write does not touch survive...
                let mut block = match inode.blocks.get(index) {
                    Some(&old) if old != 0 => v.read(old)?,
                    _ => empty_block(),
                };
                block[within..within + chunk].copy_from_slice(&data[done..done + chunk]);

                // ...and write the result somewhere NEW. The old block is
                // untouched: the committed tree still points at it.
                let new = v.allocate()?;
                v.write(new, &block)?;

                if index < inode.blocks.len() {
                    inode.blocks[index] = new;
                } else {
                    // A write past the end can leave a gap. Fill it with
                    // real zeroed blocks, never holes: a hole read as a
                    // stale number would return another file's data.
                    let gap_start = inode.blocks.len();
                    inode.blocks.resize(index + 1, 0);
                    inode.blocks[index] = new;
                    // `&mut` over a range of the Vec: each `slot` is a
                    // mutable reference to one entry, written in place.
                    for slot in &mut inode.blocks[gap_start..index] {
                        let fill = v.allocate()?;
                        v.write(fill, &empty_block())?;
                        *slot = fill;
                    }
                }
                done += chunk;
            }

            let new_inode = v.allocate()?;
            v.write(
                new_inode,
                &Inode::new(new_size, inode.kind, inode.blocks).to_block(),
            )?;

            // `file.name` came from the disk, so it is already in its
            // stored form. (C passes the caller's name here, and a
            // leading '/' would make it add a second entry instead of
            // replacing the first.)
            let entry = DirEntry::new(&file.name, new_inode, new_size, TYPE_FILE);
            let root = v.rewrite_directory(&file.name, Some(entry))?;
            v.commit(root)?;
            Ok(done)
        })
    }

    /// Replaces a file's whole contents - creating it if needed - in ONE
    /// commit.
    ///
    /// The C tool does `put` as create-then-write: two commits, and an
    /// overwrite with shorter data keeps the old tail, because writing
    /// at offset 0 never shrinks a file. Building a fresh inode instead
    /// makes `put` both atomic and exact.
    pub fn put(&mut self, name: &str, data: &[u8]) -> Result<()> {
        let name = check_name(name)?;
        if data.len() > MAX_FILE_SIZE {
            return Err(Error::TooBig);
        }

        self.change(|v| {
            let mut blocks = Vec::with_capacity(data.len().div_ceil(BLOCK_SIZE));
            for piece in data.chunks(BLOCK_SIZE) {
                let mut block = empty_block();
                block[..piece.len()].copy_from_slice(piece);
                let number = v.allocate()?;
                v.write(number, &block)?;
                blocks.push(number);
            }

            let size = data.len() as u32;
            let inode = v.allocate()?;
            v.write(inode, &Inode::new(size, TYPE_FILE, blocks).to_block())?;
            let root =
                v.rewrite_directory(name, Some(DirEntry::new(name, inode, size, TYPE_FILE)))?;
            v.commit(root)
        })
    }

    /// Removes a file. Nothing is erased: the entry is simply missing
    /// from the new directory, so its blocks become unreachable - and
    /// therefore free - the moment the commit lands.
    pub fn delete(&mut self, name: &str) -> Result<()> {
        let name = trim_slashes(name);
        self.change(|v| {
            let root = v.rewrite_directory(name, None)?;
            v.commit(root)
        })
    }
}

// ---- tests ----------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::MemDevice;

    const TOTAL: u32 = 512;

    fn fresh() -> Volume<MemDevice> {
        let mut device = MemDevice::new(TOTAL);
        format(&mut device, TOTAL).unwrap();
        Volume::mount(device).unwrap()
    }

    /// Unmount and mount again, so a test sees only what reached "disk".
    fn remount(volume: Volume<MemDevice>) -> Volume<MemDevice> {
        Volume::mount(volume.into_device()).unwrap()
    }

    /// Bytes that differ at every position, so a misplaced block shows.
    fn pattern(len: usize, seed: u8) -> Vec<u8> {
        (0..len)
            .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed))
            .collect()
    }

    fn contents(volume: &mut Volume<MemDevice>, name: &str) -> Vec<u8> {
        let file = volume.lookup(name).unwrap();
        volume.read_all(&file).unwrap()
    }

    #[test]
    fn a_fresh_volume_is_empty() {
        let mut v = fresh();
        assert!(v.list().unwrap().is_empty());
        assert_eq!(v.superblock().generation, 1);
        // Two superblocks and the root directory are in use.
        assert_eq!(v.free_blocks(), TOTAL - 3);
    }

    #[test]
    fn format_rejects_impossible_sizes() {
        let mut device = MemDevice::new(8);
        assert!(matches!(format(&mut device, 7), Err(Error::BadSize)));
        assert!(matches!(
            format(&mut device, MAX_BLOCKS + 1),
            Err(Error::BadSize)
        ));
    }

    #[test]
    fn put_and_read_back_survive_a_remount() {
        let mut v = fresh();
        let data = pattern(3 * BLOCK_SIZE + 100, 7);
        v.put("big.bin", &data).unwrap();
        v.put("small.txt", b"hello").unwrap();

        let mut v = remount(v);
        assert_eq!(contents(&mut v, "big.bin"), data);
        assert_eq!(contents(&mut v, "/small.txt"), b"hello");
        assert_eq!(v.list().unwrap().len(), 2);
    }

    #[test]
    fn put_replaces_the_whole_file() {
        let mut v = fresh();
        v.put("f", &pattern(10_000, 1)).unwrap();
        v.put("f", b"short").unwrap();

        assert_eq!(contents(&mut v, "f"), b"short");
        assert_eq!(v.list().unwrap().len(), 1);
    }

    #[test]
    fn write_at_changes_only_the_bytes_it_covers() {
        let mut v = fresh();
        let mut expected = pattern(2 * BLOCK_SIZE, 3);
        v.put("f", &expected).unwrap();

        // Straddles the boundary between block 0 and block 1.
        v.write_at("f", BLOCK_SIZE as u32 - 2, b"XYZW").unwrap();
        expected[BLOCK_SIZE - 2..BLOCK_SIZE + 2].copy_from_slice(b"XYZW");

        assert_eq!(contents(&mut v, "f"), expected);
    }

    #[test]
    fn writing_past_the_end_fills_the_gap_with_zeros() {
        let mut v = fresh();
        v.create("f").unwrap();
        v.write_at("f", 3 * BLOCK_SIZE as u32, b"end").unwrap();

        let data = contents(&mut v, "f");
        assert_eq!(data.len(), 3 * BLOCK_SIZE + 3);
        assert!(data[..3 * BLOCK_SIZE].iter().all(|&b| b == 0));
        assert_eq!(&data[3 * BLOCK_SIZE..], b"end");
    }

    #[test]
    fn a_leading_slash_names_the_same_file() {
        let mut v = fresh();
        v.create("a").unwrap();
        v.write_at("/a", 0, b"one").unwrap();

        assert_eq!(v.list().unwrap().len(), 1);
        assert_eq!(contents(&mut v, "a"), b"one");
    }

    #[test]
    fn create_refuses_duplicates_and_bad_names() {
        let mut v = fresh();
        v.create("a").unwrap();
        assert!(matches!(v.create("a"), Err(Error::Exists)));
        assert!(matches!(v.create(""), Err(Error::Name)));
        assert!(matches!(v.create("/"), Err(Error::Name)));
        assert!(matches!(v.create(&"n".repeat(NAME_MAX)), Err(Error::Name)));
        v.create(&"n".repeat(NAME_MAX - 1)).unwrap();
    }

    #[test]
    fn delete_returns_every_block() {
        let mut v = fresh();
        let free_before = v.free_blocks();
        v.put("f", &pattern(5 * BLOCK_SIZE, 9)).unwrap();
        assert!(v.free_blocks() < free_before);

        v.delete("f").unwrap();
        assert_eq!(v.free_blocks(), free_before);
        assert!(matches!(v.lookup("f"), Err(Error::NotFound)));
        assert!(matches!(v.delete("f"), Err(Error::NotFound)));
    }

    #[test]
    fn a_directory_can_span_several_blocks() {
        let mut v = fresh();
        let count = DIR_ENTRIES * 2 + 5; // three directory blocks
        for i in 0..count {
            v.create(&format!("file{i}")).unwrap();
        }

        let mut v = remount(v);
        assert_eq!(v.list().unwrap().len(), count);
        assert!(v.lookup(&format!("file{}", count - 1)).is_ok());
    }

    #[test]
    fn running_out_of_space_changes_nothing() {
        let mut device = MemDevice::new(16);
        format(&mut device, 16).unwrap();
        let mut v = Volume::mount(device).unwrap();
        v.put("keep", b"safe").unwrap();
        let free_before = v.free_blocks();

        assert!(matches!(
            v.put("huge", &pattern(64 * BLOCK_SIZE, 0)),
            Err(Error::Full)
        ));

        // The failed attempt's blocks went back to the free map...
        assert_eq!(v.free_blocks(), free_before);
        // ...and the volume on disk is exactly as it was.
        let mut v = remount(v);
        assert_eq!(v.list().unwrap().len(), 1);
        assert_eq!(contents(&mut v, "keep"), b"safe");
    }

    /// The property copy-on-write exists for.
    ///
    /// Cut the power after 0 writes, then after 1, then 2, and so on,
    /// until the change completes. After every cut, mount the disk
    /// fresh. It must show either the complete old state or the complete
    /// new state - never anything in between, never a damaged volume.
    #[test]
    fn a_power_cut_at_any_write_leaves_old_or_new_state() {
        let mut base = MemDevice::new(TOTAL);
        format(&mut base, TOTAL).unwrap();
        let mut v = Volume::mount(base).unwrap();
        v.put("old", b"before").unwrap();
        let base = v.into_device();

        let new_data = pattern(2 * BLOCK_SIZE + 10, 5);
        let mut cuts = 0;

        for writes_allowed in 0.. {
            let device = MemDevice {
                blocks: base.blocks.clone(),
                writes_left: Some(writes_allowed),
            };
            let mut v = Volume::mount(device).unwrap();

            // Overwrite "old" and add "new" - two separate commits.
            let first = v.put("old", b"after");
            let second = first.is_ok().then(|| v.put("new", &new_data));
            let finished = matches!(second, Some(Ok(())));

            let mut device = v.into_device();
            device.writes_left = None; // power is back
            let mut v = Volume::mount(device).expect("volume must always mount");

            let old = contents(&mut v, "old");
            let has_new = v.lookup("new").is_ok();

            if first.is_err() {
                // Cut during the first change: nothing of it is visible.
                assert_eq!(old, b"before", "after {writes_allowed} writes");
                assert!(!has_new);
            } else {
                // The first change committed, completely.
                assert_eq!(old, b"after", "after {writes_allowed} writes");
                assert_eq!(has_new, finished, "after {writes_allowed} writes");
                if has_new {
                    assert_eq!(contents(&mut v, "new"), new_data);
                }
            }

            if finished {
                break;
            }
            cuts += 1;
        }

        // Make sure the loop really exercised failures, not just success.
        assert!(cuts > 5, "only {cuts} cut points were tried");
    }
}
