//! The network: the client side of the network service's protocol
//! (`abi::net`).
//!
//! `Net::find()` asks for the service by its name, "net". From there:
//! `ping` (is that machine there?); `udp_bind`, which gives a
//! `UdpSocket` - a port of this machine, on which datagrams are sent
//! and arrive; and TCP: `tcp_connect` gives a `TcpStream` - a
//! connection, bytes in order in both directions - and `tcp_listen` a
//! `TcpListener`, which hands out the connections others make to a
//! port here.

use abi::net::{self, Info, Ping};

use crate::{Error, Handle};

pub use ::net::Ipv4;

/// Why the service did not do what it was asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetError {
    /// The service answered with this status (`abi::net`).
    Refused(u8),
    Unreachable(Error),
}

impl NetError {
    pub fn describe(self) -> &'static str {
        match self {
            NetError::Refused(status) => net::describe(status),
            NetError::Unreachable(Error::PeerClosed) => "no network (no card, or its service stopped)",
            NetError::Unreachable(Error::TimedOut) => "the network service is not answering",
            NetError::Unreachable(_) => "the network service cannot be reached",
        }
    }
}

/// A ping's answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pong {
    pub from: Ipv4,
    /// How many more routers the answer could have passed: it starts
    /// at a number the sender chose (often 64 or 255) and each router
    /// takes one off.
    pub ttl: u8,
}

pub struct Net {
    requests: Handle,
}

const REPLY_MAX: usize = 1 + Info::SIZE;

impl Net {
    pub fn new(requests: Handle) -> Net {
        Net { requests }
    }

    /// The network service, if this program was given it.
    pub fn find() -> Option<Net> {
        crate::services()?.lookup("net").ok().map(Net::new)
    }

    /// One exchange; the payload's length (after the status). `also`
    /// goes with the request - and is the service's from then on, or
    /// closed here if the request could not be sent. `wait` is how
    /// long the service may take by its own account, on top of the
    /// usual.
    fn call(&self, request: &[u8], also: Option<Handle>, wait: u64, reply: &mut [u8; REPLY_MAX]) -> Result<usize, NetError> {
        let (answer, theirs) = crate::channel_create().map_err(NetError::Unreachable)?;
        let sent = match also {
            Some(also) => crate::send(self.requests, request, &[theirs, also]),
            None => crate::send(self.requests, request, &[theirs]),
        };
        if sent.is_err() {
            // Not sent: both are still ours, and nobody else's to close.
            let _ = crate::close(theirs);
            if let Some(also) = also {
                let _ = crate::close(also);
            }
        }
        let deadline = crate::ticks() + wait + crate::ANSWER_WAIT;
        let received = sent.and_then(|()| crate::receive_until(answer, reply, &mut [], deadline));
        let _ = crate::close(answer);
        match received.map_err(NetError::Unreachable)? {
            (0, _) => Err(NetError::Refused(net::BAD_REQUEST)),
            (n, _) if reply[0] == net::OK => Ok(n - 1),
            _ => Err(NetError::Refused(reply[0])),
        }
    }

    /// Who this machine is on its network, and what has been counted.
    pub fn info(&self) -> Result<Info, NetError> {
        let mut reply = [0u8; REPLY_MAX];
        let length = self.call(&[net::INFO], None, 0, &mut reply)?;
        Info::from_bytes(&reply[1..1 + length]).ok_or(NetError::Refused(net::BAD_REQUEST))
    }

    /// Asks `to` to answer, with `length` bytes of data sent and
    /// expected back, and waits up to `wait` ticks for it.
    pub fn ping(&self, to: Ipv4, sequence: u16, length: u16, wait: u32) -> Result<Pong, NetError> {
        let mut reply = [0u8; REPLY_MAX];
        let request = Ping { to: to.0, sequence, length, wait }.to_bytes();
        match self.call(&request, None, u64::from(wait), &mut reply)? {
            net::PING_REPLY_SIZE => Ok(Pong { from: Ipv4([reply[1], reply[2], reply[3], reply[4]]), ttl: reply[5] }),
            _ => Err(NetError::Refused(net::BAD_REQUEST)),
        }
    }

    /// Makes a connection to `port` on `to`, and waits until it is
    /// made or known not to be - half a minute at the longest, if
    /// nothing answers at all.
    pub fn tcp_connect(&self, to: Ipv4, port: u16) -> Result<TcpStream, NetError> {
        let (ours, theirs) = crate::channel_create().map_err(NetError::Unreachable)?;
        let mut reply = [0u8; REPLY_MAX];
        match self.call(&net::connect_encode(to.0, port), Some(theirs), net::CONNECT_WAIT, &mut reply) {
            Ok(2) => Ok(TcpStream::new(ours, u16::from_le_bytes([reply[1], reply[2]]), to, port)),
            outcome => {
                let _ = crate::close(ours);
                Err(outcome.err().unwrap_or(NetError::Refused(net::BAD_REQUEST)))
            }
        }
    }

