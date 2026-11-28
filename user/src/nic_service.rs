//! The network card service: the card protocol (`abi::nic`), served.
//!
//! Every network card driver answers the same requests on the same
//! channel, so that the network stack - and the card's tests - cannot
//! tell which driver is behind it: `nic` for a virtio card, `e1000` for
//! an Intel one. What differs is only how frames get between the wire
//! and memory; that is a driver's `Card`. Everything else lives here,
//! once:
//!
//!   - who listens for which kind of frame (`abi::nic::Listeners`), and
//!     handing each arriving frame to its listener, in pieces;
//!   - assembling a frame to send from a client's pieces;
//!   - the questions (INFO, STATS) and the watchdog's test (STALL);
//!   - waiting: for the card's interrupt, for a client, or - with no
//!     interrupt - a tick at a time.
//!
//! What this service knows about a frame is its first six bytes - who
//! it is for - and the two that say what kind of thing is in it. A
//! frame for another card is dropped here (an emulated virtio card
//! hands over everything on the wire). The kind decides who gets it:
//! the network stack listens for IPv4 and ARP, the card's test for its
//! own kind, without either disturbing the other.
//!
//! No listener is ever waited for. A frame that arrives when no one
//! listens, or when the listener's queue is full, is dropped and
//! counted: the next one may be for someone who is keeping up. A
//! network loses frames anyway, and everything built on one copes.
//!
//! The card is waited for: while it has no room for another frame, no
//! more requests are read. They stay in the channel, and when that is
//! full the client is told so (QueueFull) - the client that sends
//! faster than the wire takes is the one that waits.

use abi::nic::{self, Assembler, Fed, Listeners, Stats};

use crate::{println, Error, Handle};

/// A network card, as a driver drives it.
pub trait Card {
    /// Its Ethernet address.
    fn address(&self) -> [u8; nic::ADDRESS];
    /// Whether it has a link: a cable, and something at the other end.
    fn link_up(&mut self) -> bool;
    /// Every frame the card has received since the last call, in order,
    /// to `pass` - the frame (without anything the card puts in front
    /// of it), or None for one that could not be used. Each buffer goes
    /// back to the card after `pass` has seen it.
    fn receive(&mut self, pass: &mut dyn FnMut(Option<&[u8]>));
    /// Puts `frame` (FRAME_MIN to FRAME_MAX bytes: checked before this
    /// is called) on the wire, filled up to WIRE_MIN. False if the card
    /// has no room.
    fn send(&mut self, frame: &[u8]) -> bool;
    /// Takes back what the card has finished sending.
    fn take_sent(&mut self);
    /// True if the card has no room for another frame - and then, if it
    /// interrupts, it has been asked to say when it has.
    fn transmit_full(&mut self) -> bool;
    /// After an interrupt: tells the card it was seen.
    fn interrupt_seen(&mut self);
}

/// What the service keeps besides the card.
struct Service {
    stats: Stats,
    /// Who gets the frames that arrive: by the type of each.
    listeners: Listeners<Handle, { nic::LISTENERS_MAX }>,
    /// The frame a client is sending, as its pieces arrive.
    outgoing: Assembler,
    /// Frames to send until the driver dies in one (K82), 0 for never.
    crash_in: u64,
}

impl Service {
    /// One frame from the card: passed on, or dropped and counted.
    fn arrived(&mut self, frame: Option<&[u8]>, address: &[u8; nic::ADDRESS]) {
        self.stats.received += 1;
        match frame {
            Some(frame) if (nic::FRAME_MIN..=nic::FRAME_MAX).contains(&frame.len()) => {
                if nic::for_us(frame, address) {
                    self.pass_on(frame);
                } else {
                    self.stats.not_ours += 1;
                }
            }
            // Not a frame at all: nothing to pass on.
            _ => self.stats.dropped += 1,
        }
    }

