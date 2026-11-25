//! Where blocks come from.
//!
//! In C, tfscore takes two function pointers and a `void *Context`. The
//! Rust equivalent is a *trait*: a set of methods a type promises to
//! provide. `Volume` works with anything that implements `BlockDevice`,
//! and the compiler checks the promise - there is no `void *` to cast
//! back to the wrong thing.
//!
//! Implementations: a disk in memory, here, for tests; an image file, in
//! the host tool; virtio disk requests, in TheTaaJ's file service.

use alloc::vec;
use alloc::vec::Vec;
use core::fmt;

use crate::format::{empty_block, Block};

/// What a device reports when a block cannot be read or written: what
/// failed, and a number the device chose (an OS error code, a disk
/// status) - no `std::io::Error` here, as there is no `std`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceError {
    pub what: &'static str,
    pub code: Option<i64>,
}

impl DeviceError {
    pub const fn new(what: &'static str) -> DeviceError {
        DeviceError { what, code: None }
    }
}

impl fmt::Display for DeviceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.code {
            Some(code) => write!(f, "{} (error {code})", self.what),
            None => f.write_str(self.what),
        }
    }
}

impl core::error::Error for DeviceError {}

pub trait BlockDevice {
    fn read_block(&mut self, number: u32, buffer: &mut Block) -> Result<(), DeviceError>;
    fn write_block(&mut self, number: u32, buffer: &Block) -> Result<(), DeviceError>;

    /// Every block written so far, on the medium - not only in the
    /// device's cache, where a power cut would lose it, or let a later
    /// write land before an earlier one. A disk answers a write as soon
    /// as it has the data; this is how a file system knows the order
    /// it wrote in is the order that survives (Volume's commit).
    ///
    /// No default: each device says what it means for it - a disk
    /// sends a FLUSH, an image file on the host may leave it to the
    /// host, a disk in memory has nothing to do.
    fn flush(&mut self) -> Result<(), DeviceError>;

    /// Blocks `first`, `first + 1`, ... into `buffer`, whose length is a
    /// whole number of blocks. One block at a time unless a device can
    /// do better - a disk, in one request (journal entry 080).
    fn read_blocks(&mut self, first: u32, buffer: &mut [u8]) -> Result<(), DeviceError> {
        let mut block = empty_block();
        for (n, part) in buffer.chunks_exact_mut(block.len()).enumerate() {
            self.read_block(first + n as u32, &mut block)?;
            part.copy_from_slice(&block);
        }
        Ok(())
    }
}

/// A disk held in memory, for tests.
///
/// `writes_left` simulates a power cut: once it reaches zero, every
/// further write fails and changes nothing. Running an operation with
/// every possible cut-off point is how the tests prove copy-on-write
/// really leaves the old state intact.
pub struct MemDevice {
    pub blocks: Vec<Block>,
    pub writes_left: Option<usize>,
}

impl MemDevice {
    pub fn new(count: u32) -> MemDevice {
        MemDevice { blocks: vec![empty_block(); count as usize], writes_left: None }
    }
}

impl BlockDevice for MemDevice {
    fn read_block(&mut self, number: u32, buffer: &mut Block) -> Result<(), DeviceError> {
        let block = self.blocks.get(number as usize).ok_or(DeviceError::new("read past the end"))?;
        *buffer = *block;
        Ok(())
    }

    fn write_block(&mut self, number: u32, buffer: &Block) -> Result<(), DeviceError> {
        if let Some(left) = self.writes_left.as_mut() {
            if *left == 0 {
                return Err(DeviceError::new("simulated power loss"));
            }
            *left -= 1;
        }
        let block = self.blocks.get_mut(number as usize).ok_or(DeviceError::new("write past the end"))?;
        *block = *buffer;
        Ok(())
    }

    /// Memory has no cache in front of it: every write is "on the
    /// medium" when it returns.
    fn flush(&mut self) -> Result<(), DeviceError> {
        Ok(())
    }
}

/// A disk in memory with a write cache, for tests: what a real disk
/// does, and what MemDevice does not.
///
/// A write goes into the cache and is answered; it reaches `blocks` -
/// the medium - only when `flush` says so. A power cut (`lose_power`)
/// loses the cache, except for the writes the disk happened to have
/// written out already - any of them, in any order, which the test
/// chooses. Reads see the cache, as a real disk's do.
pub struct CachingDisk {
    pub blocks: Vec<Block>,
    /// Writes answered but not yet on the medium, oldest first.
    pub cached: Vec<(u32, Block)>,
    /// Like MemDevice's `writes_left`, but counting flushes too: once it
    /// reaches zero, every write and every flush fails. The power can go
    /// while a flush is under way - after the last write of a change
    /// and before the disk has written it out - and that moment is the
    /// one an order of writes is for.
    pub requests_left: Option<usize>,
    pub flushes: usize,
}

impl CachingDisk {
    pub fn new(blocks: Vec<Block>, requests_left: Option<usize>) -> CachingDisk {
        CachingDisk { blocks, cached: Vec::new(), requests_left, flushes: 0 }
    }

    /// One more request, if the power holds.
    fn request(&mut self) -> Result<(), DeviceError> {
        if let Some(left) = self.requests_left.as_mut() {
            if *left == 0 {
                return Err(DeviceError::new("simulated power loss"));
            }
            *left -= 1;
        }
        Ok(())
    }

    /// The power goes: of the cached writes, those for which `landed`
    /// says yes reached the medium, the rest are lost. Then the power
    /// is back, with nothing cached and no limit on writes.
    pub fn lose_power(&mut self, mut landed: impl FnMut(u32) -> bool) {
        for (number, block) in core::mem::take(&mut self.cached) {
            if landed(number) {
                self.blocks[number as usize] = block;
            }
        }
        self.requests_left = None;
    }
}

impl BlockDevice for CachingDisk {
    fn read_block(&mut self, number: u32, buffer: &mut Block) -> Result<(), DeviceError> {
        let on_medium = self.blocks.get(number as usize).ok_or(DeviceError::new("read past the end"))?;
        let newest = self.cached.iter().rev().find(|(n, _)| *n == number).map(|(_, block)| block);
        *buffer = *newest.unwrap_or(on_medium);
        Ok(())
    }

    fn write_block(&mut self, number: u32, buffer: &Block) -> Result<(), DeviceError> {
        self.request()?;
        if number as usize >= self.blocks.len() {
            return Err(DeviceError::new("write past the end"));
        }
        self.cached.push((number, *buffer));
        Ok(())
    }

    fn flush(&mut self) -> Result<(), DeviceError> {
        self.request()?;
        for (number, block) in core::mem::take(&mut self.cached) {
            self.blocks[number as usize] = block;
        }
        self.flushes += 1;
        Ok(())
    }
}
