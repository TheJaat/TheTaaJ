//! Channels: the state of a two-ended message pipe.
//!
//! A channel has two ends, 0 and 1. A message sent on one end lands in
//! the other end's inbox, so one channel carries traffic both ways - a
//! request one way, the reply back. Messages keep their boundaries (one
//! send, one receive) and their order.
//!
//! The inbox is bounded. A sender that outpaces its receiver gets
//! QueueFull instead of the kernel quietly growing a queue until memory
//! runs out: a misbehaving process must not be able to exhaust the
//! kernel's memory through someone else's channel.
//!
//! When an end closes, its inbox is emptied - nobody can read it any
//! more - and the other end learns of it: sends fail with PeerClosed,
//! and a receive gets PeerClosed once the inbox is drained. This is how
//! a client learns that the service it talks to has crashed.
//!
//! The messages are generic over `H`, the handles they carry; the kernel
//! wraps this with locking and blocking (ipc.rs). Dropping a message
//! drops its handles, so the kernel takes care to drop them outside its
//! locks - every function here hands dropped messages back to the caller
//! rather than dropping them itself.

use alloc::collections::VecDeque;
use alloc::vec::Vec;

use abi::Error;

pub struct Message<H> {
    pub data: Vec<u8>,
    pub handles: Vec<H>,
}

pub struct Channel<H> {
    /// inbox[n]: messages waiting to be received at end n.
    inbox: [VecDeque<Message<H>>; 2],
    open: [bool; 2],
    capacity: usize,
}

/// The other end.
pub fn peer(end: usize) -> usize {
    1 - end
}

impl<H> Channel<H> {
    pub fn new(capacity: usize) -> Channel<H> {
        Channel { inbox: [VecDeque::new(), VecDeque::new()], open: [true, true], capacity }
    }

    pub fn is_open(&self, end: usize) -> bool {
        self.open[end]
    }

    /// Whether a send from `end` would be accepted now.
    pub fn can_send(&self, end: usize) -> Result<(), Error> {
        let to = peer(end);
        if !self.open[to] {
            Err(Error::PeerClosed)
        } else if self.inbox[to].len() >= self.capacity {
            Err(Error::QueueFull)
        } else {
            Ok(())
        }
    }

    /// Queues `message` for the other end. On error the message comes
    /// back, so the caller keeps its handles.
    pub fn send(&mut self, end: usize, message: Message<H>) -> Result<(), (Error, Message<H>)> {
        if let Err(error) = self.can_send(end) {
            return Err((error, message));
        }
        self.inbox[peer(end)].push_back(message);
        Ok(())
    }

    /// The size of the next message for `end`: (bytes, handles). None if
    /// there is none yet; PeerClosed if none will ever come.
    pub fn peek(&self, end: usize) -> Result<Option<(usize, usize)>, Error> {
        match self.inbox[end].front() {
            Some(m) => Ok(Some((m.data.len(), m.handles.len()))),
            None if !self.open[peer(end)] => Err(Error::PeerClosed),
            None => Ok(None),
        }
    }

    pub fn receive(&mut self, end: usize) -> Option<Message<H>> {
        self.inbox[end].pop_front()
    }

    /// Closes `end`. Returns the messages that were waiting for it, for
    /// the caller to drop (outside any lock).
    pub fn close(&mut self, end: usize) -> VecDeque<Message<H>> {
        self.open[end] = false;
        core::mem::take(&mut self.inbox[end])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(text: &str) -> Message<u32> {
        Message { data: text.as_bytes().to_vec(), handles: Vec::new() }
    }

    fn text(m: Message<u32>) -> String {
        String::from_utf8(m.data).unwrap()
    }

    #[test]
    fn messages_cross_both_ways_in_order() {
        let mut c = Channel::new(4);
        c.send(0, msg("one")).ok().unwrap();
        c.send(0, msg("two")).ok().unwrap();
        c.send(1, msg("reply")).ok().unwrap();
        assert_eq!(c.peek(1), Ok(Some((3, 0))));
        assert_eq!(text(c.receive(1).unwrap()), "one");
        assert_eq!(text(c.receive(1).unwrap()), "two");
        assert!(c.receive(1).is_none());
        assert_eq!(text(c.receive(0).unwrap()), "reply");
        assert_eq!(c.peek(0), Ok(None));
    }

    #[test]
    fn a_full_inbox_pushes_back_and_returns_the_message() {
        let mut c = Channel::new(2);
        c.send(0, msg("a")).ok().unwrap();
        c.send(0, msg("b")).ok().unwrap();
        let (error, back) = c.send(0, Message { data: vec![], handles: vec![7] }).err().unwrap();
        assert_eq!(error, Error::QueueFull);
        assert_eq!(back.handles, vec![7], "the handle comes back to the sender");
        c.receive(1);
        assert!(c.send(0, msg("c")).is_ok(), "room again");
    }

    #[test]
    fn closing_an_end_tells_the_other_and_discards_its_inbox() {
        let mut c = Channel::new(4);
        c.send(0, msg("unread")).ok().unwrap();
        c.send(1, msg("for 0")).ok().unwrap();
        let discarded = c.close(1);
        assert_eq!(discarded.len(), 1, "1's unread message goes with it");

        assert_eq!(c.send(0, msg("x")).err().unwrap().0, Error::PeerClosed);
        // What 1 sent before closing still arrives; then PeerClosed.
        assert_eq!(c.peek(0), Ok(Some((5, 0))));
        c.receive(0);
        assert_eq!(c.peek(0), Err(Error::PeerClosed));
    }
}