    /// Sends `frame` to whoever listens for frames of its type.
    fn pass_on(&mut self, frame: &[u8]) {
        let Some(listener) = nic::frame_type(frame).and_then(|kind| self.listeners.find(kind)) else {
            self.stats.dropped += 1;
            return;
        };
        let to = listener.who;
        let mut message = [0u8; abi::MESSAGE_MAX as usize];
        let mut send = |op: u8, piece: &[u8]| {
            message[0] = op;
            message[1..=piece.len()].copy_from_slice(piece);
            crate::send(to, &message[..=piece.len()], &[])
        };
        let mut outcome = Ok(());
        if listener.torn {
            outcome = send(nic::DROP, &[]);
            listener.torn = outcome.is_err();
        }
        if outcome.is_ok() {
            for (n, (op, piece)) in nic::split(frame).enumerate() {
                outcome = send(op, piece);
                if outcome.is_err() {
                    listener.torn = n > 0;
                    break;
                }
            }
        }
        match outcome {
            Ok(()) => self.stats.delivered += 1,
            Err(Error::QueueFull) => self.stats.dropped += 1,
            // Gone (or a channel that cannot be sent on): nobody
            // listens any more.
            Err(_) => {
                self.stats.dropped += 1;
                self.listeners.remove(to);
                let _ = crate::close(to);
            }
        }
    }

    /// One message with no handle: a piece of a frame to send. True if
    /// that made the frame the driver is to die in (K82, CRASH): it is
    /// not sent, and the caller dies.
    fn piece(&mut self, card: &mut impl Card, message: &[u8]) -> bool {
        let frame = match self.outgoing.feed(message) {
            Fed::Frame(whole) => {
                // The frame the driver is to die in (K82): not sent.
                if self.crash_in > 0 {
                    self.crash_in -= 1;
                    if self.crash_in == 0 {
                        return true;
                    }
                }
                whole
            }
            Fed::More => return false,
            Fed::TooLong | Fed::NotAFrame => {
                self.stats.refused += 1;
                return false;
            }
        };
        if (nic::FRAME_MIN..=nic::FRAME_MAX).contains(&frame.len()) && card.send(frame) {
            self.stats.sent += 1;
        } else {
            self.stats.refused += 1;
        }
        false
    }

    /// Answers one request that came with a reply channel; returns the
    /// reply's length in `reply`.
    fn answer(&mut self, card: &mut impl Card, request: &[u8], reply: &mut [u8]) -> usize {
        match request {
            [nic::INFO] => {
                reply[0] = nic::OK;
                reply[1..=nic::ADDRESS].copy_from_slice(&card.address());
                reply[1 + nic::ADDRESS] = u8::from(card.link_up());
                1 + nic::INFO_SIZE
            }
            [nic::STATS] => {
                reply[0] = nic::OK;
                reply[1..=Stats::SIZE].copy_from_slice(&self.stats.to_bytes());
                1 + Stats::SIZE
            }
            _ => {
                reply[0] = nic::BAD_REQUEST;
                1
            }
        }
    }
}

/// An Ethernet address, written the usual way.
pub struct Address(pub [u8; nic::ADDRESS]);

impl core::fmt::Display for Address {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        let a = self.0;
        write!(f, "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}", a[0], a[1], a[2], a[3], a[4], a[5])
    }
}

