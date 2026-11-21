//! A network card: the client side of its driver's protocol
//! (`abi::nic`).
//!
//! Whole Ethernet frames, out and in. Sending is a message or two and
//! no answer. Receiving is a channel of this program's own, handed to
//! the driver once (`listen`): every frame of the types asked for that
//! the card takes in for this machine arrives on it from then on.
//!
//! One program *sends* on a channel to the driver at a time: the
//! driver puts the pieces of a frame together in the order they
//! arrive, whoever sent them.

use abi::nic::{self, Assembler, Fed, Stats};

use crate::{Error, Handle};

/// Why the driver did not do what it was asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NicError {
    /// The driver answered with this status.
    Refused(u8),
    Unreachable(Error),
}

impl NicError {
    pub fn describe(self) -> &'static str {
        match self {
            NicError::Refused(_) => "the network card's driver did not understand",
            NicError::Unreachable(Error::PeerClosed) => "no network card (or its driver stopped)",
            NicError::Unreachable(Error::TimedOut) => "the network card's driver is not answering",
            NicError::Unreachable(_) => "the network card's driver cannot be reached",
        }
    }
}

/// What the card says of itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Info {
    pub address: [u8; nic::ADDRESS],
    pub link_up: bool,
}

pub struct Nic {
    requests: Handle,
    /// A frame's first piece went out and its last did not: the driver
    /// must be told to forget it before the next frame begins.
    torn: bool,
}

const REPLY_MAX: usize = 1 + Stats::SIZE;

impl Nic {
    pub fn new(requests: Handle) -> Nic {
        Nic { requests, torn: false }
    }

    /// One exchange; returns the payload length (after the status).
    fn call(&self, request: &[u8], reply: &mut [u8; REPLY_MAX]) -> Result<usize, NicError> {
        let (answer, theirs) = crate::channel_create().map_err(NicError::Unreachable)?;
        let sent = crate::send(self.requests, request, &[theirs]);
        if sent.is_err() {
            let _ = crate::close(theirs);
        }
        let deadline = crate::ticks() + crate::ANSWER_WAIT;
        let received = sent.and_then(|()| crate::receive_until(answer, reply, &mut [], deadline));
        let _ = crate::close(answer);
        match received.map_err(NicError::Unreachable)? {
            (0, _) => Err(NicError::Refused(nic::BAD_REQUEST)),
            (n, _) if reply[0] == nic::OK => Ok(n - 1),
            _ => Err(NicError::Refused(reply[0])),
        }
    }

    pub fn info(&self) -> Result<Info, NicError> {
        let mut reply = [0u8; REPLY_MAX];
        match self.call(&[nic::INFO], &mut reply)? {
            nic::INFO_SIZE => {
                let mut address = [0u8; nic::ADDRESS];
                address.copy_from_slice(&reply[1..=nic::ADDRESS]);
                Ok(Info { address, link_up: reply[1 + nic::ADDRESS] != 0 })
            }
            _ => Err(NicError::Refused(nic::BAD_REQUEST)),
        }
    }

    pub fn stats(&self) -> Result<Stats, NicError> {
        let mut reply = [0u8; REPLY_MAX];
        let length = self.call(&[nic::STATS], &mut reply)?;
        Stats::from_bytes(&reply[1..1 + length]).ok_or(NicError::Refused(nic::BAD_REQUEST))
    }

    /// A request as raw bytes, answered or not - for checking what the
    /// driver makes of wrong ones.
    pub fn raw(&self, request: &[u8]) -> Result<(), NicError> {
        let mut reply = [0u8; REPLY_MAX];
        self.call(request, &mut reply).map(|_| ())
    }

    /// A message as raw bytes with no handle: a piece of a frame, or
    /// something that is not one.
    pub fn raw_piece(&self, message: &[u8]) -> Result<(), Error> {
        crate::send(self.requests, message, &[])
    }

    /// From now on every frame of these types (the two bytes after
    /// the addresses; at most `nic::LISTEN_TYPES_MAX`) that the card
    /// receives for this machine comes to the returned `Frames` - and
    /// no longer to whoever asked for them before. With no types: every
    /// frame of a type nobody asked for. When this returns, the driver
    /// is listening for them.
    pub fn listen(&self, types: &[u16]) -> Result<Frames, Error> {
        let mut request = [0u8; 1 + 2 * nic::LISTEN_TYPES_MAX];
        request[0] = nic::LISTEN;
        let count = types.len().min(nic::LISTEN_TYPES_MAX);
        for (bytes, kind) in request[1..].as_chunks_mut::<2>().0.iter_mut().zip(&types[..count]) {
            *bytes = kind.to_be_bytes();
        }
        let (ours, theirs) = crate::channel_create()?;
        match crate::send(self.requests, &request[..1 + 2 * count], &[theirs]) {
            Ok(()) => {
                let frames = Frames { channel: ours, assembler: Assembler::new() };
                // LISTEN has no answer, so by itself it says nothing of
                // *when* the driver has taken it in. A question after
                // it does: the driver reads its requests in order. A
                // frame that arrives once this returns is not missed.
                match self.info() {
                    Ok(_) => Ok(frames),
                    Err(NicError::Unreachable(error)) => Err(error),
                    Err(NicError::Refused(_)) => Err(Error::InvalidArgument),
                }
            }
            Err(error) => {
                let _ = (crate::close(ours), crate::close(theirs));
                Err(error)
            }
        }
    }

    /// Sends one frame: destination, source, type, payload - 14 to
    /// 1514 bytes. No answer comes. QueueFull if the driver is not
    /// keeping up: the frame was not sent, and may be offered again.
    pub fn send(&mut self, frame: &[u8]) -> Result<(), Error> {
        if self.torn {
            crate::send(self.requests, &[nic::DROP], &[])?;
            self.torn = false;
        }
        let mut message = [0u8; abi::MESSAGE_MAX as usize];
        for (n, (op, piece)) in nic::split(frame).enumerate() {
            message[0] = op;
            message[1..=piece.len()].copy_from_slice(piece);
            if let Err(error) = crate::send(self.requests, &message[..=piece.len()], &[]) {
                // Pieces already gone are no frame without this one.
                self.torn = n > 0;
                return Err(error);
            }
        }
        Ok(())
    }
}

/// The frames the card receives, as they arrive.
pub struct Frames {
    channel: Handle,
    assembler: Assembler,
}

impl Frames {
    /// The channel the frames arrive on, to wait on together with
    /// other things (`object_wait`).
    pub fn channel(&self) -> Handle {
        self.channel
    }

    /// The next frame, copied to the start of `frame`; its length. None
    /// if none has come by tick `deadline`. PeerClosed: the driver has
    /// stopped, or someone else listens now.
    pub fn next_until(&mut self, frame: &mut [u8; nic::FRAME_MAX], deadline: u64) -> Result<Option<usize>, Error> {
        let mut message = [0u8; abi::MESSAGE_MAX as usize];
        loop {
            let n = match crate::receive_until(self.channel, &mut message, &mut [], deadline) {
                Ok((n, _)) => n,
                Err(Error::TimedOut) => return Ok(None),
                Err(error) => return Err(error),
            };
            // (A frame too long is the driver's mistake, not a frame.)
            if let Fed::Frame(whole) = self.assembler.feed(&message[..n]) {
                frame[..whole.len()].copy_from_slice(whole);
                return Ok(Some(whole.len()));
            }
        }
    }
}

impl Drop for Frames {
    fn drop(&mut self) {
        let _ = crate::close(self.channel);
    }
}
