//! Where blocks come from.
//!
//! In C, tfscore takes two function pointers and a `void *Context`. The
//! Rust equivalent is a *trait*: a set of methods a type promises to
//! provide. `Volume` works with anything that implements `BlockDevice`,
//! and the compiler checks the promise - there is no `void *` to cast
//! back to the wrong thing.
//!
//! Two implementations live here: an image file for the real tool, and
//! a disk in memory for tests, which can also pretend to lose power.

use std::fs::File;
use std::io::{self, ErrorKind, Read, Seek, SeekFrom, Write};

use crate::format::{empty_block, Block, BLOCK_SIZE};

pub trait BlockDevice {
    fn read_block(&mut self, number: u32, buffer: &mut Block) -> io::Result<()>;
    fn write_block(&mut self, number: u32, buffer: &Block) -> io::Result<()>;
}

/// A volume image on the host.
pub struct FileDevice {
    file: File,
}

impl FileDevice {
    /// Takes ownership of `file`. When the FileDevice is dropped, the
    /// file is closed with it - no `fclose` to forget on an error path.
    pub fn new(file: File) -> FileDevice {
        FileDevice { file }
    }
}

fn byte_offset(number: u32) -> u64 {
    u64::from(number) * BLOCK_SIZE as u64
}

impl BlockDevice for FileDevice {
    /// Past the end of the file reads as zeros, as in the C tool: that
    /// is what an unwritten part of a fresh image looks like.
    fn read_block(&mut self, number: u32, buffer: &mut Block) -> io::Result<()> {
        self.file.seek(SeekFrom::Start(byte_offset(number)))?;

        let mut filled = 0;
        while filled < BLOCK_SIZE {
            match self.file.read(&mut buffer[filled..]) {
                Ok(0) => break, // end of file
                Ok(n) => filled += n,
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
        buffer[filled..].fill(0);
        Ok(())
    }

    fn write_block(&mut self, number: u32, buffer: &Block) -> io::Result<()> {
        self.file.seek(SeekFrom::Start(byte_offset(number)))?;
        // `write_all` keeps writing until every byte is out or an error
        // occurs - a short write cannot slip through unnoticed.
        self.file.write_all(buffer)
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
        MemDevice {
            blocks: vec![empty_block(); count as usize],
            writes_left: None,
        }
    }
}

impl BlockDevice for MemDevice {
    fn read_block(&mut self, number: u32, buffer: &mut Block) -> io::Result<()> {
        match self.blocks.get(number as usize) {
            Some(block) => {
                *buffer = *block;
                Ok(())
            }
            None => Err(io::Error::new(
                ErrorKind::UnexpectedEof,
                "read past the end",
            )),
        }
    }

    fn write_block(&mut self, number: u32, buffer: &Block) -> io::Result<()> {
        if let Some(left) = self.writes_left.as_mut() {
            if *left == 0 {
                return Err(io::Error::other("simulated power loss"));
            }
            *left -= 1;
        }
        match self.blocks.get_mut(number as usize) {
            Some(block) => {
                *block = *buffer;
                Ok(())
            }
            None => Err(io::Error::new(
                ErrorKind::UnexpectedEof,
                "write past the end",
            )),
        }
    }
}