/// Serves `card` on `requests` until no clients are left (0) or
/// something fails (6). `interrupt`: the card's, if it has one - else
/// it is polled, a tick at a time. `name` is the driver's, for its log
/// lines.
pub fn serve(card: &mut impl Card, name: &str, requests: Handle, interrupt: Option<Handle>) -> i64 {
    let mut service = Service { stats: Stats::default(), listeners: Listeners::new(), outgoing: Assembler::new(), crash_in: 0 };
    let address = card.address();
    let mut message = [0u8; abi::MESSAGE_MAX as usize];
    let mut reply = [0u8; abi::MESSAGE_MAX as usize];
    loop {
        // What the card has done since the last look, first: frames in,
        // send buffers back. Before waiting again the card must have
        // been looked at *after* the interrupt was answered.
        card.receive(&mut |frame| service.arrived(frame, &address));
        card.take_sent();

        // The card, or a client - or the card alone, while it has no
        // room for another frame to send. Without an interrupt the
        // card is asked again each tick (it polls): slower, and correct.
        let full = card.transmit_full();
        let ready = match interrupt {
            Some(line) if full => crate::object_wait(&[line, line]).map(|_| Some(0)),
            Some(line) => crate::object_wait(&[line, requests]).map(Some),
            None if full => {
                crate::sleep(1);
                Ok(None)
            }
            None => crate::object_wait_until(&[requests, requests], crate::ticks() + 1),
        };
        match (ready, interrupt) {
            (Ok(Some(0)), Some(line)) => {
                // Taken, so that the next one can come; answered, so
                // that the card lets go of its line. The card is looked
                // at above.
                if crate::interrupt_wait(line).is_err() {
                    return 6;
                }
                card.interrupt_seen();
                continue;
            }
            (Ok(Some(_)), _) => {}
            (Ok(None), _) => continue,
            (Err(_), _) => return 6,
        }

        let mut handles = [0u64; abi::MESSAGE_HANDLES_MAX as usize];
        let (n, count) = match crate::receive(requests, &mut message, &mut handles) {
            Ok(received) => received,
            Err(Error::PeerClosed) => return 0, // no clients left, and none can come
            Err(error) => {
                println!("[{name}] cannot receive: {error:?}");
                return 6;
            }
        };
        let request = &message[..n];
        match (request.first().copied(), count) {
            // A piece of a frame to send: no handle, no answer.
            (_, 0) => {
                if service.piece(card, request) {
                    // The test of a driver killed mid-I/O (K82): a frame
                    // whole and not sent, frames before it perhaps still
                    // in the card's ring, no goodbye. init restarts the
                    // driver; the stack's frames meanwhile are lost, as
                    // on any wire, and TCP sends them again.
                    println!("[{name}] dying mid-send, as asked: a frame not sent");
                    // SAFETY: none - this is the test: a driver that
                    // faults mid-I/O. Address 0 is never mapped.
                    unsafe { core::ptr::read_volatile(core::ptr::null::<u8>()) };
                }
            }
            // A new listener: for the frame types it names, in place
            // of whoever listened for them. (Closing a channel is how a
            // listener is told that it is one no longer.)
            (Some(nic::LISTEN), 1) => match nic::listen_types(&request[1..]) {
                Some((types, count)) => {
                    service.listeners.add(handles[0], &types[..count], |ended| {
                        let _ = crate::close(ended);
                    });
                }
                None => {
                    let _ = crate::close(handles[0]);
                }
            },
            // A question, with the channel to answer on.
            (_, 1) => {
                // The test of a driver that has stopped: answered first.
                let mut stall = 0;
                if n == 9 && request[0] == nic::STALL {
                    stall = u64::from_le_bytes(request[1..9].try_into().unwrap_or([0; 8]));
                }
                let length = if (1..=abi::STALL_MAX).contains(&stall) {
                    reply[0] = nic::OK;
                    1
                } else if let Some(frames) = nic::crash_count(request) {
                    service.crash_in = frames;
                    println!("[{name}] asked to die in the middle of sending frame {frames} from now");
                    reply[0] = nic::OK;
                    1
                } else {
                    stall = 0;
                    service.answer(card, request, &mut reply)
                };
                let _ = crate::send(handles[0], &reply[..length], &[]);
                let _ = crate::close(handles[0]);
                if stall > 0 {
                    println!("[{name}] asked to stall: doing nothing for {stall} ticks");
                    crate::sleep(stall);
                    println!("[{name}] working again");
                }
            }
            // Anything else: not a request.
            _ => {
                for &handle in &handles[..count] {
                    let _ = crate::close(handle);
                }
            }
        }
    }
}