    /// Listens on `port` of this machine - or, for port 0, on a free
    /// one of the service's choosing (`TcpListener::port`).
    pub fn tcp_listen(&self, port: u16) -> Result<TcpListener, NetError> {
        let (ours, theirs) = crate::channel_create().map_err(NetError::Unreachable)?;
        let [low, high] = port.to_le_bytes();
        let mut reply = [0u8; REPLY_MAX];
        match self.call(&[net::TCP_LISTEN, low, high], Some(theirs), 0, &mut reply) {
            Ok(2) => Ok(TcpListener { channel: ours, port: u16::from_le_bytes([reply[1], reply[2]]), asked: false }),
            outcome => {
                let _ = crate::close(ours);
                Err(outcome.err().unwrap_or(NetError::Refused(net::BAD_REQUEST)))
            }
        }
    }

    /// A socket on `port` of this machine - or, for port 0, on a free
    /// one of the service's choosing (`UdpSocket::port` says which).
    pub fn udp_bind(&self, port: u16) -> Result<UdpSocket, NetError> {
        let (ours, theirs) = crate::channel_create().map_err(NetError::Unreachable)?;
        let mut request = [net::UDP_BIND, 0, 0];
        request[1..].copy_from_slice(&port.to_le_bytes());
        let mut reply = [0u8; REPLY_MAX];
        match self.call(&request, Some(theirs), 0, &mut reply) {
            Ok(2) => Ok(UdpSocket { channel: ours, port: u16::from_le_bytes([reply[1], reply[2]]) }),
            outcome => {
                let _ = crate::close(ours);
                Err(outcome.err().unwrap_or(NetError::Refused(net::BAD_REQUEST)))
            }
        }
    }
}

/// A port of this machine: datagrams out, and datagrams in.
pub struct UdpSocket {
    channel: Handle,
    port: u16,
}

impl UdpSocket {
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The channel datagrams arrive on, to wait on together with other
    /// things (`object_wait`).
    pub fn channel(&self) -> Handle {
        self.channel
    }

    /// Sends `data` to `port` on `to`. No answer comes, and none means
    /// nothing: a datagram may be lost anywhere on the way. An error
    /// is only what was noticed here - too much data (InvalidArgument),
    /// or the service not keeping up (QueueFull).
    pub fn send_to(&self, to: Ipv4, port: u16, data: &[u8]) -> Result<(), Error> {
        let mut message = [0u8; abi::MESSAGE_MAX as usize];
        let length = net::datagram_encode(&mut message, to.0, port, data).ok_or(Error::InvalidArgument)?;
        crate::send(self.channel, &message[..length], &[])
    }

    /// The next datagram: its data copied to the start of `data`, and
    /// who sent it, from which port, and how many bytes. None if none
    /// has come by tick `deadline`. PeerClosed: the service stopped.
    pub fn receive_until(&self, data: &mut [u8; net::DATAGRAM_MAX], deadline: u64) -> Result<Option<(Ipv4, u16, usize)>, Error> {
        let mut message = [0u8; abi::MESSAGE_MAX as usize];
        loop {
            let n = match crate::receive_until(self.channel, &mut message, &mut [], deadline) {
                Ok((n, _)) => n,
                Err(Error::TimedOut) => return Ok(None),
                Err(error) => return Err(error),
            };
            if let Some((from, port, payload)) = net::datagram_decode(&message[..n]) {
                data[..payload.len()].copy_from_slice(payload);
                return Ok(Some((Ipv4(from), port, payload.len())));
            }
        }
    }
}

impl Drop for UdpSocket {
    fn drop(&mut self) {
        let _ = crate::close(self.channel);
    }
}

/// A port of this machine that others can connect to.
pub struct TcpListener {
    channel: Handle,
    port: u16,
    /// The service has been told one more connection will be taken,
    /// and has not handed one over yet.
    asked: bool,
}

impl TcpListener {
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The channel connections arrive on, to wait on together with
    /// other things - after `accept_until` has returned None once
    /// (which is what asks for one).
    pub fn channel(&self) -> Handle {
        self.channel
    }

    /// The next connection made to the port. None if none has been by
    /// tick `deadline`. (Connections made while nobody is accepting
    /// wait - a few of them; more than that are not answered, and
    /// their makers try again.)
    pub fn accept_until(&mut self, deadline: u64) -> Result<Option<TcpStream>, NetError> {
        if !self.asked {
            crate::send(self.channel, &[net::LISTENER_ACCEPT], &[]).map_err(NetError::Unreachable)?;
            self.asked = true;
        }
        let mut message = [0u8; net::DATAGRAM_HEADER];
        let mut handles = [0u64; 1];
        match crate::receive_until(self.channel, &mut message, &mut handles, deadline) {
            Ok((length, 1)) => {
                self.asked = false;
                match net::datagram_decode(&message[..length]) {
                    Some((from, from_port, _)) => Ok(Some(TcpStream::new(handles[0], self.port, Ipv4(from), from_port))),
                    None => {
                        let _ = crate::close(handles[0]);
                        Err(NetError::Refused(net::BAD_REQUEST))
                    }
                }
            }
            Ok(_) => Err(NetError::Refused(net::BAD_REQUEST)),
            Err(Error::TimedOut) => Ok(None),
            Err(error) => Err(NetError::Unreachable(error)),
        }
    }
}

