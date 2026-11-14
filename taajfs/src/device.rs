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
}
