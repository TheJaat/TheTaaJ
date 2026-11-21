//! The network: the client side of the network service's protocol
//! (`abi::net`).
//!
//! `Net::find()` asks for the service by its name, "net". From there:
//! `ping` (is that machine there?), and `udp_bind`, which gives a
//! `UdpSocket` - a port of this machine, on which datagrams are sent
//! and arrive.

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