impl Drop for TcpListener {
    fn drop(&mut self) {
        let _ = crate::close(self.channel);
    }
}

/// A TCP connection: what is written at one end can be read at the
/// other, in order, all of it - or the connection fails, and says so.
///
/// Dropping it closes the connection: what was written is still sent.
pub struct TcpStream {
    channel: Handle,
    local_port: u16,
    peer: (Ipv4, u16),
    /// Bytes that arrived in one message and were not all asked for
    /// yet: `held[start..end]`.
    held: [u8; net::STREAM_DATA_MAX],
    start: usize,
    end: usize,
    /// The other side has said it has no more to send.
    ended: bool,
    failed: Option<u8>,
}

impl TcpStream {
    fn new(channel: Handle, local_port: u16, to: Ipv4, port: u16) -> TcpStream {
        TcpStream { channel, local_port, peer: (to, port), held: [0; net::STREAM_DATA_MAX], start: 0, end: 0, ended: false, failed: None }
    }

    /// The port of this machine the connection uses.
    pub fn local_port(&self) -> u16 {
        self.local_port
    }

    /// The other end: its address and its port.
    pub fn peer(&self) -> (Ipv4, u16) {
        self.peer
    }

    /// The channel what arrives comes on, to wait on together with
    /// other things. (Bytes may also be waiting here already: read
    /// with a deadline of 0 first.)
    pub fn channel(&self) -> Handle {
        self.channel
    }

    /// Reads into `out`: how many bytes - at least one, and no more
    /// than there are, which may be fewer than `out` holds. `Some(0)`
    /// is the end: the other side has no more to send. None if nothing
    /// has come by tick `deadline`.
    pub fn read_until(&mut self, out: &mut [u8], deadline: u64) -> Result<Option<usize>, NetError> {
        let mut message = [0u8; abi::MESSAGE_MAX as usize];
        loop {
            if self.start < self.end {
                let count = out.len().min(self.end - self.start);
                out[..count].copy_from_slice(&self.held[self.start..self.start + count]);
                self.start += count;
                return Ok(Some(count));
            }
            if out.is_empty() || self.ended {
                return Ok(Some(0));
            }
            if let Some(status) = self.failed {
                return Err(NetError::Refused(status));
            }
            match crate::receive_until(self.channel, &mut message, &mut [], deadline) {
                Ok((length, _)) => match &message[..length] {
                    [net::STREAM_DATA, data @ ..] => {
                        self.held[..data.len()].copy_from_slice(data);
                        (self.start, self.end) = (0, data.len());
                    }
                    [net::STREAM_END] => self.ended = true,
                    [net::STREAM_FAILED, status] => self.failed = Some(*status),
                    _ => {}
                },
                Err(Error::TimedOut) => return Ok(None),
                // Closed without a word: the service itself is gone.
                Err(error) => return Err(NetError::Unreachable(error)),
            }
        }
    }

    /// As `read_until`, for as long as it takes. 0 is the end.
    pub fn read(&mut self, out: &mut [u8]) -> Result<usize, NetError> {
        Ok(self.read_until(out, u64::MAX)?.unwrap_or(0))
    }

    /// One message to the service - waiting while it has no room for
    /// one, which is how a program is held back to the speed of the
    /// network and of whoever is reading at the other end.
    fn send(&mut self, message: &[u8]) -> Result<(), NetError> {
        loop {
            match crate::send(self.channel, message, &[]) {
                Ok(()) => return Ok(()),
                Err(Error::QueueFull) => crate::sleep(1),
                // The service closed the connection's channel: the
                // connection is over. Why is in a message this side
                // may not have read yet; "cut off" is true in any case.
                Err(Error::PeerClosed) => return Err(NetError::Refused(self.failed.unwrap_or(net::RESET))),
                Err(error) => return Err(NetError::Unreachable(error)),
            }
        }
    }

    /// Writes all of `data`. That it returned says the bytes are on
    /// their way - not that they arrived: a failure of the connection
    /// shows at a later write, or at a read.
    pub fn write(&mut self, data: &[u8]) -> Result<(), NetError> {
        let mut message = [0u8; abi::MESSAGE_MAX as usize];
        message[0] = net::STREAM_DATA;
        for piece in data.chunks(net::STREAM_DATA_MAX) {
            message[1..1 + piece.len()].copy_from_slice(piece);
            self.send(&message[..1 + piece.len()])?;
        }
        Ok(())
    }

    /// No more will be written: the other side reads to the end and
    /// is told it is the end. Reading goes on.
    pub fn shutdown(&mut self) -> Result<(), NetError> {
        self.send(&[net::STREAM_END])
    }
}

impl Drop for TcpStream {
    fn drop(&mut self) {
        let _ = crate::close(self.channel);
    }
}
