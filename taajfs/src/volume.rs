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
    empty_block, get_u32, put_u32, Block, DirBlock, DirEntry, Inode, Superblock, BLOCK_SIZE, DIR_ENTRIES,
    FIRST_DATA, INDEX_BLOCKS, INODE_BLOCKS, MAGIC, MAX_BLOCKS, MAX_FILE_SIZE, NAME_MAX, SUPER_A, SUPER_B,
    TYPE_DIR, TYPE_FILE, VERSION_BIG_FILES, VERSION_DIRECTORIES, VERSION_STAMPS, STAMPED_BLOCKS,
};

/// The most blocks `read_at` asks the device for at once: 256 KiB.
const READ_RUN_MAX: usize = 64;

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
    /// A name on the way to a file is not a directory.
    NotDirectory,
    /// A directory, where only a file will do.
    IsDirectory,
    /// A directory with something in it cannot be removed.
    NotEmpty,
    /// A directory cannot be moved to somewhere inside itself.
    IntoItself,
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
            Error::Name => write!(f, "each name in a path must be 1 to {} bytes, and not . or ..; at most {MAX_DEPTH} deep", NAME_MAX - 1),
            Error::TooBig => f.write_str("file would exceed the maximum size"),
            Error::BadSize => write!(f, "a volume must be 8 to {MAX_BLOCKS} blocks"),
            Error::NotDirectory => f.write_str("not a directory"),
            Error::IsDirectory => f.write_str("is a directory"),
            Error::NotEmpty => f.write_str("directory is not empty"),
            Error::IntoItself => f.write_str("a directory cannot be moved into itself"),
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

/// How many directories deep a path may go. A limit, so that walking
/// the tree - which a damaged or hostile disk could make endless - has
/// an end that does not depend on the disk.
pub const MAX_DEPTH: usize = 16;

/// The names in a path, from the root down: "docs/notes" is docs, then
/// notes. Slashes at either end, and doubled ones, mean nothing - "/a",
/// "a/" and "a" are the same - and no names at all is the root itself.
///
/// Each name must fit the 28-byte field with its terminating NUL (the
/// rule `TfsCreate` enforces). "." and ".." are refused: nothing on the
/// disk has those names, and a path that climbs is the caller's to
/// resolve before it gets here.
///
/// The result borrows from `path`: no copies, only views into it.
fn components(path: &str) -> Result<Vec<&str>> {
    let names: Vec<&str> = path.split('/').filter(|name| !name.is_empty()).collect();
    let bad = |name: &&str| name.len() >= NAME_MAX || name.contains('\0') || *name == "." || *name == "..";
    if names.len() > MAX_DEPTH + 1 || names.iter().any(bad) {
        return Err(Error::Name);
    }
    Ok(names)
}

/// A path that must name something in a directory: the directories on
/// the way, and the last name.
fn split_leaf(path: &str) -> Result<(Vec<&str>, &str)> {
    let mut names = components(path)?;
    let leaf = names.pop().ok_or(Error::Name)?;
    Ok((names, leaf))
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
    /// The lowest version either valid superblock slot says: what an
    /// older implementation could still mount the volume as (see
    /// `raise_version`).
    lowest: u32,
    /// Which blocks the committed tree uses. Derived, never stored.
    used: Vec<bool>,
    used_count: u32,
    /// Blocks handed out since the last commit: the ones the new tree
    /// does not use are free again once it is committed.
    fresh: Vec<u32>,
}

impl<D: BlockDevice> Volume<D> {
    /// Picks the newest valid superblock and rebuilds the free map.
    ///
    /// Takes the device by value: the volume owns it from now on, so
    /// nothing else can write to the disk behind the volume's back.
    pub fn mount(mut device: D) -> Result<Volume<D>> {
        let mut best: Option<(u32, Superblock)> = None;
        let mut lowest = VERSION_STAMPS;
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
            lowest = lowest.min(sb.version);
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
            lowest,
            used: Vec::new(),
            used_count: 0,
            fresh: Vec::new(),
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
        if !inode.is_valid() {
            return Err(Error::Magic);
        }
        Ok(inode)
    }

