//! Where the tool's blocks come from: an image file on the host.
//!
//! The `BlockDevice` trait and the in-memory test disk live in the
//! `taajfs` crate, with the format and the volume; this adds the one
//! device only a host has.

use std::fs::File;
use std::io::{self, ErrorKind, Read, Seek, SeekFrom, Write};

pub use taajfs::device::{BlockDevice, DeviceError, MemDevice};
use taajfs::format::{Block, BLOCK_SIZE};

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

/// An I/O error as the volume sees it: what failed, and the OS's code.
fn device_error(what: &'static str) -> impl Fn(io::Error) -> DeviceError {
    move |e| DeviceError { what, code: e.raw_os_error().map(i64::from) }
}

impl BlockDevice for FileDevice {
    /// Past the end of the file reads as zeros, as in the C tool: that
    /// is what an unwritten part of a fresh image looks like.
    fn read_block(&mut self, number: u32, buffer: &mut Block) -> Result<(), DeviceError> {
        let error = device_error("reading the image");
        self.file.seek(SeekFrom::Start(byte_offset(number))).map_err(&error)?;

        let mut filled = 0;
        while filled < BLOCK_SIZE {
            match self.file.read(&mut buffer[filled..]) {
                Ok(0) => break, // end of file
                Ok(n) => filled += n,
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => return Err(error(e)),
            }
        }
        buffer[filled..].fill(0);
        Ok(())
    }

    fn write_block(&mut self, number: u32, buffer: &Block) -> Result<(), DeviceError> {
        let error = device_error("writing the image");
        self.file.seek(SeekFrom::Start(byte_offset(number))).map_err(&error)?;
        // `write_all` keeps writing until every byte is out or an error
        // occurs - a short write cannot slip through unnoticed.
        self.file.write_all(buffer).map_err(&error)
    }
}
