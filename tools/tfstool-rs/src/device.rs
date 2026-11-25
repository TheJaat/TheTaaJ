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
    /// Where the volume starts in the file, in bytes: 0, or a TaajFS
    /// partition's start (an image made for a USB stick).
    start: u64,
}

impl FileDevice {
    /// Takes ownership of `file`. When the FileDevice is dropped, the
    /// file is closed with it - no `fclose` to forget on an error path.
    pub fn new(file: File) -> FileDevice {
        FileDevice { file, start: 0 }
    }

    /// The volume in the file's MBR partition of type 0x7F, if its
    /// first sector lists one (`taajfs::partition`); else the whole
    /// file. The second value is the partition's first sector, if any.
    pub fn find(mut file: File) -> io::Result<(FileDevice, Option<u64>)> {
        let mut sector0 = [0u8; taajfs::partition::SECTOR];
        let length = file.metadata()?.len();
        file.seek(SeekFrom::Start(0))?;
        let partition = match file.read_exact(&mut sector0) {
            Ok(()) => taajfs::partition::find_taajfs(&sector0, length / taajfs::partition::SECTOR as u64),
            Err(_) => None, // shorter than a sector: no partition table
        };
        let start = partition.map_or(0, |(first, _)| first * taajfs::partition::SECTOR as u64);
        Ok((FileDevice { file, start }, partition.map(|(first, _)| first)))
    }

    fn byte_offset(&self, number: u32) -> u64 {
        self.start + u64::from(number) * BLOCK_SIZE as u64
    }
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
        self.file.seek(SeekFrom::Start(self.byte_offset(number))).map_err(&error)?;

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
        self.file.seek(SeekFrom::Start(self.byte_offset(number))).map_err(&error)?;
        // `write_all` keeps writing until every byte is out or an error
        // occurs - a short write cannot slip through unnoticed.
        self.file.write_all(buffer).map_err(&error)
    }

    /// Nothing: the image is a file on the host, and what was written
    /// is in the host's hands once `write_all` returns - it survives
    /// this program ending, which is all an image being built needs.
    /// Making it survive the host losing power too (`sync_data`) would
    /// cost a host disk flush at every commit, thousands in a test run,
    /// for images that are thrown away.
    fn flush(&mut self) -> Result<(), DeviceError> {
        Ok(())
    }
}