    /// Every block of the directory that starts at block `first`, in
    /// chain order.
    ///
    /// C guards against a looping chain with a counter. A set of the
    /// blocks already visited catches a loop on its first repeat, and
    /// says exactly where.
    fn directory_chain(&mut self, first: u32) -> Result<Vec<(u32, DirBlock)>> {
        let mut chain = Vec::new();
        let mut seen = BTreeSet::new();
        let mut next = first;

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

    /// Every live entry of the directory that starts at block `first`
    /// (unused slots skipped).
    fn entries(&mut self, first: u32) -> Result<Vec<DirEntry>> {
        Ok(self
            .directory_chain(first)?
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
        self.fresh.clear();
        self.mark_used(SUPER_A)?;
        self.mark_used(SUPER_B)?;

        // `directory_chain` returns an owned Vec, so the loop below owns
        // the blocks and is free to call `&mut self` methods while
        // iterating. Iterating something borrowed from `self` here would
        // not compile - the borrow checker forbids changing a thing
        // while you are reading through it.
        //
        // The tree is walked with a list of directories still to visit,
        // not by a function calling itself: how deep it goes is up to
        // what is on the disk, and a disk must not be able to exhaust
        // the stack. Two more things a damaged disk could do are
        // refused: a tree deeper than MAX_DEPTH, and a directory block
        // reached twice - which, since a directory's blocks belong to it
        // alone, means a loop or two names for one directory.
        let mut to_visit = vec![(self.sb.root_dir, 0usize)];
        while let Some((first, depth)) = to_visit.pop() {
            if depth > MAX_DEPTH {
                return Err(Error::Corrupt(format!("directories nest deeper than {MAX_DEPTH}")));
            }
            for (number, dir) in self.directory_chain(first)? {
                if self.used.get(number as usize).copied().unwrap_or(false) {
                    return Err(Error::Corrupt(format!("directory block {number} is reached twice")));
                }
                self.mark_used(number)?;
                for entry in dir.entries.iter().filter(|e| e.inode != 0) {
                    if entry.kind == TYPE_DIR {
                        to_visit.push((entry.inode, depth + 1));
                        continue;
                    }
                    self.mark_used(entry.inode)?;
                    let inode = self.read_inode(entry.inode)?;
                    if inode.is_big() {
                        for &index in &inode.blocks {
                            self.mark_used(index)?;
                        }
                    }
                    for block in self.file_blocks(&inode)?.into_iter().filter(|&b| b != 0) {
                        self.mark_used(block)?;
                    }
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
        self.fresh.push(index as u32);
        Ok(index as u32)
    }

    fn mark_free(&mut self, number: u32) {
        if let Some(slot) = self.used.get_mut(number as usize) {
            if *slot {
                *slot = false;
                self.used_count -= 1;
            }
        }
    }

    /// Brings the free map from the tree at `old_root` to the one at
    /// `new_root`, just committed - what `rebuild_free_map` would make,
    /// without walking the whole tree.
    ///
    /// Rebuilding after every commit read every directory, inode and
    /// index block on the volume, for each write: on a disk with a few
    /// hundred files, a program writing a file in 1 KB pieces spent
    /// nearly all its time there (journal entry 080). But a block the
    /// committed tree reaches is never written, so a block number both
    /// trees reach has the same contents in both, and everything below
    /// it is the same too. Only what differs needs looking at: the
    /// directories whose first block changed, down to the entries that
    /// did. The tests check the result against a rebuild at every
    /// commit.
    fn update_free_map(&mut self, old_root: u32, new_root: u32) -> Result<()> {
        let (gone, came) = self.changed_blocks(old_root, new_root)?;
        let fresh = core::mem::take(&mut self.fresh);
        for number in gone.into_iter().chain(fresh) {
            self.mark_free(number);
        }
        for number in came {
            self.mark_used(number)?;
        }
        #[cfg(test)]
        {
            let (used, count) = (self.used.clone(), self.used_count);
            self.rebuild_free_map()?;
            assert!(used == self.used && count == self.used_count, "the free map kept up to date is not the rebuilt one");
        }
        Ok(())
    }

    /// The blocks only the old tree reaches, and those only the new one
    /// does, of two trees that share every block they both reach.
    fn changed_blocks(&mut self, old_root: u32, new_root: u32) -> Result<(Vec<u32>, Vec<u32>)> {
        let mut gone = Vec::new();
        let mut came = Vec::new();
        let mut pairs = vec![(old_root, new_root, 0usize)];
        while let Some((old, new, depth)) = pairs.pop() {
            if old == new {
                continue;
            }
            if depth > MAX_DEPTH {
                return Err(Error::Corrupt(format!("directories nest deeper than {MAX_DEPTH}")));
            }
            let mut before = alloc::collections::BTreeMap::new();
            for (number, dir) in self.directory_chain(old)? {
                gone.push(number);
                for entry in dir.entries.into_iter().filter(|e| e.inode != 0) {
                    before.insert(entry.name, entry);
                }
            }
            let mut after = Vec::new();
            for (number, dir) in self.directory_chain(new)? {
                came.push(number);
                after.extend(dir.entries.into_iter().filter(|e| e.inode != 0));
            }
            for entry in after {
                match before.remove(&entry.name) {
                    Some(old) if old.inode == entry.inode && old.kind == entry.kind => {}
                    Some(old) if old.kind == TYPE_DIR && entry.kind == TYPE_DIR => {
                        pairs.push((old.inode, entry.inode, depth + 1))
                    }
                    Some(old) => {
                        self.subtree_blocks(&old, depth, &mut gone)?;
                        self.subtree_blocks(&entry, depth, &mut came)?;
                    }
                    None => self.subtree_blocks(&entry, depth, &mut came)?,
                }
            }
            for old in before.into_values() {
                self.subtree_blocks(&old, depth, &mut gone)?;
            }
        }
        Ok((gone, came))
    }

    /// Every block `entry` reaches - a file's inode, index and data
    /// blocks; a directory's own, and all below it - onto `out`.
    fn subtree_blocks(&mut self, entry: &DirEntry, depth: usize, out: &mut Vec<u32>) -> Result<()> {
        let mut to_visit = vec![(*entry, depth)];
        while let Some((entry, depth)) = to_visit.pop() {
            if entry.kind == TYPE_DIR {
                if depth > MAX_DEPTH {
                    return Err(Error::Corrupt(format!("directories nest deeper than {MAX_DEPTH}")));
                }
                for (number, dir) in self.directory_chain(entry.inode)? {
                    out.push(number);
                    to_visit.extend(dir.entries.into_iter().filter(|e| e.inode != 0).map(|e| (e, depth + 1)));
                }
                continue;
            }
            out.push(entry.inode);
            let inode = self.read_inode(entry.inode)?;
            if inode.is_big() {
                out.extend_from_slice(&inode.blocks);
            }
            out.extend(self.file_blocks(&inode)?.into_iter().filter(|&b| b != 0));
        }
        Ok(())
    }

    // ---- committing -------------------------------------------------

    /// Publishes a new root with one block write, into the slot that is
    /// NOT live. Until it lands the volume reads as before; after it
    /// lands every change is visible at once.
    fn commit(&mut self, new_root: u32) -> Result<()> {
        self.commit_as(new_root, self.sb.version)
    }

    /// The same, saying which version of the format the volume now is.
    fn commit_as(&mut self, new_root: u32, version: u32) -> Result<()> {
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
            version,
            ..self.sb
        }
        .sealed();
        self.write(target, &sb.to_block())?;

        let old_root = self.sb.root_dir;
        self.sb = sb;
        self.slot = target;

        // The old tree is unreachable now: what only it reached is free.
        // A tree that cannot be compared is rebuilt from, which also
        // says what is wrong with it.
        if self.update_free_map(old_root, new_root).is_err() {
            return self.rebuild_free_map();
        }
        Ok(())
    }

    /// Makes the volume one that may hold directories: version 2 in
    /// both superblock slots, before the first directory exists.
    fn allow_directories(&mut self) -> Result<()> {
        self.raise_version(VERSION_DIRECTORIES)
    }

    /// Makes the volume one that may hold files bigger than an inode's
    /// own 1020 blocks: version 3 in both slots, before the first one.
    fn allow_big_files(&mut self) -> Result<()> {
        self.raise_version(VERSION_BIG_FILES)
    }

    /// Makes the volume one whose files carry change stamps: version 4
    /// in both slots. From then on every inode written is stamped (see
    /// `Inode`). A volume below it is left as older code can read it -
    /// the C tool reads only version 1 - so this is asked for, not done
    /// by every write: TheTaaJ's file server asks when it mounts a disk.
    pub fn allow_stamps(&mut self) -> Result<()> {
        self.raise_version(VERSION_STAMPS)
    }

    /// Whether inodes written now are stamped: only when no slot says
    /// less than version 4, as with every other version (see
    /// `raise_version`).
    fn stamping(&self) -> bool {
        self.lowest >= VERSION_STAMPS
    }

    /// How many blocks an inode written now names itself.
    fn direct_slots(&self) -> usize {
        if self.stamping() {
            STAMPED_BLOCKS
        } else {
            INODE_BLOCKS
        }
    }

    /// The stamp the next commit gives what it changes: its generation.
    fn next_stamp(&self) -> u64 {
        u64::from(self.sb.generation) + 1
    }

    /// Makes both superblock slots say at least `version`, before the
    /// first thing that version allows is written.
    ///
    /// Why both, and why first. An implementation that knows only
    /// version 1 - the C one - treats every entry as a file. Shown a
    /// directory it would read the directory's block as an inode, take
    /// whatever numbers are in it for the file's blocks, and build its
    /// free map from that: its next write could land on live data. One
    /// that knows only version 2 would do the same with a big file's
    /// index blocks. What stops either is the version number: each
    /// mounts only slots that say a version it knows. But a commit
    /// writes ONE slot. After a single new commit the other slot still
    /// says the old version, still verifies, and to the old code is the
    /// volume - an older tree, whose "free" blocks the new tree is using.
    ///
    /// So the tree as it is now - nothing new in it yet - is committed
    /// twice, unchanged, as the new version: once into each slot. A
    /// power cut between the two leaves one slot of each version, both
    /// pointing at the same tree, which either implementation may safely
    /// use. Only when no slot says less is the new kind of thing made.
    fn raise_version(&mut self, version: u32) -> Result<()> {
        if self.lowest >= version {
            return Ok(());
        }
        let version = version.max(self.sb.version);
        self.commit_as(self.sb.root_dir, version)?;
        self.commit_as(self.sb.root_dir, version)?;
        self.lowest = version;
        Ok(())
    }

    // ---- a file's blocks --------------------------------------------

    /// Every data block of a file, in order: the inode's own list for a
    /// small file, or what its index blocks hold for a big one.
    fn file_blocks(&mut self, inode: &Inode) -> Result<Vec<u32>> {
        if !inode.is_big() {
            return Ok(inode.blocks.clone());
        }
        let count = inode.block_count as usize;
        if count > inode.slots() * INDEX_BLOCKS || inode.blocks.len() != count.div_ceil(INDEX_BLOCKS) {
            return Err(Error::Corrupt(format!("a file says it has {count} blocks")));
        }
        let mut blocks = Vec::with_capacity(count);
        for &index in &inode.blocks {
            let block = self.read_index(index)?;
            let here = (count - blocks.len()).min(INDEX_BLOCKS);
            blocks.extend((0..here).map(|i| get_u32(&block, i * 4)));
        }
        Ok(blocks)
    }

    /// An index block, checked to be inside the volume.
    fn read_index(&mut self, number: u32) -> Result<Block> {
        if number < FIRST_DATA || number >= self.sb.total_blocks {
            return Err(Error::Corrupt(format!("index block {number} is outside the volume")));
        }
        self.read(number)
    }

    /// Writes a new inode for a file of `size` bytes in `blocks`: the
    /// blocks named in it, or - more than it holds - index blocks, new
    /// ones, written first. Returns the inode's block. A big file needs
    /// a version-3 volume: the caller made sure (`allow_big_files`). On a
    /// version-4 volume the inode is stamped with the coming commit's
    /// generation, and names two blocks fewer itself.
    fn write_inode(&mut self, size: u32, kind: u32, blocks: Vec<u32>) -> Result<u32> {
        let inode = if blocks.len() <= self.direct_slots() {
            Inode::new(size, kind, blocks)
        } else {
            debug_assert!(self.sb.version >= VERSION_BIG_FILES);
            let mut index = Vec::with_capacity(blocks.len().div_ceil(INDEX_BLOCKS));
            for chunk in blocks.chunks(INDEX_BLOCKS) {
                let mut block = empty_block();
                for (i, &number) in chunk.iter().enumerate() {
                    put_u32(&mut block, i * 4, number);
                }
                let number = self.allocate()?;
                self.write(number, &block)?;
                index.push(number);
            }
            Inode::big(size, kind, blocks.len() as u32, index)
        };
        let inode = if self.stamping() { inode.stamped(self.next_stamp()) } else { inode };
        let number = self.allocate()?;
        self.write(number, &inode.to_block())?;
        Ok(number)
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

    /// The first block of each directory on the way down `dirs`: the
    /// root's, then one per name. Fails if a name is missing, or is a
    /// file.
    fn descend(&mut self, dirs: &[&str]) -> Result<Vec<u32>> {
        self.descend_from(self.sb.root_dir, dirs)
    }

    /// The same, down a tree whose root directory starts at `root` -
    /// which need not be the committed one: a change made of two steps
    /// walks, for its second, the tree its first step wrote.
    fn descend_from(&mut self, root: u32, dirs: &[&str]) -> Result<Vec<u32>> {
        let mut firsts = vec![root];
        for name in dirs {
            let here = firsts[firsts.len() - 1];
            let entry = self.entries(here)?.into_iter().find(|e| e.is_named(name)).ok_or(Error::NotFound)?;
            if entry.kind != TYPE_DIR {
                return Err(Error::NotDirectory);
            }
            firsts.push(entry.inode);
        }
        Ok(firsts)
    }

    /// The entry called `leaf` in the directory `dirs` leads to, if
    /// there is one.
    fn find(&mut self, dirs: &[&str], leaf: &str) -> Result<Option<DirEntry>> {
        let firsts = self.descend(dirs)?;
        Ok(self.entries(firsts[dirs.len()])?.into_iter().find(|e| e.is_named(leaf)))
    }

    /// Changes one entry of the directory `dirs` leads to - and, because
    /// nothing committed is ever written over, every directory above it.
    ///
    /// The directory gets a new copy with the entry changed. That copy
    /// starts at a new block, so the entry that names this directory in
    /// its parent must change too - which is a change to the parent, and
    /// so on up. What comes back is the first block of a new root: one
    /// new path from the top of the tree down to the change, with
    /// everything off that path shared, untouched, with the old tree.
    /// Committing that root publishes the whole change at once.
    fn rewrite_path(&mut self, dirs: &[&str], leaf: &str, replacement: Option<DirEntry>) -> Result<u32> {
        self.rewrite_path_from(self.sb.root_dir, dirs, leaf, replacement)
    }

    /// The same, starting from the tree whose root directory starts at
    /// `root`. Nothing of that tree is written over either, committed
    /// or not: the result is one more new root.
    fn rewrite_path_from(&mut self, root: u32, dirs: &[&str], leaf: &str, replacement: Option<DirEntry>) -> Result<u32> {
        let firsts = self.descend_from(root, dirs)?;
        let mut new = self.rewrite_directory(firsts[dirs.len()], leaf, replacement)?;
        for depth in (0..dirs.len()).rev() {
            let entry = DirEntry::new(dirs[depth], new, 0, TYPE_DIR);
            new = self.rewrite_directory(firsts[depth], dirs[depth], Some(entry))?;
        }
        Ok(new)
    }

    /// Writes a complete new copy of the directory that starts at
    /// `first` with one entry replaced, added (`replacement` named
    /// something new) or removed (`replacement` None), and returns the
    /// copy's first block.
    fn rewrite_directory(&mut self, first: u32, name: &str, replacement: Option<DirEntry>) -> Result<u32> {
        let mut entries = self.entries(first)?;
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

    /// What is in the root directory.
    pub fn list(&mut self) -> Result<Vec<FileInfo>> {
        self.list_in("")
    }

    /// What is in the directory `path` names ("" is the root).
    pub fn list_in(&mut self, path: &str) -> Result<Vec<FileInfo>> {
        let dirs = components(path)?;
        let firsts = self.descend(&dirs)?;
        Ok(self.entries(firsts[dirs.len()])?.iter().map(FileInfo::from).collect())
    }

    /// The file or directory `path` names. The root itself is a
    /// directory with no name.
    pub fn lookup(&mut self, path: &str) -> Result<FileInfo> {
        let mut dirs = components(path)?;
        let Some(leaf) = dirs.pop() else {
            return Ok(FileInfo { name: String::new(), inode: self.sb.root_dir, size: 0, kind: TYPE_DIR });
        };
        self.find(&dirs, leaf)?.as_ref().map(FileInfo::from).ok_or(Error::NotFound)
    }

    /// When the file last changed: the generation of the commit that
    /// did it, or 0 for a directory and for a file not changed since
    /// the volume began stamping (see `Inode`).
    pub fn stamp(&mut self, file: &FileInfo) -> Result<u64> {
        if file.kind == TYPE_DIR {
            return Ok(0);
        }
        Ok(self.read_inode(file.inode)?.stamp)
    }

    /// Reads from `offset` into `buffer`; returns how many bytes it got.
    pub fn read_at(&mut self, file: &FileInfo, offset: u32, buffer: &mut [u8]) -> Result<usize> {
        if file.kind == TYPE_DIR {
            return Err(Error::IsDirectory);
        }
        if offset >= file.size {
            return Ok(0);
        }
        let length = buffer.len().min((file.size - offset) as usize);
        let inode = self.read_inode(file.inode)?;
        // A big file's index block, read once for every block it names.
        let mut index: Option<(usize, Block)> = None;
        let mut done = 0;
        // Which block each piece of the buffer comes from, gathered
        // first: then a run of neighbouring blocks - a file written in
        // one go is mostly one - is read from the device in one go.
        let mut pieces: Vec<(u32, usize, usize)> = Vec::new();

        while done < length {
            let at = offset as usize + done;
            let block_index = at / BLOCK_SIZE;
            let within = at % BLOCK_SIZE;
            let chunk = (BLOCK_SIZE - within).min(length - done);

            // A file shorter than its size claims ends early rather
            // than reading garbage.
            let number = if inode.is_big() {
                if block_index >= inode.block_count as usize {
                    break;
                }
                let slot = block_index / INDEX_BLOCKS;
                if index.as_ref().is_none_or(|(at, _)| *at != slot) {
                    let Some(&number) = inode.blocks.get(slot) else { break };
                    index = Some((slot, self.read_index(number)?));
                }
                let (_, block) = index.as_ref().expect("just read");
                get_u32(block, (block_index % INDEX_BLOCKS) * 4)
            } else {
                // `let ... else`: take the block number if there is one,
                // otherwise leave the loop.
                let Some(&number) = inode.blocks.get(block_index) else {
                    break;
                };
                number
            };

            pieces.push((number, within, chunk));
            done += chunk;
        }

        let mut at = 0;
        let mut run = Vec::new();
        let mut piece = 0;
        while piece < pieces.len() {
            let first = pieces[piece].0;
            let mut end = piece + 1;
            while end < pieces.len() && end - piece < READ_RUN_MAX && pieces[end].0 == first + (end - piece) as u32 {
                end += 1;
            }
            run.resize((end - piece) * BLOCK_SIZE, 0);
            self.device.read_blocks(first, &mut run)?;
            for (n, &(_, within, chunk)) in pieces[piece..end].iter().enumerate() {
                let from = n * BLOCK_SIZE + within;
                buffer[at..at + chunk].copy_from_slice(&run[from..from + chunk]);
                at += chunk;
            }
            piece = end;
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
    pub fn create(&mut self, path: &str) -> Result<()> {
        let (dirs, name) = split_leaf(path)?;
        if self.find(&dirs, name)?.is_some() {
            return Err(Error::Exists);
        }

        self.change(|v| {
            let inode = v.write_inode(0, TYPE_FILE, Vec::new())?;
            let root = v.rewrite_path(&dirs, name, Some(DirEntry::new(name, inode, 0, TYPE_FILE)))?;
            v.commit(root)
        })
    }

    /// Marks a file changed now, as `touch` does, without changing what
    /// is in it: a new inode, with the same blocks and a new stamp, in
    /// one commit. A file of an inode from before stamping, too big for
    /// a stamped inode's own list, gets index blocks.
    pub fn touch(&mut self, path: &str) -> Result<()> {
        let (dirs, name) = split_leaf(path)?;
        let file = self.find(&dirs, name)?.as_ref().map(FileInfo::from).ok_or(Error::NotFound)?;
        if file.kind == TYPE_DIR {
            return Err(Error::IsDirectory);
        }
        let inode = self.read_inode(file.inode)?;
        let blocks = self.file_blocks(&inode)?;
        if blocks.len() > self.direct_slots() {
            self.allow_big_files()?;
        }
        self.change(|v| {
            let new_inode = v.write_inode(file.size, inode.kind, blocks)?;
            let entry = DirEntry::new(&file.name, new_inode, file.size, TYPE_FILE);
            let root = v.rewrite_path(&dirs, &file.name, Some(entry))?;
            v.commit(root)
        })
    }

    /// An empty directory. Its parent must exist: one call makes one
    /// directory.
    pub fn make_directory(&mut self, path: &str) -> Result<()> {
        let (dirs, name) = split_leaf(path)?;
        if dirs.len() >= MAX_DEPTH {
            return Err(Error::Name); // it would be deeper than a path can reach
        }
        if self.find(&dirs, name)?.is_some() {
            return Err(Error::Exists);
        }

        self.allow_directories()?;
        self.change(|v| {
            // An empty directory is one block with no entries - as the
            // root of a fresh volume is.
            let first = v.allocate()?;
            v.write(first, &DirBlock::new(Vec::new(), 0).to_block())?;
            let root = v.rewrite_path(&dirs, name, Some(DirEntry::new(name, first, 0, TYPE_DIR)))?;
            v.commit(root)
        })
    }

    /// Writes `data` at `offset` into an existing file, growing it if
    /// needed. The same as `TfsWriteFile`, block for block.
    pub fn write_at(&mut self, path: &str, offset: u32, data: &[u8]) -> Result<usize> {
        let end = offset as usize + data.len();
        if end > MAX_FILE_SIZE {
            return Err(Error::TooBig);
        }
        let (dirs, name) = split_leaf(path)?;
        let file = self.find(&dirs, name)?.as_ref().map(FileInfo::from).ok_or(Error::NotFound)?;
        if file.kind == TYPE_DIR {
            return Err(Error::IsDirectory);
        }
        if end.div_ceil(BLOCK_SIZE) > self.direct_slots() {
            self.allow_big_files()?;
        }

        self.change(|v| {
            let inode = v.read_inode(file.inode)?;
            let mut blocks = v.file_blocks(&inode)?;
            let new_size = file.size.max(end as u32);
            let mut done = 0;

            while done < data.len() {
                let at = offset as usize + done;
                let index = at / BLOCK_SIZE;
                let within = at % BLOCK_SIZE;
                let chunk = (BLOCK_SIZE - within).min(data.len() - done);

                // Start from the old contents, so the bytes of the block
                // this write does not touch survive...
                let mut block = match blocks.get(index) {
                    Some(&old) if old != 0 => v.read(old)?,
                    _ => empty_block(),
                };
                block[within..within + chunk].copy_from_slice(&data[done..done + chunk]);

                // ...and write the result somewhere NEW. The old block is
                // untouched: the committed tree still points at it.
                let new = v.allocate()?;
                v.write(new, &block)?;

                if index < blocks.len() {
                    blocks[index] = new;
                } else {
                    // A write past the end can leave a gap. Fill it with
                    // real zeroed blocks, never holes: a hole read as a
                    // stale number would return another file's data.
                    let gap_start = blocks.len();
                    blocks.resize(index + 1, 0);
                    blocks[index] = new;
                    // `&mut` over a range of the Vec: each `slot` is a
                    // mutable reference to one entry, written in place.
                    for slot in &mut blocks[gap_start..index] {
                        let fill = v.allocate()?;
                        v.write(fill, &empty_block())?;
                        *slot = fill;
                    }
                }
                done += chunk;
            }

            let new_inode = v.write_inode(new_size, inode.kind, blocks)?;

            // `file.name` came from the disk, so it is already in its
            // stored form. (C passes the caller's name here, and a
            // leading '/' would make it add a second entry instead of
            // replacing the first.)
            let entry = DirEntry::new(&file.name, new_inode, new_size, TYPE_FILE);
            let root = v.rewrite_path(&dirs, &file.name, Some(entry))?;
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
    pub fn put(&mut self, path: &str, data: &[u8]) -> Result<()> {
        let (dirs, name) = split_leaf(path)?;
        if data.len() > MAX_FILE_SIZE {
            return Err(Error::TooBig);
        }
        // A file may replace a file. Replacing a directory would drop
        // everything in it in passing: that takes `delete`, and an empty
        // directory.
        if self.find(&dirs, name)?.is_some_and(|entry| entry.kind == TYPE_DIR) {
            return Err(Error::IsDirectory);
        }
        if data.len().div_ceil(BLOCK_SIZE) > self.direct_slots() {
            self.allow_big_files()?;
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
            let inode = v.write_inode(size, TYPE_FILE, blocks)?;
            let root =
                v.rewrite_path(&dirs, name, Some(DirEntry::new(name, inode, size, TYPE_FILE)))?;
            v.commit(root)
        })
    }

    /// Removes a file, or a directory with nothing in it. Nothing is
    /// erased: the entry is simply missing from the new directory, so
    /// its blocks become unreachable - and therefore free - the moment
    /// the commit lands.
    ///
    /// A directory must be empty first. Removing a whole tree in one
    /// call would be easy here - drop one entry and everything under it
    /// is unreachable - which is exactly why it is not offered: one
    /// mistyped name should not cost more than one thing.
    pub fn delete(&mut self, path: &str) -> Result<()> {
        let (dirs, name) = split_leaf(path)?;
        let entry = self.find(&dirs, name)?.ok_or(Error::NotFound)?;
        if entry.kind == TYPE_DIR && !self.entries(entry.inode)?.is_empty() {
            return Err(Error::NotEmpty);
        }
        self.change(|v| {
            let root = v.rewrite_path(&dirs, name, None)?;
            v.commit(root)
        })
    }

    /// How many directories deep the tree under the directory starting
    /// at `first` goes: 0 if it holds no directories, 1 if it holds
    /// some that hold none, and so on. Walked with a list, not by a
    /// function calling itself, and with the same limit as the mount.
    fn height(&mut self, first: u32) -> Result<usize> {
        let mut highest = 0;
        let mut to_visit = vec![(first, 0usize)];
        while let Some((first, depth)) = to_visit.pop() {
            if depth > MAX_DEPTH {
                return Err(Error::Corrupt(format!("directories nest deeper than {MAX_DEPTH}")));
            }
            highest = highest.max(depth);
            for entry in self.entries(first)? {
                if entry.kind == TYPE_DIR {
                    to_visit.push((entry.inode, depth + 1));
                }
            }
        }
        Ok(highest)
    }

    /// Gives what `from` names the name `to` instead: a new name in the
    /// same directory, or a place in another one. A file or a whole
    /// directory moves the same way, and nothing in it is read or
    /// copied - an entry is a name and a block number, and only the
    /// name's place changes.
    ///
    /// It is one commit. Taking the entry out of one directory and
    /// putting it into another are two changes to the tree, and between
    /// them the thing would be in neither place (or in both): so the
    /// second change is made to the tree the first one wrote, which
    /// nobody can see yet, and only the root that has both is
    /// committed. A power cut leaves it wholly in the old place or
    /// wholly in the new.
    ///
    /// Refused: a `to` that already names something (nothing is ever
    /// replaced by a move - remove it first); a directory moved to
    /// inside itself, which would cut it loose from the tree; and a
    /// move that would put directories deeper than MAX_DEPTH, where no
    /// path could reach them and the next mount would refuse the volume.
    pub fn rename(&mut self, from: &str, to: &str) -> Result<()> {
        let (from_dirs, from_name) = split_leaf(from)?;
        let (to_dirs, to_name) = split_leaf(to)?;
        let entry = self.find(&from_dirs, from_name)?.ok_or(Error::NotFound)?;
        let same_directory = from_dirs == to_dirs;
        if same_directory && from_name == to_name {
            return Ok(()); // it is there already
        }
        if entry.kind == TYPE_DIR {
            // Inside itself: the way to the new place leads through the
            // old one. (Slices compare element by element.)
            let through = from_dirs.len() + 1;
            if to_dirs.len() >= through && to_dirs[..through - 1] == from_dirs[..] && to_dirs[through - 1] == from_name {
                return Err(Error::IntoItself);
            }
            if to_dirs.len() + 1 + self.height(entry.inode)? > MAX_DEPTH {
                return Err(Error::Name);
            }
        }
        if self.find(&to_dirs, to_name)?.is_some() {
            return Err(Error::Exists);
        }

        self.change(|v| {
            let moved = DirEntry::new(to_name, entry.inode, entry.size, entry.kind);
            let root = if same_directory {
                // One directory changes: the entry is replaced, where
                // it stands, by itself under the new name.
                v.rewrite_path(&from_dirs, from_name, Some(moved))?
            } else {
                let without = v.rewrite_path(&from_dirs, from_name, None)?;
                v.rewrite_path_from(without, &to_dirs, to_name, Some(moved))?
            };
            v.commit(root)
        })
    }
}

// ---- tests ----------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::MemDevice;
    use crate::format::VERSION;

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

    // ---- change stamps (version 4) ------------------------------------

    use crate::format::STAMPED_BLOCKS;

    fn stamp_of(volume: &mut Volume<MemDevice>, name: &str) -> u64 {
        let file = volume.lookup(name).unwrap();
        volume.stamp(&file).unwrap()
    }

    #[test]
    fn a_volume_stamps_only_once_it_is_asked_to() {
        let mut v = fresh_big();
        v.put("old", b"before").unwrap();
        assert_eq!(stamp_of(&mut v, "old"), 0);
        assert_eq!(v.superblock().version, VERSION, "an unasked volume stays one the C tool reads");

        v.allow_stamps().unwrap();
        v.put("new", b"after").unwrap();
        let generation = u64::from(v.superblock().generation);
        let device = v.into_device();
        assert_eq!(slot_versions(&device), [VERSION_STAMPS, VERSION_STAMPS]);
        let mut v = Volume::mount(device).unwrap();
        assert_eq!(stamp_of(&mut v, "new"), generation, "the stamp is the commit's generation");
        assert_eq!(stamp_of(&mut v, "old"), 0);
        assert_eq!(contents(&mut v, "old"), b"before");
    }

    #[test]
    fn what_changed_later_has_the_higher_stamp() {
        let mut v = fresh_big();
        v.allow_stamps().unwrap();
        v.put("source.c", b"int x;").unwrap();
        v.put("source.o", b"object").unwrap();
        assert!(stamp_of(&mut v, "source.o") > stamp_of(&mut v, "source.c"));

        // Changed in the middle, grown, created: each newer than all before.
        v.write_at("source.c", 4, b"y;").unwrap();
        let c = stamp_of(&mut v, "source.c");
        assert!(c > stamp_of(&mut v, "source.o"));
        v.create("empty").unwrap();
        assert!(stamp_of(&mut v, "empty") > c);

        // A new name is no change to the file; a directory has none.
        v.make_directory("d").unwrap();
        v.rename("source.c", "d/moved.c").unwrap();
        assert_eq!(stamp_of(&mut v, "d/moved.c"), c);
        assert_eq!(stamp_of(&mut v, "d"), 0);
        let mut v = remount(v);
        assert_eq!(stamp_of(&mut v, "d/moved.c"), c);
        assert_eq!(contents(&mut v, "d/moved.c"), b"int y;");
    }

    #[test]
    fn touch_gives_a_new_stamp_and_keeps_the_contents() {
        let mut v = fresh_big();
        v.allow_stamps().unwrap();
        let data = pattern(3 * BLOCK_SIZE + 5, 9);
        v.put("a", &data).unwrap();
        v.put("b", b"b").unwrap();
        v.touch("a").unwrap();
        assert!(stamp_of(&mut v, "a") > stamp_of(&mut v, "b"));
        let mut v = remount(v);
        assert_eq!(contents(&mut v, "a"), data);
        assert!(matches!(v.touch("nothere"), Err(Error::NotFound)));
        v.make_directory("d").unwrap();
        assert!(matches!(v.touch("d"), Err(Error::IsDirectory)));
    }

    #[test]
    fn a_stamped_inode_holds_two_blocks_fewer_and_a_full_old_one_is_kept_whole() {
        // 1020 blocks: one inode before stamping, too many for a stamped
        // one. Touched after stamping begins, it gets an index block.
        let mut v = fresh_big();
        let full = pattern(INODE_BLOCKS * BLOCK_SIZE, 4);
        v.put("full", &full).unwrap();
        v.allow_stamps().unwrap();
        v.touch("full").unwrap();
        // 1019 blocks, written stamped: a big file.
        let just_over = pattern((STAMPED_BLOCKS + 1) * BLOCK_SIZE - 1, 5);
        v.put("over", &just_over).unwrap();
        let mut v = remount(v);
        assert_eq!(contents(&mut v, "full"), full);
        assert_eq!(contents(&mut v, "over"), just_over);
        assert!(stamp_of(&mut v, "over") > stamp_of(&mut v, "full"));
        assert!(stamp_of(&mut v, "full") > 0);
        // Every block is accounted for: delete both, and all are free.
        let free = {
            let mut fresh = fresh_big();
            fresh.allow_stamps().unwrap();
            fresh.free_blocks()
        };
        v.delete("full").unwrap();
        v.delete("over").unwrap();
        assert_eq!(v.free_blocks(), free);
    }

    #[test]
    fn a_power_cut_while_stamping_begins_leaves_a_volume_that_mounts() {
        // allow_stamps commits twice. Cut after the first: one slot of
        // each version, the same tree; it mounts, and writes unstamped,
        // as version-3 code would, until asked again.
        let mut v = fresh_big();
        v.put("f", b"data").unwrap();
        let mut device = v.into_device();
        device.writes_left = Some(1);
        let mut v = Volume::mount(device).unwrap();
        assert!(v.allow_stamps().is_err());
        let mut device = v.into_device();
        device.writes_left = None;
        let mut v = Volume::mount(device).unwrap();
        v.put("g", b"more").unwrap();
        assert_eq!(stamp_of(&mut v, "g"), 0);
        v.allow_stamps().unwrap();
        v.put("h", b"most").unwrap();
        assert!(stamp_of(&mut v, "h") > 0);
        assert_eq!(contents(&mut v, "f"), b"data");
    }

    // ---- big files (version 3) ----------------------------------------

    use crate::format::{DIRECT_MAX, VERSION_BIG_FILES};

    /// A volume with room for big files: 16 MiB.
    const BIG_TOTAL: u32 = 4096;

    fn fresh_big() -> Volume<MemDevice> {
        let mut device = MemDevice::new(BIG_TOTAL);
        format(&mut device, BIG_TOTAL).unwrap();
        Volume::mount(device).unwrap()
    }

    /// The version each valid superblock slot says.
    fn slot_versions(device: &MemDevice) -> Vec<u32> {
        [SUPER_A, SUPER_B]
            .iter()
            .map(|&slot| Superblock::parse(&device.blocks[slot as usize]))
            .filter(|sb| sb.is_valid())
            .map(|sb| sb.version)
            .collect()
    }

    #[test]
    fn a_file_bigger_than_an_inode_reaches_survives_a_remount() {
        // Just small; one block too many for the inode; one data block
        // into a second index block; three index blocks.
        for blocks in [INODE_BLOCKS, INODE_BLOCKS + 1, INDEX_BLOCKS + 1, 2100] {
            let mut v = fresh_big();
            let data = pattern(blocks * BLOCK_SIZE - 7, blocks as u8);
            v.put("f", &data).unwrap();
            let mut v = remount(v);
            assert_eq!(contents(&mut v, "f"), data, "{blocks} blocks");
            // A few bytes at a time across the end of the first index
            // block, as the file service reads.
            let file = v.lookup("f").unwrap();
            for at in [0, INDEX_BLOCKS * BLOCK_SIZE - 3, (INODE_BLOCKS - 1) * BLOCK_SIZE + 4090] {
                if at + 10 > data.len() {
                    continue;
                }
                let mut buffer = [0u8; 10];
                assert_eq!(v.read_at(&file, at as u32, &mut buffer).unwrap(), 10);
                assert_eq!(&buffer, &data[at..at + 10], "{blocks} blocks, at {at}");
            }
        }
    }

    #[test]
    fn the_first_big_file_raises_both_slots_to_version_3() {
        let mut v = fresh_big();
        v.put("small", &pattern(DIRECT_MAX, 1)).unwrap();
        let device = v.into_device();
        assert_eq!(slot_versions(&device), [VERSION, VERSION], "the most an inode holds is not big");
        let mut v = Volume::mount(device).unwrap();
        v.put("big", &pattern(DIRECT_MAX + 1, 2)).unwrap();
        // More changes after it must not bring an older version back.
        v.delete("big").unwrap();
        v.put("x", b"x").unwrap();
        let device = v.into_device();
        assert_eq!(slot_versions(&device), [VERSION_BIG_FILES, VERSION_BIG_FILES]);
        // And directories are still allowed, without lowering it.
        let mut v = Volume::mount(device).unwrap();
        v.make_directory("d").unwrap();
        assert_eq!(slot_versions(&v.into_device()), [VERSION_BIG_FILES, VERSION_BIG_FILES]);
    }

    #[test]
    fn a_write_can_grow_a_file_into_a_big_one_and_change_its_middle() {
        let mut v = fresh_big();
        let mut expected = pattern(1000 * BLOCK_SIZE, 3);
        v.put("f", &expected).unwrap();
        // Past the end, beyond what an inode holds: the gap is zeros.
        let far = 1500 * BLOCK_SIZE + 123;
        v.write_at("f", far as u32, b"far out").unwrap();
        expected.resize(far, 0);
        expected.extend_from_slice(b"far out");
        // Across the boundary between the first two index blocks.
        let middle = INDEX_BLOCKS * BLOCK_SIZE - 4;
        v.write_at("f", middle as u32, b"straddles").unwrap();
        expected[middle..middle + 9].copy_from_slice(b"straddles");
        let mut v = remount(v);
        assert_eq!(contents(&mut v, "f"), expected);
    }

    #[test]
    fn deleting_a_big_file_returns_every_block_index_blocks_too() {
        let mut v = fresh_big();
        let free = v.free_blocks();
        let blocks = 2100;
        v.put("f", &pattern(blocks * BLOCK_SIZE, 4)).unwrap();
        // The data, three index blocks and the inode (the directory is
        // rewritten, not grown).
        assert_eq!(free - v.free_blocks(), blocks as u32 + 3 + 1);
        v.delete("f").unwrap();
        assert_eq!(v.free_blocks(), free);
        assert_eq!(remount(v).free_blocks(), free);
    }

    #[test]
    fn a_damaged_big_inode_is_refused_at_mount() {
        let mut v = fresh_big();
        v.put("f", &pattern((INODE_BLOCKS + 5) * BLOCK_SIZE, 5)).unwrap();
        let inode = v.lookup("f").unwrap().inode as usize;
        let good = v.into_device();

        // More blocks than its index blocks could name.
        let mut device = MemDevice { blocks: good.blocks.clone(), writes_left: None };
        put_u32(&mut device.blocks[inode], 12, u32::MAX);
        assert!(matches!(Volume::mount(device), Err(Error::Corrupt(_))));

        // An index block outside the volume.
        let mut device = MemDevice { blocks: good.blocks.clone(), writes_left: None };
        put_u32(&mut device.blocks[inode], 16, BIG_TOTAL + 1);
        assert!(matches!(Volume::mount(device), Err(Error::Corrupt(_))));
    }

    #[test]
    fn a_power_cut_during_a_big_put_leaves_old_or_new_state() {
        let mut v = fresh_big();
        v.put("keep", b"safe").unwrap();
        let base = v.into_device();
        let data = pattern((INODE_BLOCKS + 80) * BLOCK_SIZE, 6);

        // Every write would be some 1100 boots of a 16 MiB volume: the
        // first writes (raising the version), every 37th, and the last.
        let mut tried = 0;
        let mut writes_allowed = 0;
        loop {
            let device = MemDevice { blocks: base.blocks.clone(), writes_left: Some(writes_allowed) };
            let mut v = Volume::mount(device).unwrap();
            let finished = v.put("big", &data).is_ok();
            let mut device = v.into_device();
            device.writes_left = None;
            let versions = slot_versions(&device);
            let mut v = Volume::mount(device).expect("volume must always mount");
            let at = format!("after {writes_allowed} writes");

            assert_eq!(contents(&mut v, "keep"), b"safe", "{at}");
            if v.lookup("big").is_ok() {
                assert_eq!(contents(&mut v, "big"), data, "{at}");
                assert_eq!(versions, [VERSION_BIG_FILES, VERSION_BIG_FILES], "{at}");
            }
            assert_eq!(v.lookup("big").is_ok(), finished, "{at}");
            if finished {
                break;
            }
            tried += 1;
            writes_allowed += if (8..=INODE_BLOCKS + 75).contains(&writes_allowed) { 37 } else { 1 };
        }
        assert!(tried > 30, "only {tried} cut points were tried");
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
        power_cuts(false);
    }

    #[test]
    fn a_power_cut_at_any_write_leaves_old_or_new_state_with_stamps_too() {
        power_cuts(true);
    }

    fn power_cuts(stamping: bool) {
        let mut base = MemDevice::new(TOTAL);
        format(&mut base, TOTAL).unwrap();
        let mut v = Volume::mount(base).unwrap();
        if stamping {
            v.allow_stamps().unwrap();
        }
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
                let file = v.lookup("old").unwrap();
                assert_eq!(v.stamp(&file).unwrap() > 0, stamping, "after {writes_allowed} writes");
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

    /// The names in a directory, sorted, directories marked with a "/".
    fn names(volume: &mut Volume<MemDevice>, path: &str) -> Vec<String> {
        let mut names: Vec<String> = volume
            .list_in(path)
            .unwrap()
            .iter()
            .map(|e| if e.kind == TYPE_DIR { format!("{}/", e.name) } else { e.name.clone() })
            .collect();
        names.sort();
        names
    }

    #[test]
    fn directories_hold_files_and_directories() {
        let mut v = fresh();
        v.make_directory("docs").unwrap();
        v.make_directory("docs/notes").unwrap();
        v.put("docs/notes/today", b"deep").unwrap();
        v.put("docs/plan", &pattern(3 * BLOCK_SIZE, 4)).unwrap();
        v.put("top", b"root").unwrap();

        let free = v.free_blocks();
        let mut v = remount(v);
        // What mounting works out from the tree is what the running
        // volume believed: nothing leaked, nothing counted twice.
        assert_eq!(v.free_blocks(), free);

        assert_eq!(names(&mut v, ""), ["docs/", "top"]);
        assert_eq!(names(&mut v, "docs"), ["notes/", "plan"]);
        assert_eq!(names(&mut v, "/docs/notes/"), ["today"]);
        assert_eq!(contents(&mut v, "docs/notes/today"), b"deep");
        assert_eq!(contents(&mut v, "docs/plan"), pattern(3 * BLOCK_SIZE, 4));
        assert_eq!(v.lookup("docs").unwrap().kind, TYPE_DIR);
        assert_eq!(v.lookup("").unwrap().kind, TYPE_DIR);
        assert_eq!(v.lookup("docs/plan").unwrap().kind, TYPE_FILE);
    }

    #[test]
    fn the_same_name_in_two_directories_is_two_files() {
        let mut v = fresh();
        v.make_directory("a").unwrap();
        v.make_directory("b").unwrap();
        v.put("a/note", b"in a").unwrap();
        v.put("b/note", b"in b").unwrap();
        v.put("note", b"at the top").unwrap();

        v.write_at("b/note", 3, b"B!").unwrap();
        v.delete("a/note").unwrap();

        let mut v = remount(v);
        assert!(matches!(v.lookup("a/note"), Err(Error::NotFound)));
        assert_eq!(contents(&mut v, "b/note"), b"in B!");
        assert_eq!(contents(&mut v, "note"), b"at the top");
    }

    #[test]
    fn a_path_must_lead_through_directories() {
        let mut v = fresh();
        v.make_directory("d").unwrap();
        v.put("f", b"file").unwrap();

        // Through a file, or through nothing.
        assert!(matches!(v.put("f/x", b""), Err(Error::NotDirectory)));
        assert!(matches!(v.lookup("f/x"), Err(Error::NotDirectory)));
        assert!(matches!(v.list_in("f"), Err(Error::NotDirectory)));
        assert!(matches!(v.put("missing/x", b""), Err(Error::NotFound)));
        assert!(matches!(v.make_directory("missing/x"), Err(Error::NotFound)));

        // A directory is not a file.
        assert!(matches!(v.put("d", b"x"), Err(Error::IsDirectory)));
        assert!(matches!(v.write_at("d", 0, b"x"), Err(Error::IsDirectory)));
        let d = v.lookup("d").unwrap();
        assert!(matches!(v.read_at(&d, 0, &mut [0u8; 4]), Err(Error::IsDirectory)));

        // One name, one thing.
        assert!(matches!(v.make_directory("d"), Err(Error::Exists)));
        assert!(matches!(v.make_directory("f"), Err(Error::Exists)));
        assert!(matches!(v.create("d"), Err(Error::Exists)));

        // Names that mean "here" and "up" are not names on the disk.
        for bad in [".", "..", "d/..", "d/./x", "../f"] {
            assert!(matches!(v.put(bad, b""), Err(Error::Name)), "{bad}");
            assert!(matches!(v.make_directory(bad), Err(Error::Name)), "{bad}");
        }

        // None of the refusals changed anything.
        let mut v = remount(v);
        assert_eq!(names(&mut v, ""), ["d/", "f"]);
        assert_eq!(names(&mut v, "d"), Vec::<String>::new());
    }

    #[test]
    fn only_an_empty_directory_can_be_removed() {
        let mut v = fresh();
        let free_before = v.free_blocks();
        v.make_directory("d").unwrap();
        v.make_directory("d/inner").unwrap();
        v.put("d/inner/f", &pattern(2 * BLOCK_SIZE, 1)).unwrap();

        assert!(matches!(v.delete("d"), Err(Error::NotEmpty)));
        assert!(matches!(v.delete("d/inner"), Err(Error::NotEmpty)));
        assert_eq!(contents(&mut v, "d/inner/f"), pattern(2 * BLOCK_SIZE, 1));

        v.delete("d/inner/f").unwrap();
        v.delete("d/inner").unwrap();
        v.delete("d").unwrap();
        assert!(matches!(v.delete("d"), Err(Error::NotFound)));
        assert!(matches!(v.delete(""), Err(Error::Name)));

        // Every block came back, and a fresh mount agrees.
        assert_eq!(v.free_blocks(), free_before);
        assert_eq!(remount(v).free_blocks(), free_before);
    }

    #[test]
    fn a_directory_inside_a_directory_can_span_several_blocks() {
        let mut v = fresh();
        v.make_directory("many").unwrap();
        let count = DIR_ENTRIES * 2 + 3;
        for i in 0..count {
            v.create(&format!("many/file{i}")).unwrap();
        }
        v.put("many/file7", b"seven").unwrap();

        let free = v.free_blocks();
        let mut v = remount(v);
        assert_eq!(v.free_blocks(), free);
        assert_eq!(v.list_in("many").unwrap().len(), count);
        assert_eq!(contents(&mut v, "many/file7"), b"seven");
        assert_eq!(names(&mut v, ""), ["many/"]);
    }

    #[test]
    fn directories_nest_to_the_limit_and_no_further() {
        let mut v = fresh();
        let mut path = String::new();
        for level in 0..MAX_DEPTH {
            path.push_str(&format!("/d{level}"));
            v.make_directory(&path).unwrap();
        }
        // A file fits in the deepest one; another directory does not.
        v.put(&format!("{path}/leaf"), b"bottom").unwrap();
        assert!(matches!(v.make_directory(&format!("{path}/more")), Err(Error::Name)));
        assert!(matches!(v.put(&format!("{path}/more/leaf"), b""), Err(Error::Name)));

        // And a volume as deep as the limit allows still mounts.
        let mut v = remount(v);
        assert_eq!(contents(&mut v, &format!("{path}/leaf")), b"bottom");
    }

    /// What a disk says is not to be believed: a directory that contains
    /// itself, or one reached by two names, must stop the mount - not
    /// send it round for ever, and not let two directories share blocks.
    #[test]
    fn a_damaged_tree_is_refused_at_mount() {
        // A directory whose entry leads back to the root.
        let mut v = fresh();
        v.make_directory("d").unwrap();
        let inside = v.lookup("d").unwrap().inode;
        let root = v.superblock().root_dir;
        let mut device = v.into_device();
        let back = DirEntry::new("back", root, 0, TYPE_DIR);
        device.blocks[inside as usize] = DirBlock::new(vec![back], 0).to_block();
        assert!(matches!(Volume::mount(device), Err(Error::Corrupt(_))));

        // Two names for one directory.
        let mut v = fresh();
        v.make_directory("one").unwrap();
        let shared = v.lookup("one").unwrap().inode;
        let root = v.superblock().root_dir;
        let mut device = v.into_device();
        let entries = vec![
            DirEntry::new("one", shared, 0, TYPE_DIR),
            DirEntry::new("two", shared, 0, TYPE_DIR),
        ];
        device.blocks[root as usize] = DirBlock::new(entries, 0).to_block();
        assert!(matches!(Volume::mount(device), Err(Error::Corrupt(_))));
    }

    /// The superblock slots an implementation that knows only version 1
    /// would accept, newest first: the first is the one it would mount.
    fn version_1_slots(device: &MemDevice) -> Vec<Superblock> {
        let mut slots: Vec<Superblock> = [SUPER_A, SUPER_B]
            .iter()
            .map(|&slot| Superblock::parse(&device.blocks[slot as usize]))
            .filter(|sb| sb.is_valid() && sb.version == VERSION)
            .collect();
        slots.sort_by_key(|sb| core::cmp::Reverse(sb.generation));
        slots
    }

    #[test]
    fn a_volume_without_directories_stays_version_1() {
        let mut v = fresh();
        v.put("a", b"1").unwrap();
        v.write_at("a", 1, b"2").unwrap();
        v.create("b").unwrap();
        v.delete("b").unwrap();
        assert_eq!(v.superblock().version, VERSION);
        assert_eq!(version_1_slots(&v.into_device()).len(), 2);
    }

    /// The first directory changes the format's version - and at no
    /// moment, whenever the power goes, may the disk hold a directory
    /// while a slot still says "version 1": that slot is all an old
    /// implementation would see.
    #[test]
    fn the_first_directory_locks_old_readers_out_before_it_exists() {
        let mut v = fresh();
        v.put("keep", b"safe").unwrap();
        let base = v.into_device();
        let mut cuts = 0;

        for writes_allowed in 0.. {
            let device = MemDevice { blocks: base.blocks.clone(), writes_left: Some(writes_allowed) };
            let mut v = Volume::mount(device).unwrap();
            let made = v.make_directory("d").is_ok();
            // More changes after it, if it got that far: they must not
            // bring version 1 back.
            let finished = made && v.put("d/f", b"x").is_ok() && v.delete("d/f").is_ok() && v.delete("d").is_ok();

            let mut device = v.into_device();
            device.writes_left = None;
            let old_slots = version_1_slots(&device);
            let mut v = Volume::mount(device).expect("volume must always mount");
            let at = format!("after {writes_allowed} writes");

            assert_eq!(contents(&mut v, "keep"), b"safe", "{at}");
            let has_directory = v.lookup("d").is_ok();
            if has_directory {
                assert!(old_slots.is_empty(), "{at}: a directory, and a slot an old reader would mount");
            }
            // If an old reader would still mount the volume, what it
            // mounts is the tree as it is - not an older one, whose
            // free blocks the live tree may be using.
            if let Some(theirs) = old_slots.first() {
                assert_eq!(theirs.root_dir, v.superblock().root_dir, "{at}");
            }
            if made {
                assert_eq!(v.superblock().version, VERSION_DIRECTORIES, "{at}");
            }
            if finished {
                // Empty of directories again - and still version 2.
                assert_eq!(v.superblock().version, VERSION_DIRECTORIES);
                break;
            }
            cuts += 1;
        }
        assert!(cuts > 6, "only {cuts} cut points were tried");
    }

    /// The power-cut property again, for a change deep in the tree. Such
    /// a change rewrites every directory above it, so there are more
    /// writes to be cut between - and still nothing may show until the
    /// one write that commits it all.
    #[test]
    fn a_rename_in_place_changes_only_the_name() {
        let mut v = fresh();
        v.put("first", b"one").unwrap();
        v.put("draft", &pattern(3 * BLOCK_SIZE, 9)).unwrap();
        v.put("last", b"three").unwrap();
        let free = v.free_blocks();
        let generation = v.superblock().generation;

        v.rename("draft", "final").unwrap();
        assert_eq!(v.superblock().generation, generation + 1, "one commit");
        // The same blocks under another name: none taken, none lost.
        assert_eq!(v.free_blocks(), free);
        let mut v = remount(v);
        assert_eq!(v.free_blocks(), free);
        assert_eq!(names(&mut v, ""), ["final", "first", "last"]);
        assert_eq!(contents(&mut v, "final"), pattern(3 * BLOCK_SIZE, 9));
        assert!(matches!(v.lookup("draft"), Err(Error::NotFound)));
        // Where it stood in the directory, it still stands.
        let order: Vec<String> = v.list().unwrap().into_iter().map(|e| e.name).collect();
        assert_eq!(order, ["first", "final", "last"]);
        // A volume with no directories is still one the old tool reads.
        assert_eq!(v.superblock().version, VERSION);
    }

    #[test]
    fn a_rename_moves_between_directories() {
        let mut v = fresh();
        v.make_directory("a").unwrap();
        v.make_directory("a/b").unwrap();
        v.make_directory("c").unwrap();
        v.put("a/b/file", b"deep").unwrap();
        v.put("top", b"root").unwrap();
        let free = v.free_blocks();

        // Down, up, sideways - and with a new name on the way.
        v.rename("top", "a/b/top").unwrap();
        v.rename("a/b/file", "file").unwrap();
        v.rename("a/b/top", "c/moved").unwrap();
        assert_eq!(v.free_blocks(), free);
        let mut v = remount(v);
        assert_eq!(v.free_blocks(), free);
        assert_eq!(names(&mut v, ""), ["a/", "c/", "file"]);
        assert!(names(&mut v, "a/b").is_empty());
        assert_eq!(names(&mut v, "c"), ["moved"]);
        assert_eq!(contents(&mut v, "file"), b"deep");
        assert_eq!(contents(&mut v, "c/moved"), b"root");
    }

    #[test]
    fn a_directory_moves_with_everything_in_it() {
        let mut v = fresh();
        v.make_directory("docs").unwrap();
        v.make_directory("docs/notes").unwrap();
        v.put("docs/notes/today", b"deep").unwrap();
        v.put("docs/plan", &pattern(2 * BLOCK_SIZE, 4)).unwrap();
        v.make_directory("archive").unwrap();
        let free = v.free_blocks();

        v.rename("docs", "archive/old").unwrap();
        assert_eq!(v.free_blocks(), free);
        let mut v = remount(v);
        assert_eq!(v.free_blocks(), free);
        assert_eq!(names(&mut v, ""), ["archive/"]);
        assert_eq!(names(&mut v, "archive/old"), ["notes/", "plan"]);
        assert_eq!(contents(&mut v, "archive/old/notes/today"), b"deep");
        assert_eq!(contents(&mut v, "archive/old/plan"), pattern(2 * BLOCK_SIZE, 4));

        // And out again, from inside what its parent was.
        v.rename("archive/old/notes", "notes").unwrap();
        v.rename("archive", "notes/archive").unwrap();
        let mut v = remount(v);
        assert_eq!(names(&mut v, ""), ["notes/"]);
        assert_eq!(names(&mut v, "notes"), ["archive/", "today"]);
        assert_eq!(names(&mut v, "notes/archive/old"), ["plan"]);
    }

    #[test]
    fn a_rename_that_cannot_be_changes_nothing() {
        let mut v = fresh();
        v.make_directory("d").unwrap();
        v.make_directory("d/inner").unwrap();
        v.make_directory("dd").unwrap();
        v.put("d/f", b"file").unwrap();
        v.put("g", b"other").unwrap();
        let free = v.free_blocks();
        let generation = v.superblock().generation;

        assert!(matches!(v.rename("nothing", "x"), Err(Error::NotFound)));
        assert!(matches!(v.rename("g", "nowhere/x"), Err(Error::NotFound)));
        assert!(matches!(v.rename("g", "d/f/x"), Err(Error::NotDirectory)));
        assert!(matches!(v.rename("g/x", "y"), Err(Error::NotDirectory)));
        // Nothing is replaced: not a file, not a directory, not by either.
        assert!(matches!(v.rename("g", "d/f"), Err(Error::Exists)));
        assert!(matches!(v.rename("g", "d"), Err(Error::Exists)));
        assert!(matches!(v.rename("d", "g"), Err(Error::Exists)));
        assert!(matches!(v.rename("d", "dd"), Err(Error::Exists)));
        // Into itself, at any depth - but not into a neighbour whose
        // name merely starts the same way.
        assert!(matches!(v.rename("d", "d/x"), Err(Error::IntoItself)));
        assert!(matches!(v.rename("d", "d/inner/x"), Err(Error::IntoItself)));
        assert!(matches!(v.rename("d/inner", "d/inner/inner"), Err(Error::IntoItself)));
        for bad in ["", "/", ".", "d/..", "a/./b", &"n".repeat(NAME_MAX)[..]] {
            assert!(matches!(v.rename("g", bad), Err(Error::Name)), "{bad:?}");
            assert!(matches!(v.rename(bad, "x"), Err(Error::Name)), "{bad:?}");
        }
        // To where it already is: nothing to do, and nothing done.
        v.rename("g", "/g").unwrap();
        v.rename("d/inner", "d//inner/").unwrap();

        assert_eq!(v.superblock().generation, generation, "nothing was committed");
        assert_eq!(v.free_blocks(), free);
        let mut v = remount(v);
        assert_eq!(names(&mut v, ""), ["d/", "dd/", "g"]);
        assert_eq!(names(&mut v, "d"), ["f", "inner/"]);

        // The neighbour: "dd" is not inside "d".
        v.rename("d", "dd/d").unwrap();
        assert_eq!(names(&mut v, "dd/d"), ["f", "inner/"]);
    }

    #[test]
    fn a_move_cannot_put_directories_out_of_reach() {
        let mut v = fresh();
        let mut path = String::new();
        for level in 0..MAX_DEPTH - 1 {
            path.push_str(&format!("/d{level}"));
            v.make_directory(&path).unwrap();
        }
        // `path` is one short of the limit: a directory with nothing in
        // it fits there, one holding a directory does not - a file can
        // go anywhere a directory is.
        v.make_directory("flat").unwrap();
        v.make_directory("tall").unwrap();
        v.make_directory("tall/inner").unwrap();
        v.put("tall/inner/file", b"x").unwrap();
        assert!(matches!(v.rename("tall", &format!("{path}/tall")), Err(Error::Name)));
        v.rename("flat", &format!("{path}/flat")).unwrap();
        v.rename("tall/inner/file", &format!("{path}/flat/file")).unwrap();
        assert!(matches!(v.rename("tall/inner", &format!("{path}/flat/inner")), Err(Error::Name)));
        // Whatever was allowed, the volume still mounts.
        let mut v = remount(v);
        assert_eq!(contents(&mut v, &format!("{path}/flat/file")), b"x");
        assert_eq!(names(&mut v, "tall"), ["inner/"]);
    }

    #[test]
    fn a_rename_with_no_room_changes_nothing() {
        // A move needs free blocks for the directories it rewrites,
        // though it keeps none of them.
        let mut device = MemDevice::new(16);
        format(&mut device, 16).unwrap();
        let mut v = Volume::mount(device).unwrap();
        v.make_directory("a").unwrap();
        v.put("f", b"x").unwrap();
        while v.free_blocks() > 2 {
            let n = v.free_blocks();
            v.put(&format!("fill{n}"), b"").unwrap();
        }
        let free = v.free_blocks();
        assert!(matches!(v.rename("f", "a/f"), Err(Error::Full)));
        assert_eq!(v.free_blocks(), free, "what the failed move took is back");
        let mut v = remount(v);
        assert_eq!(contents(&mut v, "f"), b"x");
        assert!(names(&mut v, "a").is_empty());
    }

    /// A move is two changes to the tree and one commit: at no cut is
    /// the thing in both places, or in neither.
    #[test]
    fn a_power_cut_during_a_move_leaves_it_in_one_place() {
        let mut v = fresh();
        v.make_directory("from").unwrap();
        v.make_directory("from/sub").unwrap();
        v.make_directory("to").unwrap();
        v.make_directory("to/deep").unwrap();
        v.put("from/sub/file", b"contents").unwrap();
        v.put("from/sub/other", b"stays").unwrap();
        let base = v.into_device();
        let mut cuts = 0;

        for writes_allowed in 0.. {
            let device = MemDevice { blocks: base.blocks.clone(), writes_left: Some(writes_allowed) };
            let mut v = Volume::mount(device).unwrap();
            // A file across the tree, then the directory it came from.
            let first = v.rename("from/sub/file", "to/deep/file");
            let second = first.is_ok().then(|| v.rename("from/sub", "to/sub"));
            let done = usize::from(first.is_ok()) + usize::from(matches!(second, Some(Ok(()))));

            let mut device = v.into_device();
            device.writes_left = None; // power is back
            let mut v = Volume::mount(device).expect("volume must always mount");
            let at = format!("after {writes_allowed} writes, {done} moves done");

            assert_eq!(v.lookup("to/deep/file").is_ok(), done >= 1, "{at}");
            assert_eq!(v.lookup("from/sub/file").is_ok(), done == 0, "{at}");
            assert_eq!(contents(&mut v, if done >= 1 { "to/deep/file" } else { "from/sub/file" }), b"contents", "{at}");
            assert_eq!(v.lookup("to/sub").is_ok(), done == 2, "{at}");
            assert_eq!(v.lookup("from/sub").is_ok(), done < 2, "{at}");
            assert_eq!(contents(&mut v, if done == 2 { "to/sub/other" } else { "from/sub/other" }), b"stays", "{at}");

            if done == 2 {
                break;
            }
            cuts += 1;
        }
        assert!(cuts > 6, "only {cuts} cut points were tried");
    }

    #[test]
    fn a_power_cut_during_a_nested_change_leaves_old_or_new_state() {
        let mut v = fresh();
        v.make_directory("docs").unwrap();
        v.make_directory("docs/notes").unwrap();
        v.put("docs/notes/old", b"before").unwrap();
        v.put("docs/keep", b"untouched").unwrap();
        let base = v.into_device();

        let new_data = pattern(2 * BLOCK_SIZE + 10, 5);
        let mut cuts = 0;

        for writes_allowed in 0.. {
            let device = MemDevice {
                blocks: base.blocks.clone(),
                writes_left: Some(writes_allowed),
            };
            let mut v = Volume::mount(device).unwrap();

            // Four changes, each its own commit; each is tried only if
            // the one before it finished.
            let mut done = 0;
            type Step<'a> = &'a dyn Fn(&mut Volume<MemDevice>) -> Result<()>;
            let steps: [Step; 4] = [
                &|v| v.put("docs/notes/old", b"after"),
                &|v| v.make_directory("docs/notes/deep"),
                &|v| v.put("docs/notes/deep/new", &new_data),
                &|v| v.delete("docs/keep"),
            ];
            for step in steps {
                if step(&mut v).is_err() {
                    break;
                }
                done += 1;
            }

            let mut device = v.into_device();
            device.writes_left = None; // power is back
            let mut v = Volume::mount(device).expect("volume must always mount");
            let at = format!("after {writes_allowed} writes, {done} changes done");

            // Exactly the finished changes are visible, each complete.
            let old = contents(&mut v, "docs/notes/old");
            assert_eq!(old, if done >= 1 { &b"after"[..] } else { &b"before"[..] }, "{at}");
            assert_eq!(v.lookup("docs/notes/deep").is_ok(), done >= 2, "{at}");
            assert_eq!(v.lookup("docs/notes/deep/new").is_ok(), done >= 3, "{at}");
            if done >= 3 {
                assert_eq!(contents(&mut v, "docs/notes/deep/new"), new_data, "{at}");
            }
            assert_eq!(v.lookup("docs/keep").is_ok(), done < 4, "{at}");
            if done < 4 {
                assert_eq!(contents(&mut v, "docs/keep"), b"untouched", "{at}");
            }

            if done == 4 {
                break;
            }
            cuts += 1;
        }

        assert!(cuts > 12, "only {cuts} cut points were tried");
    }
}
