//! A disk: the client side of the block service's protocol.
//!
//! The same shape as `files`: each request goes out with the end of a
//! fresh channel, and the reply comes back on it - or PeerClosed, if
//! the driver died first (or there is no disk at all).

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

const REPLY_MAX: usize = 1 + block::SECTOR;

impl Disk {
    pub fn new(requests: Handle) -> Disk {
        Disk { requests }
    }

    /// One exchange; returns the payload length (after the status byte).
    fn call(&self, request: &[u8], reply: &mut [u8; REPLY_MAX]) -> Result<usize, DiskError> {
        let (answer, theirs) = crate::channel_create().map_err(DiskError::Unreachable)?;
        let sent = crate::send(self.requests, request, &[theirs]);
        if sent.is_err() {
            let _ = crate::close(theirs);
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

    pub fn write(&self, sector: u64, data: &[u8; block::SECTOR]) -> Result<(), DiskError> {
        let mut request = [0u8; block::REQUEST_SIZE + block::SECTOR];
        request[0] = block::WRITE;
        request[1..9].copy_from_slice(&sector.to_le_bytes());
        request[9..].copy_from_slice(data);
        let mut reply = [0u8; REPLY_MAX];
        self.call(&request, &mut reply).map(|_| ())
    }
}
