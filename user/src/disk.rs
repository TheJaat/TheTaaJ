//! A disk: the client side of the block service's protocol.
//!
//! The same shape as `files`: each request goes out with the end of a
//! fresh channel, and the reply comes back on it - or PeerClosed, if
//! the driver died first (or there is no disk at all).
//!
//! One sector fits in a message. More than one goes through a `Window`:
//! memory this process and the driver both see for the length of one
//! request. The request then says only "sectors 800 to 807"; the bytes
//! are already where both can reach them, and are copied once - between
//! the window and the device's own buffer - instead of being cut into
//! messages.

use abi::block;

use crate::{Error, Handle};

/// Why a disk operation failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiskError {
    /// The driver answered with this status.
    Refused(u8),
    Unreachable(Error),
}

impl DiskError {
    pub fn describe(self) -> &'static str {
        match self {
            DiskError::Refused(status) => block::describe(status),
            DiskError::Unreachable(Error::PeerClosed) => "no disk (or its driver stopped)",
            DiskError::Unreachable(_) => "the disk service cannot be reached",
        }
    }
}

pub struct Disk {
    requests: Handle,
}

/// Memory for moving many sectors at once: a memory object, mapped
/// here. Each request lends the driver a handle to it.
pub struct Window {
    memory: Handle,
    base: *mut u8,
    sectors: u32,
}

impl Window {
    /// A window big enough for `sectors` sectors (up to SECTORS_MAX).
    pub fn new(sectors: u32) -> Result<Window, Error> {
        let sectors = sectors.clamp(1, block::SECTORS_MAX);
        let memory = crate::memory_create(u64::from(sectors) * block::SECTOR as u64)?;
        match crate::memory_map(memory, true) {
            Ok(base) => Ok(Window { memory, base, sectors }),
            Err(error) => {
                let _ = crate::close(memory);
                Err(error)
            }
        }
    }

    pub fn sectors(&self) -> u32 {
        self.sectors
    }

    /// The window's bytes: what a read left there, or what a write is
    /// to send.
    pub fn bytes(&mut self) -> &mut [u8] {
        // SAFETY: the mapping, `sectors` sectors long, alive as long as
        // the window; the driver touches it only during a request, and
        // a request borrows the window for its whole length.
        unsafe { core::slice::from_raw_parts_mut(self.base, self.sectors as usize * block::SECTOR) }
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        let _ = crate::memory_unmap(self.base);
        let _ = crate::close(self.memory);
    }
}

const REPLY_MAX: usize = 1 + block::SECTOR;

impl Disk {
    pub fn new(requests: Handle) -> Disk {
        Disk { requests }
    }

    /// One exchange; returns the payload length (after the status byte).
    fn call(&self, request: &[u8], reply: &mut [u8; REPLY_MAX]) -> Result<usize, DiskError> {
        self.call_with(request, None, reply)
    }

    /// One exchange, lending the driver `lent` - a handle that goes
    /// with the request, and is closed here if the request never left.
    fn call_with(&self, request: &[u8], lent: Option<Handle>, reply: &mut [u8; REPLY_MAX]) -> Result<usize, DiskError> {
        let (answer, theirs) = match crate::channel_create() {
            Ok(ends) => ends,
            Err(error) => {
                if let Some(lent) = lent {
                    let _ = crate::close(lent);
                }
                return Err(DiskError::Unreachable(error));
            }
        };
        let handles = [theirs, lent.unwrap_or(0)];
        let handles = &handles[..1 + usize::from(lent.is_some())];
        let sent = crate::send(self.requests, request, handles);
        if sent.is_err() {
            for &handle in handles {
                let _ = crate::close(handle);
            }
        }
        let received = sent.and_then(|()| crate::receive(answer, reply, &mut []));
        let _ = crate::close(answer);
        match received.map_err(DiskError::Unreachable)? {
            (0, _) => Err(DiskError::Refused(block::BAD_REQUEST)),
            (n, _) if reply[0] == block::OK => Ok(n - 1),
            _ => Err(DiskError::Refused(reply[0])),
        }
    }

    /// (sectors, read-only)
    pub fn info(&self) -> Result<(u64, bool), DiskError> {
        let mut reply = [0u8; REPLY_MAX];
        match self.call(&[block::INFO], &mut reply)? {
            9 => Ok((u64::from_le_bytes(reply[1..9].try_into().unwrap_or([0; 8])), reply[9] != 0)),
            _ => Err(DiskError::Refused(block::BAD_REQUEST)),
        }
    }

    pub fn read(&self, sector: u64, out: &mut [u8; block::SECTOR]) -> Result<(), DiskError> {
        let mut request = [0u8; block::REQUEST_SIZE];
        request[0] = block::READ;
        request[1..].copy_from_slice(&sector.to_le_bytes());
        let mut reply = [0u8; REPLY_MAX];
        match self.call(&request, &mut reply)? {
            block::SECTOR => {
                out.copy_from_slice(&reply[1..]);
                Ok(())
            }
            _ => Err(DiskError::Refused(block::BAD_REQUEST)),
        }
    }

    /// A request as raw bytes, with whatever handle the caller chooses
    /// to lend - for checking that the driver refuses wrong ones.
    pub fn raw(&self, request: &[u8], lent: Option<Handle>) -> Result<(), DiskError> {
        let mut reply = [0u8; REPLY_MAX];
        self.call_with(request, lent, &mut reply).map(|_| ())
    }

    /// `count` sectors from `sector` on, into the start of `window`.
    pub fn read_many(&self, window: &mut Window, sector: u64, count: u32) -> Result<(), DiskError> {
        self.many(block::READ_MANY, window, sector, count)
    }

    /// The first `count` sectors' worth of `window`, to `sector` on.
    pub fn write_many(&self, window: &mut Window, sector: u64, count: u32) -> Result<(), DiskError> {
        self.many(block::WRITE_MANY, window, sector, count)
    }

    fn many(&self, op: u8, window: &mut Window, sector: u64, count: u32) -> Result<(), DiskError> {
        if count == 0 || count > window.sectors {
            return Err(DiskError::Refused(block::BAD_REQUEST));
        }
        let mut request = [0u8; block::MANY_REQUEST_SIZE];
        request[0] = op;
        request[1..9].copy_from_slice(&sector.to_le_bytes());
        request[9..].copy_from_slice(&count.to_le_bytes());
        // The driver's own handle to the window, for this request: to
        // read what is to be written, or also to write what was read.
        let rights = match op {
            block::READ_MANY => crate::rights::READ | crate::rights::WRITE,
            _ => crate::rights::READ,
        };
        let lent = crate::duplicate(window.memory, rights | crate::rights::TRANSFER).map_err(DiskError::Unreachable)?;
        let mut reply = [0u8; REPLY_MAX];
        self.call_with(&request, Some(lent), &mut reply).map(|_| ())
    }

    pub fn write(&self, sector: u64, data: &[u8; block::SECTOR]) -> Result<(), DiskError> {
        let mut request = [0u8; block::REQUEST_SIZE + block::SECTOR];
        request[0] = block::WRITE;
        request[1..9].copy_from_slice(&sector.to_le_bytes());
        request[9..].copy_from_slice(data);
        let mut reply = [0u8; REPLY_MAX];
        self.call(&request, &mut reply).map(|_| ())
    }
}
