//! Files: the client side of the file service's protocol.
//!
//! A program that was given a channel to the `fs` service makes a
//! `Files` from it. Each call is one or more request-reply exchanges:
//! a request goes out on the shared request channel together with one
//! end of a fresh channel, and the reply comes back on the other end -
//! so replies cannot get mixed up between clients, and if the service
//! dies before answering, its end of the reply channel dies with it and
//! the wait ends in PeerClosed instead of lasting forever.
//!
//! Files bigger than one message are read and written a message's worth
//! at a time.

use fileserver::protocol::{self, Request, Status, MESSAGE_MAX};

use crate::{Error, Handle};

/// Why a file operation failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileError {
    /// The service answered: no.
    Refused(Status),
    /// It could not be asked, or did not answer.
    Unreachable(Error),
}

impl FileError {
    pub fn describe(self) -> &'static str {
        match self {
            FileError::Refused(status) => status.describe(),
            FileError::Unreachable(Error::PeerClosed) => "the file service stopped before answering",
            FileError::Unreachable(_) => "the file service cannot be reached",
        }
    }
}

pub struct Files {
    requests: Handle,
}

impl Files {
    pub fn new(requests: Handle) -> Files {
        Files { requests }
    }

    /// One exchange. Returns the reply's payload length (in `reply`
    /// after the status byte).
    fn call(&self, request: Request, reply: &mut [u8; MESSAGE_MAX]) -> Result<usize, FileError> {
        let mut message = [0u8; MESSAGE_MAX];
        let length = request.encode(&mut message).ok_or(FileError::Refused(Status::BadName))?;
        let (answer, theirs) = crate::channel_create().map_err(FileError::Unreachable)?;
        let sent = crate::send(self.requests, &message[..length], &[theirs]);
        if sent.is_err() {
            let _ = crate::close(theirs); // not sent: still ours
        }
        let received = sent.and_then(|()| crate::receive(answer, reply, &mut []));
        let _ = crate::close(answer);
        match received.map_err(FileError::Unreachable)? {
            (0, _) => Err(FileError::Refused(Status::BadRequest)),
            (n, _) => match Status::from_u8(reply[0]) {
                Status::Ok => Ok(n - 1),
                status => Err(FileError::Refused(status)),
            },
        }
    }

    /// Every file's name and size, in name order.
    pub fn list(&self, mut each: impl FnMut(&str, u64)) -> Result<usize, FileError> {
        let mut reply = [0u8; MESSAGE_MAX];
        let mut count = 0;
        loop {
            let n = self.call(Request::new(protocol::LIST, "", count as u64), &mut reply)?;
            let before = count;
            for (name, size) in protocol::entries(&reply[1..=n]) {
                each(name, size);
                count += 1;
            }
            if count == before {
                return Ok(count);
            }
        }
    }

    /// The whole file, a piece at a time. Returns its size.
    pub fn read(&self, name: &str, mut each: impl FnMut(&[u8])) -> Result<u64, FileError> {
        let mut reply = [0u8; MESSAGE_MAX];
        let mut offset = 0u64;
        loop {
            let n = self.call(Request::new(protocol::READ, name, offset), &mut reply)?;
            if n > 0 {
                each(&reply[1..=n]);
            }
            offset += n as u64;
            if n < protocol::READ_MAX {
                return Ok(offset);
            }
        }
    }

    /// Replaces the file's contents with `data`, creating it if needed.
    pub fn write(&self, name: &str, data: &[u8]) -> Result<u64, FileError> {
        let mut reply = [0u8; MESSAGE_MAX];
        self.call(Request::new(protocol::TRUNCATE, name, 0), &mut reply)?;
        self.write_at(name, 0, data)
    }

    /// Adds `data` at the end of the file, creating it if needed.
    pub fn append(&self, name: &str, data: &[u8]) -> Result<u64, FileError> {
        self.write_at(name, protocol::APPEND, data)
    }

    fn write_at(&self, name: &str, offset: u64, data: &[u8]) -> Result<u64, FileError> {
        if name.len() > protocol::NAME_MAX {
            return Err(FileError::Refused(Status::BadName));
        }
        let mut reply = [0u8; MESSAGE_MAX];
        let mut size = None;
        let mut offset = offset;
        // At least one request, so an empty write still creates the file.
        let mut chunks = data.chunks(protocol::write_max(name).max(1));
        let first: &[u8] = chunks.next().unwrap_or(&[]);
        for chunk in core::iter::once(first).chain(chunks) {
            let request = Request { op: protocol::WRITE, name, number: offset, data: chunk };
            let n = self.call(request, &mut reply)?;
            size = protocol::number(&reply[1..=n]);
            if offset != protocol::APPEND {
                offset += chunk.len() as u64;
            }
        }
        size.ok_or(FileError::Refused(Status::BadRequest))
    }

    pub fn size(&self, name: &str) -> Result<u64, FileError> {
        let mut reply = [0u8; MESSAGE_MAX];
        let n = self.call(Request::new(protocol::STAT, name, 0), &mut reply)?;
        protocol::number(&reply[1..=n]).ok_or(FileError::Refused(Status::BadRequest))
    }

    pub fn remove(&self, name: &str) -> Result<(), FileError> {
        let mut reply = [0u8; MESSAGE_MAX];
        self.call(Request::new(protocol::REMOVE, name, 0), &mut reply).map(|_| ())
    }
}
