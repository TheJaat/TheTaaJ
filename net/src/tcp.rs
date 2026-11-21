//! TCP: one connection, as a machine that is handed segments and the
//! time, and asked for the segments to send.
//!
//! What IPv4 gives is packets that may be lost, doubled, or arrive in
//! another order. What a program wants is a pipe: the bytes it wrote,
//! all of them, once, in order - and to be told when the other side is
//! done. TCP makes the second out of the first, with one idea: **every
//! byte has a number**.
//!
//!   - Each side picks where its numbering starts and says so in its
//!     first segment (SYN). The other side answers "the next number I
//!     expect from you is ..." (ACK). Three segments, and both know
//!     both numberings: the handshake.
//!   - Every segment after that says the number of its first byte, and
//!     acknowledges what has arrived from the other side.
//!   - What is not acknowledged in time is sent again. What arrives
//!     twice has a number already passed, and is dropped.
//!   - Each side says how many more bytes it has room for (the
//!     window), and the other sends no more than that.
//!   - "I have no more to send" (FIN) takes a number too, so it is
//!     acknowledged and resent like a byte. Each direction is closed
//!     by itself.
//!
//! The states have the names RFC 9293 gives them:
//!
//! ```text
//!   connect()                         a SYN arrives for a listener
//!       | send SYN                        | send SYN+ACK
//!   SynSent                           SynReceived
//!       | SYN+ACK arrives, send ACK       | its ACK arrives
//!       +----------> Established <--------+
//!           shutdown() |        | FIN arrives
//!             send FIN |        | (acknowledged)
//!               FinWait1        CloseWait
//!     our FIN is  |    | FIN        | shutdown(): send FIN
//!   acknowledged  |    | arrives    |
//!          FinWait2   Closing     LastAck
//!     FIN arrives |    | our FIN    | our FIN is acknowledged
//!                 |    | is acked   |
//!                TimeWait         Closed
//!                    | ten seconds
//!                 Closed
//! ```
//!
//! (TimeWait: the side that closed first waits a while before it
//! forgets the connection - its last ACK may have been lost, and then
//! the other side's FIN comes again and must be answered.)
//!
//! Two more things are here because without them it works badly
//! enough to count as not working. What arrives ahead of a gap is
//! kept, so that one lost segment costs one segment and not everything
//! sent after it. And the sender holds itself back (the congestion
//! window): it starts with a few segments, sends more as
//! acknowledgments come, and far fewer after a loss - a loss usually
//! means some router on the way is full.
//!
//! What is not here: no window larger than 65535 bytes, no timestamps,
//! no selective acknowledgments (the other side is not told *which*
//! segments past a gap arrived), no waiting to send small pieces
//! together or to acknowledge two segments with one answer.
//!
//! Nothing here knows of addresses or ports: `stack` finds the
//! connection a segment belongs to, and writes the headers.

use alloc::boxed::Box;
use alloc::vec;

use crate::wire::tcp::{Segment, ACK, FIN, PSH, RST, SYN};

/// The longest segment this side is willing to receive: what fits an
/// Ethernet frame after the IPv4 and TCP headers. Said in the SYN.
pub const MSS_OURS: u16 = 1460;
/// And what the other side takes, if its SYN did not say: every
/// machine on the Internet must accept packets of 576 bytes.
pub const MSS_DEFAULT: u16 = 536;

/// How long an acknowledgment is waited for before the first one has
/// been timed; and the shortest and longest the wait is ever set to.
pub const RTO_FIRST_MS: u64 = 1000;
pub const RTO_MIN_MS: u64 = 200;
pub const RTO_MAX_MS: u64 = 60_000;
/// How often a SYN is sent again before giving up (1 + 2 + 4 + 8
/// seconds of waiting, then the last 16), and how often data is.
pub const SYN_RETRIES: u8 = 4;
pub const DATA_RETRIES: u8 = 6;
/// How many segments may be on their way at the start, before any
/// acknowledgment has said how much the network carries (RFC 6928).
pub const FIRST_SEGMENTS: u32 = 10;
/// How many pieces that arrived ahead of a gap are kept track of.
const AHEAD: usize = 4;
/// How long TimeWait lasts.
pub const TIME_WAIT_MS: u64 = 10_000;
/// How long a connection whose program has gone may take to finish
/// closing, before it is cut off.
pub const ORPHAN_MS: u64 = 60_000;

/// `a` comes before `b`. The numbers go round - after 0xFFFFFFFF comes
/// 0 - so "before" means "less than half the circle behind": the
/// difference, read as a signed number, is negative.
fn before(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) < 0
}

fn after(a: u32, b: u32) -> bool {
    before(b, a)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    SynSent,
    SynReceived,
    Established,
    FinWait1,
    FinWait2,
    CloseWait,
    Closing,
    LastAck,
    TimeWait,
    Closed,
}

/// Why a connection closed, when it was not by both sides saying so.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Failure {
    /// The answer to the SYN was "no such port here" (a RST).
    Refused,
    /// The other side forgot the connection, or cut it off.
    Reset,
    /// Sent again and again, and never acknowledged.
    TimedOut,
    /// Nobody on the wire has the address (said by the stack).
    Unreachable,
}

/// A segment to send. The ports and the checksum are the stack's.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Out {
    pub sequence: u32,
    pub acknowledgment: u32,
    pub flags: u8,
    pub window: u16,
    pub mss: Option<u16>,
    /// How many bytes were written to the buffer `poll` was given.
    pub payload: usize,
}

/// How many bytes are kept in each direction.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Buffers {
    /// Written by the program, not yet acknowledged by the other side.
    pub send: usize,
    /// Arrived, not yet read by the program. (Also the largest window
    /// ever offered: at most 65535.)
    pub receive: usize,
}

impl Default for Buffers {
    fn default() -> Buffers {
        Buffers { send: 16 * 1024, receive: 16 * 1024 }
    }
}

/// Bytes in a circle: added at the end, taken from the start.
struct Ring {
    bytes: Box<[u8]>,
    start: usize,
    length: usize,
}

impl Ring {
    fn new(size: usize) -> Ring {
        Ring { bytes: vec![0u8; size.max(1)].into_boxed_slice(), start: 0, length: 0 }
    }

    fn free(&self) -> usize {
        self.bytes.len() - self.length
    }

    /// Adds as much of `data` as there is room for; how much.
    fn push(&mut self, data: &[u8]) -> usize {
        let count = data.len().min(self.free());
        let size = self.bytes.len();
        let end = (self.start + self.length) % size;
        let first = count.min(size - end);
        self.bytes[end..end + first].copy_from_slice(&data[..first]);
        self.bytes[..count - first].copy_from_slice(&data[first..count]);
        self.length += count;
        count
    }

    /// Copies to `out` from `offset` bytes after the start, without
    /// taking anything; how much.
    fn copy_out(&self, offset: usize, out: &mut [u8]) -> usize {
        let count = out.len().min(self.length.saturating_sub(offset));
        let size = self.bytes.len();
        let from = (self.start + offset) % size;
        let first = count.min(size - from);
        out[..first].copy_from_slice(&self.bytes[from..from + first]);
        out[first..count].copy_from_slice(&self.bytes[..count - first]);
        count
    }

    /// Writes `data` where it will be once `offset` more bytes have
    /// been added - without adding it yet. (It must fit: `offset +
    /// data.len() <= free()`.)
    fn write_ahead(&mut self, offset: usize, data: &[u8]) {
        let size = self.bytes.len();
        let at = (self.start + self.length + offset) % size;
        let first = data.len().min(size - at);
        self.bytes[at..at + first].copy_from_slice(&data[..first]);
        self.bytes[..data.len() - first].copy_from_slice(&data[first..]);
    }

    /// Adds `count` bytes that `write_ahead` put there.
    fn extend(&mut self, count: usize) {
        self.length += count.min(self.free());
    }

    fn consume(&mut self, count: usize) {
        let count = count.min(self.length);
        self.start = (self.start + count) % self.bytes.len();
        self.length -= count;
    }
}

/// A RST that is owed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Reset {
    /// This side is cutting the connection off.
    Abort,
    /// The answer to an acknowledgment of something never sent: a RST
    /// with that number, so that the other side takes it.
    Reply(u32),
}

pub struct Connection {
    state: State,
    failure: Option<Failure>,

    // Sending. The numbers, in order round the circle:
    //   iss <= unacknowledged <= next <= highest
    /// Where this side's numbering starts: the SYN's own number.
    iss: u32,
    /// The oldest number not yet acknowledged.
    unacknowledged: u32,
    /// The next number to send. After a timeout it goes back to
    /// `unacknowledged`: everything from there is sent again.
    next: u32,
    /// One past the highest number ever sent.
    highest: u32,
    /// How many bytes the other side has room for, counted from
    /// `unacknowledged` - and the segment that last said so, so that an
    /// older one arriving late does not say otherwise.
    window: u32,
    window_sequence: u32,
    window_acknowledgment: u32,
    /// What the program wrote, from `unacknowledged` on.
    send: Ring,
    /// The program has no more to send: a FIN follows the data.
    closing: bool,
    fin_acknowledged: bool,
    /// The longest segment the other side takes.
    mss: usize,

    /// How many bytes may be on their way at once, by this side's own
    /// judgment of the network (the other limit is `window`, the other
    /// side's room) - and the size above which it grows slowly.
    congestion_window: u32,
    slow_start_until: u32,

    /// How long an acknowledgment is waited for, as worked out from
    /// the smoothed time one took and how much that varies; and how
    /// many times that has been doubled since the last one came.
    rto: u64,
    backoff: u32,
    smoothed: Option<u64>,
    variation: u64,
    /// When what is unacknowledged is sent again.
    timer: Option<u64>,
    /// How many times in a row that has happened with no word from the
    /// other side.
    retries: u8,
    /// One segment being timed: the number that acknowledges it, and
    /// when it was sent.
    sample: Option<(u32, u64)>,
    /// The other side's window is shut: send one byte anyway, to hear
    /// whether it has opened.
    probe: bool,
    /// The same acknowledgment three times: the segment after it was
    /// lost, and is sent again without waiting for the timer.
    resend_one: bool,
    duplicates: u8,
    retransmitted: u32,

    // Receiving.
    /// The next number expected from the other side.
    receive_next: u32,
    receive: Ring,
    /// What has arrived ahead of a gap: from which number to which, in
    /// order, none touching another. The bytes are in `receive`
    /// already, past its end, where they will belong.
    ahead: [(u32, u32); AHEAD],
    ahead_count: usize,
    /// The other side's FIN, seen ahead of a gap: its number.
    fin_ahead: Option<u32>,
    /// One past the last number the other side was told it may send.
    offered: u32,
    /// The other side's FIN has arrived: after what is in `receive`,
    /// there is no more.
    finished: bool,
    acknowledgment_owed: bool,
    reset_owed: Option<Reset>,

    /// When the connection ends by itself: TimeWait over, or an orphan
    /// that took too long.
    deadline: Option<u64>,
    /// The program has gone: nobody will read what arrives.
    orphan: bool,
}

impl Connection {
    fn new(state: State, iss: u32, buffers: Buffers) -> Connection {
        Connection {
            state,
            failure: None,
            iss,
            unacknowledged: iss,
            next: iss,
            highest: iss,
            window: 0,
            window_sequence: 0,
            window_acknowledgment: 0,
            send: Ring::new(buffers.send),
            closing: false,
            fin_acknowledged: false,
            mss: usize::from(MSS_DEFAULT),
            congestion_window: FIRST_SEGMENTS * u32::from(MSS_DEFAULT),
            slow_start_until: u32::MAX,
            rto: RTO_FIRST_MS,
            backoff: 0,
            smoothed: None,
            variation: 0,
            timer: None,
            retries: 0,
            sample: None,
            probe: false,
            resend_one: false,
            duplicates: 0,
            retransmitted: 0,
            receive_next: 0,
            receive: Ring::new(buffers.receive.min(usize::from(u16::MAX))),
            ahead: [(0, 0); AHEAD],
            ahead_count: 0,
            fin_ahead: None,
            offered: 0,
            finished: false,
            acknowledgment_owed: false,
            reset_owed: None,
            deadline: None,
            orphan: false,
        }
    }

    /// A connection this side opens: `poll` will give its SYN. `iss`
    /// is where its numbering starts - a number an outsider cannot
    /// guess, or anyone could cut a connection off with a made-up RST.
    pub fn connect(iss: u32, buffers: Buffers) -> Connection {
        Connection::new(State::SynSent, iss, buffers)
    }

    /// A connection the other side opened with `syn`: `poll` will give
    /// the SYN+ACK.
    pub fn accept(iss: u32, syn: &Segment<'_>, buffers: Buffers) -> Connection {
        let mut connection = Connection::new(State::SynReceived, iss, buffers);
        connection.heard_syn(syn);
        connection
    }

    /// What a SYN says: where the other side's numbering starts, and
    /// how long a segment it takes.
    fn heard_syn(&mut self, syn: &Segment<'_>) {
        self.receive_next = syn.sequence.wrapping_add(1);
        self.offered = self.receive_next;
        self.mss = usize::from(syn.mss.unwrap_or(MSS_DEFAULT).clamp(1, MSS_OURS));
        self.congestion_window = FIRST_SEGMENTS * self.mss as u32;
        self.window = u32::from(syn.window);
        self.window_sequence = syn.sequence;
        self.window_acknowledgment = syn.acknowledgment;
    }

    pub fn state(&self) -> State {
        self.state
    }

    pub fn failure(&self) -> Option<Failure> {
        self.failure
    }

    /// How many bytes have arrived and wait to be read.
    pub fn readable(&self) -> usize {
        self.receive.length
    }

    /// The other side has said it is done: once `readable` is 0, there
    /// is no more to read.
    pub fn finished(&self) -> bool {
        self.finished
    }

    /// How many bytes `write` would take now.
    pub fn writable(&self) -> usize {
        if matches!(self.state, State::Established | State::CloseWait) && !self.closing { self.send.free() } else { 0 }
    }

    /// How many bytes were written and are not yet acknowledged.
    pub fn unsent(&self) -> usize {
        self.send.length
    }

    /// How many segments were sent a second time.
    pub fn retransmitted(&self) -> u32 {
        self.retransmitted
    }

    /// When `tick` next has something to do, if ever.
    pub fn next_timer(&self) -> Option<u64> {
        match (self.timer, self.deadline) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// Takes as much of `data` as there is room for, to be sent; how
    /// much. Nothing before the connection is made, or after
    /// `shutdown`.
    pub fn write(&mut self, data: &[u8]) -> usize {
        if self.writable() == 0 {
            return 0;
        }
        self.send.push(data)
    }

    /// Copies what has arrived to `out` without taking it; how much.
    pub fn peek(&self, out: &mut [u8]) -> usize {
        self.receive.copy_out(0, out)
    }

    /// Takes `count` bytes that `peek` showed: there is room for that
    /// many more, and the other side is told when it is worth a
    /// segment to say so.
    pub fn consume(&mut self, count: usize) {
        self.receive.consume(count);
        // Worth it: a whole segment more than it knows of, or half the
        // buffer. (Saying so for every byte read would have the other
        // side send segments of one byte: the "silly window".)
        let grown = self.receive_next.wrapping_add(self.window_offer()).wrapping_sub(self.offered);
        if self.state != State::Closed && grown as usize >= usize::from(MSS_OURS).min(self.receive.bytes.len() / 2) {
            self.acknowledgment_owed = true;
        }
    }

    /// `peek` and `consume` in one.
    pub fn read(&mut self, out: &mut [u8]) -> usize {
        let count = self.peek(out);
        self.consume(count);
        count
    }

    /// The program has no more to send: after what it wrote, a FIN.
    /// The other direction stays open.
    pub fn shutdown(&mut self) {
        if self.closing {
            return;
        }
        match self.state {
            // Nothing was ever agreed: there is nothing to finish.
            State::SynSent => self.end(),
            State::SynReceived => self.closing = true, // FinWait1 when the handshake is done
            State::Established => {
                self.closing = true;
                self.state = State::FinWait1;
            }
            State::CloseWait => {
                self.closing = true;
                self.state = State::LastAck;
            }
            _ => {}
        }
    }

    /// The program has gone. What it wrote is still sent, and the
    /// connection closed in order - but nobody will read: if something
    /// is waiting to be read, or arrives later, the other side is told
    /// with a RST that it went nowhere.
    pub fn close(&mut self, now: u64) {
        if self.state == State::Closed {
            return;
        }
        if self.receive.length > 0 {
            return self.abort();
        }
        self.orphan = true;
        self.shutdown();
        if self.state != State::Closed && self.deadline.is_none() {
            self.deadline = Some(now + ORPHAN_MS);
        }
    }

    /// Cuts the connection off: a RST, and nothing more is sent or
    /// taken.
    pub fn abort(&mut self) {
        if !matches!(self.state, State::Closed | State::SynSent) {
            self.reset_owed = Some(Reset::Abort);
        }
        self.end();
    }

    /// The stack's: nobody on the wire has the other side's address.
    pub fn unreachable(&mut self) {
        if self.state == State::SynSent {
            self.fail(Failure::Unreachable);
        }
    }

    fn end(&mut self) {
        self.state = State::Closed;
        self.timer = None;
        self.deadline = None;
        self.acknowledgment_owed = false;
    }

    fn fail(&mut self, failure: Failure) {
        self.failure = Some(failure);
        self.end();
    }

    /// How long an acknowledgment is waited for now.
    fn wait(&self) -> u64 {
        (self.rto << self.backoff.min(16)).min(RTO_MAX_MS)
    }

    /// How many bytes the other side may be told there is room for.
    fn window_offer(&self) -> u32 {
        self.receive.free().min(usize::from(u16::MAX)) as u32
    }

    /// One past the last byte written: the number the FIN takes.
    fn end_of_data(&self) -> u32 {
        self.unacknowledged.wrapping_add(self.send.length as u32)
    }

    /// A segment with the acknowledgment and the window as they are
    /// now - which settles what was owed.
    fn out(&mut self, sequence: u32, flags: u8, payload: usize) -> Out {
        let window = self.window_offer();
        self.offered = self.receive_next.wrapping_add(window);
        self.acknowledgment_owed = false;
        Out { sequence, acknowledgment: self.receive_next, flags, window: window as u16, mss: None, payload }
    }

    /// Notes that numbers up to `end` are on their way: the timer runs
    /// if it was not running, and the segment is timed if none is
    /// being - and if it was not sent before (an acknowledgment of
    /// something sent twice says nothing of how long either took).
    fn sent(&mut self, end: u32, now: u64) {
        self.next = end;
        if after(end, self.highest) {
            self.highest = end;
            if self.sample.is_none() {
                self.sample = Some((end, now));
            }
        }
        if self.timer.is_none() {
            self.timer = Some(now + self.wait());
        }
    }

    /// The next segment to send, if there is one. Its data is written
    /// to the start of `payload` (which should have room for
    /// `MSS_OURS` bytes; less means shorter segments). To be called
    /// until it gives None, after anything that may have changed what
    /// there is to send: a segment arrived, the program wrote or read,
    /// the clock.
    pub fn poll(&mut self, now: u64, payload: &mut [u8]) -> Option<Out> {
        match self.reset_owed.take() {
            Some(Reset::Abort) => {
                return Some(Out { sequence: self.highest, acknowledgment: self.receive_next, flags: RST | ACK, window: 0, mss: None, payload: 0 });
            }
            Some(Reset::Reply(sequence)) => {
                return Some(Out { sequence, acknowledgment: 0, flags: RST, window: 0, mss: None, payload: 0 });
            }
            None => {}
        }
        match self.state {
            State::Closed => return None,
            State::SynSent | State::SynReceived if self.next == self.iss => {
                if self.highest != self.iss {
                    self.retransmitted += 1;
                }
                self.sent(self.iss.wrapping_add(1), now);
                let window = self.window_offer();
                self.offered = self.receive_next.wrapping_add(window);
                self.acknowledgment_owed = false;
                let (flags, acknowledgment) = if self.state == State::SynSent { (SYN, 0) } else { (SYN | ACK, self.receive_next) };
                return Some(Out { sequence: self.iss, acknowledgment, flags, window: window as u16, mss: Some(MSS_OURS), payload: 0 });
            }
            State::SynSent => return None,
            State::SynReceived => {}
            _ => {
                if let Some(out) = self.poll_data(now, payload) {
                    return Some(out);
                }
            }
        }
        if self.acknowledgment_owed {
            return Some(self.out(self.highest, ACK, 0));
        }
        None
    }

    /// Data, or the FIN after it.
    fn poll_data(&mut self, now: u64, payload: &mut [u8]) -> Option<Out> {
        let end = self.end_of_data();
        let fin_sent = self.closing && self.highest == end.wrapping_add(1);
        if self.resend_one {
            self.resend_one = false;
            self.sample = None;
            // Only what was sent before, and no more than the other
            // side has room for.
            let sent_before = self.highest.wrapping_sub(self.unacknowledged) as usize;
            let count = self.send.length.min(self.mss).min(payload.len()).min(sent_before).min(self.window as usize);
            if count > 0 {
                self.send.copy_out(0, &mut payload[..count]);
                self.retransmitted += 1;
                return Some(self.out(self.unacknowledged, ACK, count));
            }
            if fin_sent && !self.fin_acknowledged {
                self.retransmitted += 1;
                return Some(self.out(end, FIN | ACK, 0));
            }
        }
        // Negative when the FIN has gone out: `next` is one past it.
        let pending = end.wrapping_sub(self.next) as i32;
        if pending > 0 {
            let in_flight = self.next.wrapping_sub(self.unacknowledged);
            let mut room = self.window.saturating_sub(in_flight);
            if room == 0 && self.probe {
                room = 1;
            }
            let count = (pending as usize).min(self.mss).min(payload.len()).min(room as usize);
            // This side's own limit is counted in whole segments: when
            // what is left of it is less than the segment there is to
            // send, that waits - unless nothing at all is on its way.
            let held_back = in_flight > 0 && in_flight as usize + count > self.congestion_window as usize;
            if count > 0 && !held_back {
                self.probe = false;
                self.send.copy_out(in_flight as usize, &mut payload[..count]);
                let sequence = self.next;
                if before(sequence, self.highest) {
                    self.retransmitted += 1;
                }
                self.sent(sequence.wrapping_add(count as u32), now);
                // "Push": nothing more is coming for now.
                let flags = if self.next == end { ACK | PSH } else { ACK };
                return Some(self.out(sequence, flags, count));
            }
            // The other side has no room, and nothing is on its way
            // that it would answer: the timer must run, or a lost
            // "there is room again" would be waited for for ever.
            if in_flight == 0 && self.timer.is_none() {
                self.timer = Some(now + self.wait());
            }
        } else if pending == 0 && self.closing && !self.fin_acknowledged {
            if before(self.next, self.highest) {
                self.retransmitted += 1;
            }
            self.sent(end.wrapping_add(1), now);
            return Some(self.out(end, FIN | ACK, 0));
        }
        None
    }

    /// The clock.
    pub fn tick(&mut self, now: u64) {
        if self.deadline.is_some_and(|deadline| now >= deadline) {
            if self.state == State::TimeWait {
                self.end();
            } else {
                self.abort();
            }
            return;
        }
        if self.timer.is_some_and(|timer| now >= timer) {
            self.timer = None;
            let limit = if matches!(self.state, State::SynSent | State::SynReceived) { SYN_RETRIES } else { DATA_RETRIES };
            if self.retries >= limit {
                // Tell the other side, in case it is there after all
                // and only its answers are lost.
                if self.state != State::SynSent {
                    self.reset_owed = Some(Reset::Abort);
                }
                return self.fail(Failure::TimedOut);
            }
            self.retries += 1;
            // Wait twice as long next time, and start again with one
            // segment at a time: if the network is losing packets
            // because it is full, sending as before makes it worse for
            // everyone. (Not for a shut window: that is the other
            // side's program being slow, not the network.)
            self.backoff += 1;
            self.sample = None;
            self.duplicates = 0;
            if self.window == 0 {
                self.probe = true;
            } else {
                self.slow_start_until = self.half_of_what_was_on_its_way();
                self.congestion_window = self.mss as u32;
            }
            // From the oldest unacknowledged number on (`poll` starts
            // the timer as it sends).
            self.next = self.unacknowledged;
        }
    }

    /// What the congestion window is cut to after a loss: half of what
    /// was on its way when it happened, and never less than two
    /// segments.
    fn half_of_what_was_on_its_way(&self) -> u32 {
        (self.highest.wrapping_sub(self.unacknowledged) / 2).max(2 * self.mss as u32)
    }

    /// One acknowledgment was timed: `took` ms. The wait is set from
    /// the average and from how much the times vary (RFC 6298) - long
    /// enough that a slow answer is not taken for a lost one, short
    /// enough that a lost one is noticed.
    fn measured(&mut self, took: u64) {
        match self.smoothed {
            None => {
                self.smoothed = Some(took);
                self.variation = took / 2;
            }
            Some(smoothed) => {
                self.variation = (3 * self.variation + smoothed.abs_diff(took)) / 4;
                self.smoothed = Some((7 * smoothed + took) / 8);
            }
        }
        let smoothed = self.smoothed.unwrap_or(took);
        self.rto = (smoothed + (4 * self.variation).max(10)).clamp(RTO_MIN_MS, RTO_MAX_MS);
    }

    /// A segment arrived for this connection.
    pub fn segment(&mut self, segment: &Segment<'_>, now: u64) {
        match self.state {
            State::Closed => {}
            State::SynSent => self.segment_while_opening(segment, now),
            _ => self.segment_synchronized(segment, now),
        }
    }

    /// In SynSent: only the answer to the SYN is of interest.
    fn segment_while_opening(&mut self, segment: &Segment<'_>, now: u64) {
        let acknowledges = segment.has(ACK).then_some(segment.acknowledgment);
        if let Some(acknowledged) = acknowledges {
            // An acknowledgment of anything but the SYN is of some
            // older connection: the other side is told to forget it.
            if acknowledged != self.iss.wrapping_add(1) || self.next == self.iss {
                if !segment.has(RST) {
                    self.reset_owed = Some(Reset::Reply(acknowledged));
                }
                return;
            }
        }
        if segment.has(RST) {
            // Believed only if it names the SYN: anyone can send a RST.
            if acknowledges.is_some() {
                self.fail(Failure::Refused);
            }
            return;
        }
        if !segment.has(SYN) {
            return;
        }
        self.heard_syn(segment);
        self.acknowledgment_owed = true;
        if acknowledges.is_some() {
            self.acknowledged(self.iss.wrapping_add(1), now);
            self.state = State::Established;
        } else {
            // Both sides opened at once, each with a SYN: this one's is
            // sent again, now as a SYN+ACK.
            self.state = State::SynReceived;
            self.next = self.iss;
            self.timer = None;
        }
    }

    /// The other side acknowledged everything before `acknowledged`,
    /// which is more than it had.
    fn acknowledged(&mut self, acknowledged: u32, now: u64) {
        let count = acknowledged.wrapping_sub(self.unacknowledged) as usize;
        // The SYN, when it is what is acknowledged, is not in `send`;
        // nor is the FIN.
        let syn = usize::from(self.unacknowledged == self.iss);
        let data = (count - syn).min(self.send.length);
        self.send.consume(data);
        if self.closing && count - syn > data {
            self.fin_acknowledged = true;
        }
        self.unacknowledged = acknowledged;
        if before(self.next, acknowledged) {
            self.next = acknowledged;
        }
        if let Some((sample, at)) = self.sample {
            if !before(acknowledged, sample) {
                self.sample = None;
                self.measured(now.saturating_sub(at));
            }
        }
        // An answer came: the wait is the worked-out one again, and
        // more may be sent - twice as much with every round of
        // acknowledgments while that is still little ("slow start",
        // which is the fast part), one segment more per round after.
        self.backoff = 0;
        let mss = self.mss as u32;
        let more = if data == 0 {
            0
        } else if self.congestion_window < self.slow_start_until {
            (data as u32).min(mss)
        } else {
            (mss * mss / self.congestion_window).max(1)
        };
        self.congestion_window = (self.congestion_window + more).min(1 << 20);
        self.duplicates = 0;
        self.resend_one = false;
        self.probe = false;
        self.timer = if acknowledged == self.highest { None } else { Some(now + self.wait()) };
    }

    fn segment_synchronized(&mut self, segment: &Segment<'_>, now: u64) {
        // 1. Is any of it within what this side said it has room for?
        //    If not it is old, or doubled - or not of this connection
        //    at all. The answer is where this side really is.
        let expected = self.receive_next;
        let window = self.window_offer();
        let within = |number: u32| !before(number, expected) && before(number, expected.wrapping_add(window));
        let length = segment.length();
        let acceptable = match (length, window) {
            (0, 0) => segment.sequence == expected,
            (0, _) => within(segment.sequence),
            (_, 0) => false,
            _ => within(segment.sequence) || within(segment.sequence.wrapping_add(length - 1)),
        };
        if !acceptable {
            if !segment.has(RST) {
                self.acknowledgment_owed = true;
            }
            match self.state {
                // The other side's FIN again: the ACK for it was lost.
                State::TimeWait => self.deadline = Some(now + TIME_WAIT_MS),
                // Both sides opened at once, and this is the other's
                // SYN again, now acknowledging this side's: agreed.
                State::SynReceived
                    if segment.has(SYN)
                        && segment.has(ACK)
                        && segment.sequence.wrapping_add(1) == expected
                        && segment.acknowledgment == self.iss.wrapping_add(1) =>
                {
                    self.acknowledged(segment.acknowledgment, now);
                    self.state = if self.closing { State::FinWait1 } else { State::Established };
                    self.window = u32::from(segment.window);
                }
                // Its SYN again: the SYN+ACK was lost. Sent again now.
                State::SynReceived if segment.has(SYN) && segment.sequence.wrapping_add(1) == expected => {
                    self.next = self.iss;
                    self.timer = None;
                }
                _ => {}
            }
            return;
        }

        // 2. "Forget this connection." Believed only with exactly the
        //    number expected next; one that is merely near is answered
        //    with where this side is - if the other side really did
        //    forget, its answer to *that* will be a RST with the right
        //    number (RFC 5961).
        if segment.has(RST) {
            if segment.sequence == expected {
                self.fail(if self.state == State::SynReceived { Failure::Refused } else { Failure::Reset });
            } else {
                self.acknowledgment_owed = true;
            }
            return;
        }
        // 3. A SYN in the middle of a connection: the same answer.
        if segment.has(SYN) {
            self.acknowledgment_owed = true;
            return;
        }
        // 4. What it acknowledges.
        if !segment.has(ACK) {
            return;
        }
        let acknowledged = segment.acknowledgment;
        if self.state == State::SynReceived {
            if acknowledged != self.iss.wrapping_add(1) {
                self.reset_owed = Some(Reset::Reply(acknowledged));
                return;
            }
            self.acknowledged(acknowledged, now);
            self.state = if self.closing { State::FinWait1 } else { State::Established };
            self.window = u32::from(segment.window);
            self.window_sequence = segment.sequence;
            self.window_acknowledgment = acknowledged;
        }
        if after(acknowledged, self.highest) {
            // Something never sent.
            self.acknowledgment_owed = true;
            return;
        }
        if !before(acknowledged, self.unacknowledged) {
            // The other side is there.
            self.retries = 0;
            let window = u32::from(segment.window);
            if after(acknowledged, self.unacknowledged) {
                self.acknowledged(acknowledged, now);
            } else if length == 0 && window == self.window && window > 0 && self.unacknowledged != self.highest {
                // The same acknowledgment again, for nothing: it has
                // received something *after* a gap. Three times, and
                // what should fill the gap is taken to be lost.
                self.duplicates = self.duplicates.saturating_add(1);
                if self.duplicates == 3 {
                    self.resend_one = true;
                    self.slow_start_until = self.half_of_what_was_on_its_way();
                    self.congestion_window = self.slow_start_until;
                }
            }
            // The window, unless this segment is older than the one
            // that last said.
            if before(self.window_sequence, segment.sequence)
                || (self.window_sequence == segment.sequence && !after(self.window_acknowledgment, acknowledged))
            {
                // Open again, with nothing on its way: no need to wait
                // for the probe's turn.
                if self.window == 0 && window > 0 && self.next == self.unacknowledged {
                    self.timer = None;
                    self.probe = false;
                }
                self.window = window;
                self.window_sequence = segment.sequence;
                self.window_acknowledgment = acknowledged;
            }
        }
        if self.fin_acknowledged {
            match self.state {
                State::FinWait1 => self.state = State::FinWait2,
                State::Closing => {
                    self.state = State::TimeWait;
                    self.deadline = Some(now + TIME_WAIT_MS);
                }
                State::LastAck => return self.end(),
                _ => {}
            }
        }

        // 5. What it carries. The part already received is stepped
        //    over.
        let old = expected.wrapping_sub(segment.sequence) as i32;
        let payload = if old > 0 { &segment.payload[(old as usize).min(segment.payload.len())..] } else { segment.payload };
        if payload.is_empty() && !segment.has(FIN) {
            return;
        }
        self.acknowledgment_owed = true;
        if !matches!(self.state, State::Established | State::FinWait1 | State::FinWait2) {
            return;
        }
        if self.orphan && !payload.is_empty() {
            // Nobody is left to read it.
            return self.abort();
        }
        if old < 0 {
            // It starts after a gap: kept, where it will belong. The
            // answer says what is still expected - the same number as
            // before, which is how the sender learns of the gap.
            return self.keep_ahead(segment.sequence, payload, segment.has(FIN));
        }
        let taken = self.receive.push(payload);
        self.receive_next = self.receive_next.wrapping_add(taken as u32);
        let mut finished = segment.has(FIN) && taken == payload.len();
        // Did that fill a gap? Then what was kept past it has arrived
        // too.
        while self.ahead_count > 0 && !after(self.ahead[0].0, self.receive_next) {
            let (_, end) = self.ahead[0];
            if after(end, self.receive_next) {
                self.receive.extend(end.wrapping_sub(self.receive_next) as usize);
                self.receive_next = end;
            }
            self.ahead.copy_within(1..self.ahead_count, 0);
            self.ahead_count -= 1;
        }
        if self.fin_ahead == Some(self.receive_next) {
            finished = true;
        }
        // 6. "No more from me" - if everything before it has arrived.
        if finished {
            self.fin_ahead = None;
            self.ahead_count = 0;
            self.receive_next = self.receive_next.wrapping_add(1);
            self.finished = true;
            match self.state {
                State::Established => self.state = State::CloseWait,
                State::FinWait1 => self.state = State::Closing,
                State::FinWait2 => {
                    self.state = State::TimeWait;
                    self.timer = None;
                    self.deadline = Some(now + TIME_WAIT_MS);
                }
                _ => {}
            }
        }
    }

    /// Keeps `payload`, which starts at `sequence` - after a gap.
    fn keep_ahead(&mut self, sequence: u32, payload: &[u8], fin: bool) {
        let offset = sequence.wrapping_sub(self.receive_next) as usize;
        let count = payload.len().min(self.receive.free().saturating_sub(offset));
        if fin && count == payload.len() {
            self.fin_ahead = Some(sequence.wrapping_add(count as u32));
        }
        if count == 0 {
            return;
        }
        // The list of what is kept, with this piece in its place -
        // joined with every piece it touches or overlaps. If that
        // makes one piece too many, it is not kept: it will be sent
        // again.
        let mut piece = (sequence, sequence.wrapping_add(count as u32));
        let mut list = [(0, 0); AHEAD + 1];
        let mut length = 0;
        let mut placed = false;
        for &(from, to) in &self.ahead[..self.ahead_count] {
            if before(to, piece.0) {
                list[length] = (from, to);
                length += 1;
            } else if after(from, piece.1) {
                if !placed {
                    list[length] = piece;
                    length += 1;
                    placed = true;
                }
                list[length] = (from, to);
                length += 1;
            } else {
                if before(from, piece.0) {
                    piece.0 = from;
                }
                if after(to, piece.1) {
                    piece.1 = to;
                }
            }
        }
        if !placed {
            list[length] = piece;
            length += 1;
        }
        if length > AHEAD {
            return;
        }
        self.ahead.copy_from_slice(&list[..AHEAD]);
        self.ahead_count = length;
        self.receive.write_ahead(offset, &payload[..count]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A segment as one side gave it, kept: what the wire carries.
    #[derive(Clone, Debug, PartialEq)]
    struct Sent {
        out: Out,
        data: Vec<u8>,
    }

    impl Sent {
        fn segment(&self) -> Segment<'_> {
            Segment {
                source_port: 1,
                destination_port: 2,
                sequence: self.out.sequence,
                acknowledgment: self.out.acknowledgment,
                flags: self.out.flags,
                window: self.out.window,
                mss: self.out.mss,
                payload: &self.data,
            }
        }

        fn has(&self, flag: u8) -> bool {
            self.out.flags & flag != 0
        }
    }

    /// A segment made by hand: what some other machine might send.
    fn made(sequence: u32, acknowledgment: u32, flags: u8, window: u16, data: &[u8]) -> Sent {
        Sent { out: Out { sequence, acknowledgment, flags, window, mss: None, payload: data.len() }, data: data.to_vec() }
    }

    /// Everything `connection` has to send now.
    fn drain(connection: &mut Connection, now: u64) -> Vec<Sent> {
        let mut sent = Vec::new();
        let mut payload = [0u8; MSS_OURS as usize];
        while let Some(out) = connection.poll(now, &mut payload) {
            sent.push(Sent { out, data: payload[..out.payload].to_vec() });
            assert!(sent.len() < 1000, "a connection that never stops sending");
        }
        sent
    }

    fn deliver(to: &mut Connection, sent: &[Sent], now: u64) {
        for one in sent {
            to.segment(&one.segment(), now);
        }
    }

    /// Passes segments back and forth until neither side has more.
    fn settle(a: &mut Connection, b: &mut Connection, now: u64) {
        for _ in 0..1000 {
            let from_a = drain(a, now);
            deliver(b, &from_a, now);
            let from_b = drain(b, now);
            deliver(a, &from_b, now);
            if from_a.is_empty() && from_b.is_empty() {
                return;
            }
        }
        panic!("two connections that never stop talking");
    }

    const A: u32 = 1000;
    const B: u32 = 5000;

    /// Two sides of a connection that has been made: the first opened
    /// it. Their numberings start at `a` and `b`.
    fn pair_from(a: u32, b: u32, buffers_a: Buffers, buffers_b: Buffers) -> (Connection, Connection) {
        let mut opener = Connection::connect(a, buffers_a);
        let syn = drain(&mut opener, 0);
        let mut listener = Connection::accept(b, &syn[0].segment(), buffers_b);
        settle(&mut opener, &mut listener, 0);
        assert_eq!((opener.state(), listener.state()), (State::Established, State::Established));
        (opener, listener)
    }

    fn pair() -> (Connection, Connection) {
        pair_from(A, B, Buffers::default(), Buffers::default())
    }

    fn pattern(length: usize, salt: u8) -> Vec<u8> {
        (0..length).map(|n| (n as u8).wrapping_mul(31).wrapping_add(salt).wrapping_add((n >> 8) as u8)).collect()
    }

    fn read_all(connection: &mut Connection) -> Vec<u8> {
        let mut all = Vec::new();
        let mut piece = [0u8; 700];
        loop {
            let count = connection.read(&mut piece);
            if count == 0 {
                return all;
            }
            all.extend_from_slice(&piece[..count]);
        }
    }

    #[test]
    fn numbers_go_round() {
        assert!(before(1, 2) && !before(2, 1) && !before(7, 7));
        assert!(before(0xFFFF_FFF0, 3), "after the highest number comes 0");
        assert!(after(3, 0xFFFF_FFF0));
        // Halfway round, where a signed number turns from the largest
        // to the smallest: nothing special about it here.
        assert!(before(0x7FFF_FFF0, 0x8000_0010) && after(0x8000_0010, 0x7FFF_FFF0));
        assert!(before(0x8000_0010, 0x8000_0020) && before(0x7FFF_0000, 0x7FFF_0001));
    }

    #[test]
    fn a_ring_keeps_bytes_in_order_round_its_end() {
        let mut ring = Ring::new(8);
        assert_eq!(ring.push(b"abcdef"), 6);
        ring.consume(4);
        assert_eq!(ring.push(b"ghijklmnop"), 6, "only what there is room for");
        assert_eq!(ring.free(), 0);
        let mut out = [0u8; 16];
        assert_eq!(ring.copy_out(0, &mut out), 8);
        assert_eq!(&out[..8], b"efghijkl");
        assert_eq!(ring.copy_out(5, &mut out), 3);
        assert_eq!(&out[..3], b"jkl");
        assert_eq!(ring.copy_out(9, &mut out), 0);
        ring.consume(100);
        assert_eq!((ring.length, ring.free()), (0, 8));
    }

    #[test]
    fn three_segments_make_a_connection() {
        let mut opener = Connection::connect(A, Buffers::default());
        assert_eq!(opener.write(b"too early"), 0, "nothing is taken before the connection is made");
        let syn = drain(&mut opener, 0);
        assert_eq!(syn, [Sent { out: Out { sequence: A, acknowledgment: 0, flags: SYN, window: 16384, mss: Some(1460), payload: 0 }, data: vec![] }]);
        assert_eq!(opener.state(), State::SynSent);
        assert_eq!(opener.next_timer(), Some(RTO_FIRST_MS));

        let mut listener = Connection::accept(B, &syn[0].segment(), Buffers::default());
        let answer = drain(&mut listener, 0);
        assert_eq!(answer.len(), 1);
        assert_eq!(answer[0].out, Out { sequence: B, acknowledgment: A + 1, flags: SYN | ACK, window: 16384, mss: Some(1460), payload: 0 });
        assert_eq!(listener.state(), State::SynReceived);

        deliver(&mut opener, &answer, 30);
        assert_eq!(opener.state(), State::Established);
        let third = drain(&mut opener, 30);
        assert_eq!(third.len(), 1);
        assert_eq!(third[0].out, Out { sequence: A + 1, acknowledgment: B + 1, flags: ACK, window: 16384, mss: None, payload: 0 });
        deliver(&mut listener, &third, 60);
        assert_eq!(listener.state(), State::Established);
        // And then both are silent, with no clock running.
        assert!(drain(&mut opener, 60).is_empty() && drain(&mut listener, 60).is_empty());
        assert_eq!((opener.next_timer(), listener.next_timer()), (None, None));
        assert_eq!((opener.failure(), listener.failure()), (None, None));
    }

    #[test]
    fn bytes_cross_and_are_acknowledged() {
        let (mut a, mut b) = pair();
        assert_eq!(a.write(b"hello"), 5);
        let sent = drain(&mut a, 100);
        assert_eq!(sent.len(), 1);
        assert_eq!((sent[0].out.sequence, sent[0].out.flags, &sent[0].data[..]), (A + 1, ACK | PSH, &b"hello"[..]));
        assert_eq!(a.unsent(), 5, "kept until it is acknowledged");
        deliver(&mut b, &sent, 100);
        assert_eq!(b.readable(), 5);
        let acknowledgment = drain(&mut b, 100);
        assert_eq!(acknowledgment.len(), 1);
        assert_eq!((acknowledgment[0].out.acknowledgment, acknowledgment[0].out.flags, acknowledgment[0].out.window), (A + 6, ACK, 16384 - 5));
        deliver(&mut a, &acknowledgment, 100);
        assert_eq!((a.unsent(), a.next_timer()), (0, None));
        assert_eq!(read_all(&mut b), b"hello");
        // The other way, in the same connection.
        assert_eq!(b.write(b"and back"), 8);
        settle(&mut a, &mut b, 200);
        assert_eq!(read_all(&mut a), b"and back");
    }

    #[test]
    fn what_is_written_goes_out_in_segments_the_other_side_takes() {
        let (mut a, mut b) = pair();
        let data = pattern(5000, 1);
        assert_eq!(a.write(&data), 5000);
        let sent = drain(&mut a, 0);
        assert_eq!(sent.iter().map(|one| one.data.len()).collect::<Vec<_>>(), [1460, 1460, 1460, 620]);
        assert_eq!(sent.iter().map(|one| one.has(PSH)).collect::<Vec<_>>(), [false, false, false, true]);
        deliver(&mut b, &sent, 0);
        assert_eq!(read_all(&mut b), data);

        // A side whose SYN named no size is sent 536 bytes at a time;
        // one that named a small size, that.
        for (said, takes) in [(None, 536), (Some(100), 100), (Some(9000), 1460)] {
            let mut syn = made(B, 0, SYN, 8000, &[]);
            syn.out.mss = said;
            let mut c = Connection::accept(A, &syn.segment(), Buffers::default());
            drain(&mut c, 0);
            c.segment(&made(B + 1, A + 1, ACK, 8000, &[]).segment(), 0);
            c.write(&data);
            assert_eq!(drain(&mut c, 0)[0].data.len(), takes);
        }
    }

    #[test]
    fn what_is_not_acknowledged_is_sent_again_and_waited_for_twice_as_long() {
        let (mut a, mut b) = pair();
        a.write(b"lost");
        let first = drain(&mut a, 0);
        // (The handshake was timed at 0 ms: the wait is the shortest.)
        assert_eq!(a.next_timer(), Some(RTO_MIN_MS));
        a.tick(RTO_MIN_MS - 1);
        assert!(drain(&mut a, RTO_MIN_MS - 1).is_empty(), "not before its time");
        a.tick(RTO_MIN_MS);
        let second = drain(&mut a, RTO_MIN_MS);
        assert_eq!(second, first, "the same bytes with the same numbers");
        assert_eq!(a.retransmitted(), 1);
        assert_eq!(a.next_timer(), Some(RTO_MIN_MS + 2 * RTO_MIN_MS));
        a.tick(3 * RTO_MIN_MS);
        let third = drain(&mut a, 3 * RTO_MIN_MS);
        assert_eq!(a.next_timer(), Some(3 * RTO_MIN_MS + 4 * RTO_MIN_MS));
        // The third copy arrives - and then the first, late.
        deliver(&mut b, &third, 700);
        deliver(&mut b, &first, 700);
        settle(&mut a, &mut b, 700);
        assert_eq!(read_all(&mut b), b"lost", "once");
        assert_eq!((a.unsent(), a.next_timer(), a.failure()), (0, None, None));
    }

    #[test]
    fn a_side_that_never_answers_is_given_up_on() {
        let (mut a, _) = pair();
        a.write(b"anyone?");
        let mut copies = 0;
        let mut now = 0;
        while a.state() != State::Closed {
            let sent = drain(&mut a, now);
            copies += sent.iter().filter(|one| !one.data.is_empty()).count();
            if a.state() == State::Closed {
                // The last word: a RST, in case the other side is there.
                assert!(sent.iter().any(|one| one.has(RST)));
                break;
            }
            now = a.next_timer().expect("a clock runs while something is unacknowledged");
            a.tick(now);
            assert!(now < 1_000_000);
        }
        assert_eq!(copies, 1 + usize::from(DATA_RETRIES));
        assert_eq!(a.failure(), Some(Failure::TimedOut));
        // 200 ms, doubled each time: 200 * (2^7 - 1).
        assert_eq!(now, RTO_MIN_MS * 127);
        assert_eq!(a.write(b"more"), 0);
        assert_eq!(a.next_timer(), None);
    }

    #[test]
    fn a_syn_nobody_answers_is_given_up_on_after_half_a_minute() {
        let mut opener = Connection::connect(A, Buffers::default());
        let mut syns = 0;
        let mut now = 0;
        loop {
            let sent = drain(&mut opener, now);
            assert!(sent.iter().all(|one| one.out.flags == SYN && one.out.sequence == A));
            syns += sent.len();
            let Some(next) = opener.next_timer() else { break };
            now = next;
            opener.tick(now);
        }
        assert_eq!((syns, now), (5, 31_000));
        assert_eq!((opener.state(), opener.failure()), (State::Closed, Some(Failure::TimedOut)));
    }

    #[test]
    fn no_such_port_is_an_answer_too() {
        let mut opener = Connection::connect(A, Buffers::default());
        drain(&mut opener, 0);
        // A RST that does not name the SYN could be anyone's.
        opener.segment(&made(0, A + 77, RST | ACK, 0, &[]).segment(), 5);
        opener.segment(&made(0, 0, RST, 0, &[]).segment(), 5);
        assert_eq!(opener.state(), State::SynSent);
        assert!(drain(&mut opener, 5).is_empty(), "a RST is never answered");
        opener.segment(&made(0, A + 1, RST | ACK, 0, &[]).segment(), 5);
        assert_eq!((opener.state(), opener.failure()), (State::Closed, Some(Failure::Refused)));
        assert!(drain(&mut opener, 5).is_empty());
    }

    #[test]
    fn an_answer_to_some_older_connection_is_told_to_go_away() {
        let mut opener = Connection::connect(A, Buffers::default());
        drain(&mut opener, 0);
        // A SYN+ACK acknowledging a number this side never used.
        opener.segment(&made(B, 777, SYN | ACK, 100, &[]).segment(), 5);
        assert_eq!(opener.state(), State::SynSent);
        let reply = drain(&mut opener, 5);
        assert_eq!(reply.len(), 1);
        assert_eq!((reply[0].out.flags, reply[0].out.sequence), (RST, 777));
        // And the same for a listener's half-made connection.
        let mut listener = Connection::accept(B, &made(A, 0, SYN, 100, &[]).segment(), Buffers::default());
        drain(&mut listener, 0);
        listener.segment(&made(A + 1, 999, ACK, 100, &[]).segment(), 5);
        assert_eq!(listener.state(), State::SynReceived);
        let reply = drain(&mut listener, 5);
        assert_eq!((reply[0].out.flags, reply[0].out.sequence), (RST, 999));
    }

    #[test]
    fn a_lost_syn_ack_is_sent_again_when_the_syn_comes_again() {
        let mut opener = Connection::connect(A, Buffers::default());
        let syn = drain(&mut opener, 0);
        let mut listener = Connection::accept(B, &syn[0].segment(), Buffers::default());
        let lost = drain(&mut listener, 0);
        opener.tick(1000);
        let again = drain(&mut opener, 1000);
        assert_eq!(again, syn);
        deliver(&mut listener, &again, 1000);
        let answer = drain(&mut listener, 1000);
        assert!(answer.contains(&lost[0]), "the SYN+ACK again, at once: {answer:?}");
        deliver(&mut opener, &answer, 1000);
        settle(&mut opener, &mut listener, 1000);
        assert_eq!((opener.state(), listener.state()), (State::Established, State::Established));
        // And by its own clock, if the SYN does not come again.
        let mut listener = Connection::accept(B, &syn[0].segment(), Buffers::default());
        let lost = drain(&mut listener, 0);
        listener.tick(1000);
        assert_eq!(drain(&mut listener, 1000), lost);
        // Never answered: forgotten, without a word.
        let mut now = 1000;
        while let Some(next) = listener.next_timer() {
            now = next;
            listener.tick(now);
            assert!(drain(&mut listener, now).iter().all(|one| one.out.flags == (SYN | ACK) || one.has(RST)));
        }
        assert_eq!((listener.state(), listener.failure(), now), (State::Closed, Some(Failure::TimedOut), 31_000));
    }

    #[test]
    fn both_sides_may_open_at_once() {
        let mut a = Connection::connect(A, Buffers::default());
        let mut b = Connection::connect(B, Buffers::default());
        let from_a = drain(&mut a, 0);
        let from_b = drain(&mut b, 0);
        deliver(&mut b, &from_a, 10);
        deliver(&mut a, &from_b, 10);
        assert_eq!((a.state(), b.state()), (State::SynReceived, State::SynReceived));
        settle(&mut a, &mut b, 10);
        assert_eq!((a.state(), b.state()), (State::Established, State::Established));
        a.write(b"one way");
        b.write(b"the other");
        settle(&mut a, &mut b, 20);
        assert_eq!((read_all(&mut b), read_all(&mut a)), (b"one way".to_vec(), b"the other".to_vec()));
    }

    #[test]
    fn what_arrives_twice_or_late_is_dropped_and_answered() {
        let (mut a, mut b) = pair();
        a.write(b"abcdef");
        let sent = drain(&mut a, 0);
        deliver(&mut b, &sent, 0);
        drain(&mut b, 0);
        deliver(&mut b, &sent, 0);
        assert_eq!(b.readable(), 6, "not twice");
        let answer = drain(&mut b, 0);
        assert_eq!(answer.len(), 1, "but answered: the first answer may have been lost");
        assert_eq!(answer[0].out.acknowledgment, A + 7);
        // A segment that overlaps what has arrived: only its new part.
        b.segment(&made(A + 4, B + 1, ACK, 1000, b"defghi").segment(), 0);
        assert_eq!(read_all(&mut b), b"abcdefghi");
        // One from far outside the window: dropped, answered.
        b.segment(&made(A + 100_000, B + 1, ACK, 1000, b"zzz").segment(), 0);
        assert_eq!(b.readable(), 0);
        assert_eq!(drain(&mut b, 0).last().map(|one| one.out.acknowledgment), Some(A + 10));
    }

    #[test]
    fn what_arrives_ahead_of_a_gap_is_kept_and_the_gap_asked_for() {
        let (mut a, mut b) = pair();
        let data = pattern(1460 * 5, 3);
        a.write(&data);
        let sent = drain(&mut a, 0);
        assert_eq!(sent.len(), 5);
        // The first is lost; the other four arrive, and each is
        // answered with the number still expected: the lost one's.
        let mut answers = Vec::new();
        for one in &sent[1..] {
            deliver(&mut b, std::slice::from_ref(one), 10);
            answers.extend(drain(&mut b, 10));
            assert_eq!(b.readable(), 0, "nothing is handed on out of order");
        }
        assert_eq!(answers.iter().map(|one| one.out.acknowledgment).collect::<Vec<_>>(), [A + 1; 4]);
        // The same answer twice more is nothing yet (segments do
        // overtake each other). The third time says "a gap": the
        // segment that fills it is sent again at once, long before
        // the timer.
        deliver(&mut a, &answers[..2], 10);
        assert!(drain(&mut a, 10).is_empty());
        deliver(&mut a, &answers[2..], 10);
        let again = drain(&mut a, 10);
        assert_eq!(again, [sent[0].clone()]);
        assert_eq!(a.retransmitted(), 1);
        // A loss: the sender holds itself back to half of what was on
        // its way (five segments, 7300 bytes).
        assert_eq!(a.congestion_window, 3650);
        // It fills the gap: everything is there, and acknowledged in
        // one answer.
        deliver(&mut b, &again, 20);
        assert_eq!(b.readable(), data.len());
        let answer = drain(&mut b, 20);
        assert_eq!(answer.len(), 1);
        assert_eq!(answer[0].out.acknowledgment, A + 1 + data.len() as u32);
        deliver(&mut a, &answer, 20);
        assert_eq!((a.unsent(), a.next_timer(), a.retransmitted()), (0, None, 1));
        assert!(read_all(&mut b) == data);
    }

    #[test]
    fn pieces_may_arrive_in_any_order() {
        let data = pattern(1000, 6);
        let piece = |n: usize| made(A + 1 + 100 * n as u32, B + 1, ACK, 1000, &data[100 * n..100 * (n + 1)]);
        // Every order of the first five pieces, the rest in order.
        let mut orders = vec![vec![]];
        for _ in 0..5 {
            orders = orders.iter().flat_map(|order: &Vec<usize>| (0..5).filter(|n| !order.contains(n)).map(|n| [&order[..], &[n]].concat())).collect();
        }
        assert_eq!(orders.len(), 120);
        for order in orders {
            let (_, mut b) = pair();
            for &n in order.iter().chain(&[5, 6, 7, 8, 9]) {
                b.segment(&piece(n).segment(), 0);
            }
            assert!(read_all(&mut b) == data, "{order:?}");
        }
        // Pieces that overlap each other and what has arrived, twice
        // over, and one that reaches past the end of the room there is.
        let (_, mut b) = pair_from(A, B, Buffers::default(), Buffers { send: 100, receive: 600 });
        for (from, to) in [(300, 500), (100, 250), (450, 560), (200, 320), (150, 480), (550, 900), (0, 120), (0, 700)] {
            b.segment(&made(A + 1 + from as u32, B + 1, ACK, 1000, &data[from..to]).segment(), 0);
        }
        assert_eq!(drain(&mut b, 0).last().map(|one| (one.out.acknowledgment, one.out.window)), Some((A + 601, 0)));
        assert!(read_all(&mut b) == data[..600]);

        // More gaps than are kept track of: the piece that would be
        // one too many is dropped, and nothing else is disturbed.
        let (_, mut b) = pair();
        for n in [2, 4, 6, 8, 9] {
            b.segment(&piece(n).segment(), 0);
        }
        b.segment(&piece(0).segment(), 0);
        b.segment(&piece(1).segment(), 0); // joins up with 2
        assert_eq!(b.readable(), 300);
        b.segment(&piece(3).segment(), 0); // and 4
        b.segment(&piece(5).segment(), 0); // and 6
        b.segment(&piece(7).segment(), 0); // and 8, 9 - of which one was dropped
        let got = read_all(&mut b);
        assert!(got.len() == 900 || got.len() == 1000, "{}", got.len());
        assert!(got == data[..got.len()]);
    }

    #[test]
    fn a_fin_ahead_of_a_gap_waits_for_the_gap() {
        let (_, mut b) = pair();
        b.segment(&made(A + 4, B + 1, ACK | FIN, 1000, b"def").segment(), 0);
        assert_eq!((b.state(), b.finished(), b.readable()), (State::Established, false, 0));
        assert_eq!(drain(&mut b, 0).last().map(|one| one.out.acknowledgment), Some(A + 1));
        b.segment(&made(A + 1, B + 1, ACK, 1000, b"abc").segment(), 0);
        assert_eq!((b.state(), b.finished()), (State::CloseWait, true));
        assert_eq!(drain(&mut b, 0).last().map(|one| one.out.acknowledgment), Some(A + 8), "six bytes and the FIN");
        assert_eq!(read_all(&mut b), b"abcdef");
    }

    #[test]
    fn the_sender_holds_itself_back_and_more_so_after_a_loss() {
        let big = Buffers { send: 60_000, receive: 60_000 };
        let (mut a, mut b) = pair_from(A, B, big, big);
        let data = pattern(60_000, 2);
        a.write(&data);
        // Room for 60000 bytes on the other side - and ten segments
        // are sent: nobody knows yet what the network carries.
        let first = drain(&mut a, 0);
        assert_eq!(first.len(), FIRST_SEGMENTS as usize);
        // Each acknowledgment lets out two more: the one it made room
        // for, and one more - which doubles what is on its way with
        // every round.
        deliver(&mut b, &first[..1], 10);
        deliver(&mut a, &drain(&mut b, 10), 10);
        assert_eq!(drain(&mut a, 10).len(), 2);
        // Everything on its way is lost. The timer: one segment, and
        // no more until it is answered.
        let now = a.next_timer().unwrap();
        a.tick(now);
        let after_loss = drain(&mut a, now);
        assert_eq!(after_loss.len(), 1);
        assert_eq!(after_loss[0].out.sequence, A + 1 + 1460);
        deliver(&mut b, &after_loss, now);
        deliver(&mut a, &drain(&mut b, now), now);
        assert_eq!(drain(&mut a, now).len(), 2, "then two");
        // And it all arrives in the end.
        let mut got = Vec::new();
        for step in 0..200 {
            let at = now + 10 * step;
            a.tick(at);
            for one in drain(&mut a, at) {
                deliver(&mut b, &[one], at);
                deliver(&mut a, &drain(&mut b, at), at);
            }
            got.extend(read_all(&mut b));
            deliver(&mut a, &drain(&mut b, at), at);
        }
        assert!(got == data, "{} of {}", got.len(), data.len());
    }

    #[test]
    fn no_more_is_sent_than_the_other_side_has_room_for() {
        let small = Buffers { send: 16384, receive: 1000 };
        let (mut a, mut b) = pair_from(A, B, Buffers::default(), small);
        let data = pattern(3000, 9);
        a.write(&data);
        let sent = drain(&mut a, 0);
        assert_eq!(sent.iter().map(|one| one.data.len()).sum::<usize>(), 1000, "the window the SYN+ACK offered");
        deliver(&mut b, &sent, 0);
        let full = drain(&mut b, 0);
        assert_eq!(full.last().map(|one| (one.out.acknowledgment, one.out.window)), Some((A + 1001, 0)));
        deliver(&mut a, &full, 0);
        assert!(drain(&mut a, 0).is_empty(), "no room: nothing sent");

        // The program reads; the other side is told there is room.
        let mut got = read_all(&mut b);
        let opened = drain(&mut b, 50);
        assert_eq!(opened.len(), 1);
        assert_eq!(opened[0].out.window, 1000);
        deliver(&mut a, &opened, 50);
        let sent = drain(&mut a, 50);
        assert_eq!(sent.iter().map(|one| one.data.len()).sum::<usize>(), 1000);
        deliver(&mut b, &sent, 50);
        deliver(&mut a, &drain(&mut b, 50), 50);
        got.extend(read_all(&mut b));

        // This time "there is room" is lost. The sender asks, with one
        // byte past the window, when its clock says so.
        let lost = drain(&mut b, 60);
        assert_eq!(lost.len(), 1);
        assert!(drain(&mut a, 60).is_empty());
        let now = a.next_timer().expect("a clock runs while the window is shut");
        a.tick(now);
        let probe = drain(&mut a, now);
        assert_eq!(probe.iter().map(|one| one.data.len()).collect::<Vec<_>>(), [1]);
        deliver(&mut b, &probe, now);
        settle(&mut a, &mut b, now);
        got.extend(read_all(&mut b));
        settle(&mut a, &mut b, now);
        got.extend(read_all(&mut b));
        assert!(got == data, "{} bytes of {}", got.len(), data.len());
        assert_eq!((a.unsent(), a.failure()), (0, None));
    }

    #[test]
    fn a_shut_window_is_asked_about_for_as_long_as_the_other_side_answers() {
        let small = Buffers { send: 16384, receive: 500 };
        let (mut a, mut b) = pair_from(A, B, Buffers::default(), small);
        let data = pattern(800, 4);
        a.write(&data);
        settle(&mut a, &mut b, 0);
        assert_eq!(b.readable(), 500);
        // A program that reads nothing for an hour: the connection
        // waits, asking now and then - never more than a byte, and
        // far more often than it would send data again before giving
        // up.
        let mut probes = 0;
        let mut now = 0;
        while now < 3_600_000 {
            now = a.next_timer().expect("the clock keeps running");
            a.tick(now);
            let sent = drain(&mut a, now);
            assert!(sent.iter().all(|one| one.data.len() <= 1), "{sent:?}");
            probes += sent.len();
            deliver(&mut b, &sent, now);
            deliver(&mut a, &drain(&mut b, now), now);
            assert_eq!(a.state(), State::Established);
        }
        assert!(probes > usize::from(DATA_RETRIES) * 2, "{probes}");
        assert_eq!(b.readable(), 500, "the byte past the window was not taken");
        let mut got = read_all(&mut b);
        settle(&mut a, &mut b, now);
        // (Whatever is still unsent goes when the next probe is due.)
        if b.readable() < 300 {
            let next = a.next_timer().unwrap();
            a.tick(next);
            settle(&mut a, &mut b, next);
        }
        got.extend(read_all(&mut b));
        assert!(got == data, "{} bytes of {}", got.len(), data.len());
    }

    #[test]
    fn each_side_closes_its_own_direction() {
        let (mut a, mut b) = pair();
        a.write(b"last words");
        a.shutdown();
        assert_eq!(a.state(), State::FinWait1);
        assert_eq!(a.write(b"more"), 0, "nothing after the end");
        let sent = drain(&mut a, 0);
        assert_eq!(sent.len(), 2);
        assert_eq!((sent[1].out.flags, sent[1].out.sequence), (FIN | ACK, A + 11), "the FIN has the number after the last byte");
        deliver(&mut b, &sent, 0);
        assert_eq!((b.state(), b.finished(), b.readable()), (State::CloseWait, true, 10));
        deliver(&mut a, &drain(&mut b, 0), 0);
        assert_eq!(a.state(), State::FinWait2);
        assert_eq!(a.next_timer(), None, "nothing is waited for: the other side may take as long as it likes");

        // Half closed: the other direction still works.
        let reply = pattern(4000, 5);
        assert_eq!(b.write(&reply), 4000);
        settle(&mut a, &mut b, 5000);
        assert_eq!(read_all(&mut a), reply);
        assert_eq!(read_all(&mut b), b"last words");

        b.shutdown();
        assert_eq!(b.state(), State::LastAck);
        settle(&mut a, &mut b, 6000);
        assert_eq!((a.state(), b.state()), (State::TimeWait, State::Closed));
        assert_eq!((a.finished(), a.failure(), b.failure()), (true, None, None));
        // The side that closed first remembers the connection a while.
        assert_eq!(a.next_timer(), Some(6000 + TIME_WAIT_MS));
        a.tick(6000 + TIME_WAIT_MS - 1);
        assert_eq!(a.state(), State::TimeWait);
        a.tick(6000 + TIME_WAIT_MS);
        assert_eq!((a.state(), a.next_timer()), (State::Closed, None));
    }

    #[test]
    fn a_lost_fin_is_sent_again_and_a_lost_last_ack_too() {
        let (mut a, mut b) = pair();
        a.shutdown();
        let lost = drain(&mut a, 0);
        assert_eq!(lost.len(), 1);
        let now = a.next_timer().unwrap();
        a.tick(now);
        let again = drain(&mut a, now);
        assert_eq!(again, lost);
        deliver(&mut b, &again, now);
        b.shutdown();
        let both = drain(&mut b, now); // its ACK and its FIN, in one
        assert_eq!(both.len(), 1);
        assert_eq!(both[0].out.flags, FIN | ACK);
        deliver(&mut a, &both, now);
        assert_eq!(a.state(), State::TimeWait);
        let last_ack = drain(&mut a, now);
        assert_eq!(last_ack.len(), 1);
        // That last ACK is lost: the FIN comes again, and is answered
        // again - which is what TimeWait is for - and the wait starts
        // over.
        let later = b.next_timer().unwrap();
        b.tick(later);
        let fin_again = drain(&mut b, later);
        assert_eq!(fin_again.len(), 1);
        assert!(fin_again[0].has(FIN));
        deliver(&mut a, &fin_again, later);
        assert_eq!(a.next_timer(), Some(later + TIME_WAIT_MS));
        let answer = drain(&mut a, later);
        assert_eq!(answer.len(), 1);
        deliver(&mut b, &answer, later);
        assert_eq!((b.state(), b.failure()), (State::Closed, None));
    }

    #[test]
    fn both_sides_may_close_at_once() {
        let (mut a, mut b) = pair();
        a.shutdown();
        b.shutdown();
        let from_a = drain(&mut a, 0);
        let from_b = drain(&mut b, 0);
        deliver(&mut b, &from_a, 10);
        deliver(&mut a, &from_b, 10);
        assert_eq!((a.state(), b.state()), (State::Closing, State::Closing));
        settle(&mut a, &mut b, 10);
        assert_eq!((a.state(), b.state()), (State::TimeWait, State::TimeWait));
        a.tick(10 + TIME_WAIT_MS);
        b.tick(10 + TIME_WAIT_MS);
        assert_eq!((a.state(), b.state(), a.failure(), b.failure()), (State::Closed, State::Closed, None, None));
    }

    #[test]
    fn a_reset_is_believed_only_with_exactly_the_number_expected() {
        let (mut a, _) = pair();
        // Far away: not a word in answer.
        a.segment(&made(B + 500_000, 0, RST, 0, &[]).segment(), 0);
        assert_eq!(a.state(), State::Established);
        assert!(drain(&mut a, 0).is_empty());
        // Near, but not exact: "this is where I am".
        a.segment(&made(B + 50, 0, RST, 0, &[]).segment(), 0);
        assert_eq!(a.state(), State::Established);
        let challenge = drain(&mut a, 0);
        assert_eq!(challenge.len(), 1);
        assert_eq!((challenge[0].out.flags, challenge[0].out.acknowledgment), (ACK, B + 1));
        // Exact.
        a.segment(&made(B + 1, 0, RST, 0, &[]).segment(), 0);
        assert_eq!((a.state(), a.failure()), (State::Closed, Some(Failure::Reset)));
        assert!(drain(&mut a, 0).is_empty());
        assert_eq!(a.next_timer(), None);
    }

    #[test]
    fn what_does_not_belong_changes_nothing() {
        let (mut a, mut b) = pair();
        // A SYN in the middle of a connection.
        a.segment(&made(B + 10, 0, SYN, 100, &[]).segment(), 0);
        // An acknowledgment of bytes never sent.
        a.segment(&made(B + 1, A + 5000, ACK, 100, &[]).segment(), 0);
        // A segment with no acknowledgment at all.
        a.segment(&made(B + 1, 0, 0, 100, b"data").segment(), 0);
        assert_eq!((a.state(), a.readable()), (State::Established, 0));
        let answers = drain(&mut a, 0);
        assert!(answers.iter().all(|one| one.out.flags == ACK && one.out.acknowledgment == B + 1), "{answers:?}");
        // An old acknowledgment, arriving late, takes nothing back.
        a.write(b"abc");
        settle(&mut a, &mut b, 0);
        a.segment(&made(B + 1, A + 1, ACK, 3, &[]).segment(), 0);
        assert_eq!(a.unsent(), 0);
        // ... and neither does its window: an older segment does not
        // say how much room there is now.
        a.write(&pattern(2000, 0));
        assert_eq!(drain(&mut a, 0).iter().map(|one| one.data.len()).sum::<usize>(), 2000);
    }

    #[test]
    fn the_numbers_may_pass_zero_in_the_middle_of_a_connection() {
        let (mut a, mut b) = pair_from(0xFFFF_FFF0, 0xFFFF_FFFE, Buffers::default(), Buffers::default());
        let (there, back) = (pattern(9000, 1), pattern(7000, 2));
        a.write(&there);
        b.write(&back);
        settle(&mut a, &mut b, 0);
        assert_eq!((read_all(&mut b), read_all(&mut a)), (there, back));
        // And a loss just there.
        a.write(b"again");
        drain(&mut a, 10);
        let now = a.next_timer().unwrap();
        a.tick(now);
        settle(&mut a, &mut b, now);
        assert_eq!(read_all(&mut b), b"again");
        a.shutdown();
        settle(&mut a, &mut b, now);
        b.shutdown();
        settle(&mut a, &mut b, now);
        assert_eq!((a.state(), b.state()), (State::TimeWait, State::Closed));
    }

    #[test]
    fn a_connection_may_pass_halfway_round_too() {
        let (mut a, mut b) = pair_from(0x7FFF_F000, 0x7FFF_FF00, Buffers::default(), Buffers::default());
        let (there, back) = (pattern(12_000, 3), pattern(9000, 4));
        a.write(&there);
        b.write(&back);
        settle(&mut a, &mut b, 0);
        assert!(read_all(&mut b) == there && read_all(&mut a) == back);
        a.shutdown();
        settle(&mut a, &mut b, 0);
        b.shutdown();
        settle(&mut a, &mut b, 0);
        assert_eq!((a.state(), b.state(), a.failure(), b.failure()), (State::TimeWait, State::Closed, None, None));
    }

    #[test]
    fn the_first_of_several_segments_is_the_one_timed() {
        let (mut a, mut b) = pair();
        // (The handshake took 0 ms: the one sample so far.)
        assert_eq!(a.smoothed, Some(0));
        // One segment at 0, another at 300; one answer for both at
        // 500. The time is 500 ms - the first's - not 200: smoothed,
        // (7 * 0 + 500) / 8.
        a.write(b"first");
        let first = drain(&mut a, 0);
        a.write(b"second");
        let second = drain(&mut a, 300);
        deliver(&mut b, &first, 500);
        deliver(&mut b, &second, 500);
        let answer = drain(&mut b, 500);
        deliver(&mut a, &answer[answer.len() - 1..], 500);
        assert_eq!(a.smoothed, Some(62));
    }

    #[test]
    fn a_window_said_by_an_older_segment_is_not_believed() {
        let (mut a, _) = pair();
        // Nothing but an answer, with room for 5000 ...
        a.segment(&made(B + 11, A + 1, ACK, 5000, &[]).segment(), 0);
        // ... and then, late, an older segment (it overlaps what has
        // arrived since, so it is not simply dropped) saying 1000.
        a.segment(&made(B + 1, A + 1, ACK, 5000, &pattern(10, 0)).segment(), 0);
        a.segment(&made(B + 1, A + 1, ACK, 1000, &pattern(20, 0)).segment(), 0);
        a.write(&pattern(4000, 1));
        assert_eq!(drain(&mut a, 0).iter().map(|one| one.data.len()).sum::<usize>(), 4000);
    }

    #[test]
    fn a_window_that_opens_again_starts_the_clock_afresh() {
        let small = Buffers { send: 16384, receive: 1000 };
        let (mut a, mut b) = pair_from(A, B, Buffers::default(), small);
        a.write(&pattern(3000, 9));
        deliver(&mut b, &drain(&mut a, 0), 0);
        deliver(&mut a, &drain(&mut b, 0), 0);
        assert!(drain(&mut a, 0).is_empty());
        // Shut: the clock runs, for asking.
        assert_eq!(a.next_timer(), Some(RTO_MIN_MS));
        read_all(&mut b);
        deliver(&mut a, &drain(&mut b, 150), 150);
        // Open again at 150: what is sent now is waited for from now.
        assert!(!drain(&mut a, 150).is_empty());
        assert_eq!(a.next_timer(), Some(150 + RTO_MIN_MS));
    }

    #[test]
    fn a_program_that_goes_away_closes_in_order_or_says_why_not() {
        // Nothing unread: what it wrote is sent, then a FIN.
        let (mut a, mut b) = pair();
        a.write(b"goodbye");
        a.close(0);
        settle(&mut a, &mut b, 0);
        assert_eq!((read_all(&mut b), b.finished(), b.failure()), (b"goodbye".to_vec(), true, None));
        // The other side answers with data: nobody is there to read
        // it, and it is told so.
        b.write(b"wait, one more thing");
        settle(&mut a, &mut b, 10);
        assert_eq!((a.state(), b.state(), b.failure()), (State::Closed, State::Closed, Some(Failure::Reset)));

        // Something unread when it goes: cut off at once.
        let (mut a, mut b) = pair();
        b.write(b"never read");
        settle(&mut a, &mut b, 0);
        a.close(0);
        assert_eq!(a.state(), State::Closed);
        let sent = drain(&mut a, 0);
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].out.flags, RST | ACK);
        deliver(&mut b, &sent, 0);
        assert_eq!(b.failure(), Some(Failure::Reset));

        // The other side never closes: after a minute, cut off.
        let (mut a, mut b) = pair();
        a.close(500);
        settle(&mut a, &mut b, 500);
        assert_eq!(a.state(), State::FinWait2);
        assert_eq!(a.next_timer(), Some(500 + ORPHAN_MS));
        a.tick(500 + ORPHAN_MS);
        assert_eq!(a.state(), State::Closed);
        settle(&mut a, &mut b, 500 + ORPHAN_MS);
        assert_eq!(b.failure(), Some(Failure::Reset));

        // Before the connection was made: nothing to say.
        let mut opener = Connection::connect(A, Buffers::default());
        drain(&mut opener, 0);
        opener.close(0);
        assert_eq!((opener.state(), opener.next_timer()), (State::Closed, None));
        assert!(drain(&mut opener, 0).is_empty());
    }

    #[test]
    fn the_wait_follows_how_long_answers_take() {
        let (mut a, mut b) = pair();
        let rto = |a: &mut Connection, b: &mut Connection, start: u64, took: u64| {
            a.write(b"x");
            let sent = drain(a, start);
            deliver(b, &sent, start + took);
            deliver(a, &drain(b, start + took), start + took);
            read_all(b);
            a.rto
        };
        // Answers after 500 ms, time after time: the wait comes down
        // towards that (and never below it).
        let mut now = 0;
        let mut waits = Vec::new();
        for _ in 0..30 {
            waits.push(rto(&mut a, &mut b, now, 500));
            now += 1000;
        }
        assert!(waits.iter().max() > Some(&1000), "at first the times vary - a handshake at once, then half a second: {waits:?}");
        assert!((510..=600).contains(&waits[29]), "{waits:?}");
        // One answer takes three seconds: the wait jumps up.
        assert!(rto(&mut a, &mut b, now, 3000) > 1500);
        // An answer to something sent twice is not timed at all.
        let before = a.rto;
        a.write(b"y");
        drain(&mut a, 100_000);
        a.tick(a.next_timer().unwrap());
        assert_eq!(a.wait(), (before * 2).min(RTO_MAX_MS));
        settle(&mut a, &mut b, 200_000);
        assert_eq!((a.rto, a.wait()), (before, before), "which copy was answered? nobody knows - so nothing is learned from it");
    }

    /// Numbers that look random and are the same every run.
    struct Dice(u64);

    impl Dice {
        fn roll(&mut self, sides: u64) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0 % sides
        }
    }

    /// Two programs sending each other `there` and `back` over a
    /// network that loses `lost` segments in a hundred, doubles some
    /// and delivers some late (so: out of order). Returns the
    /// simulated time it took, and how many segments were sent again.
    fn across_a_bad_network(seed: u64, lost: u64, there: &[u8], back: &[u8], buffers: Buffers) -> (u64, u32) {
        let mut dice = Dice(seed);
        let mut a = Connection::connect(dice.roll(1 << 32) as u32, buffers);
        let mut b: Option<Connection> = None;
        // In flight: when it arrives, to which side, what.
        let mut wire: Vec<(u64, bool, Sent)> = Vec::new();
        let (mut written_a, mut written_b) = (0, 0);
        let (mut got_a, mut got_b) = (Vec::new(), Vec::new());
        let mut now = 0;
        loop {
            // The programs.
            written_a += a.write(&there[written_a..]);
            if written_a == there.len() && a.state() != State::SynSent {
                a.shutdown();
            }
            got_a.extend(read_all(&mut a));
            if let Some(b) = b.as_mut() {
                written_b += b.write(&back[written_b..]);
                if written_b == back.len() {
                    b.shutdown();
                }
                got_b.extend(read_all(b));
            }
            // What each sends goes on the wire - or does not. (Each
            // segment that arrives is answered before the next is
            // looked at, as it is with a real card.)
            let mut outgoing: Vec<(bool, Sent)> = drain(&mut a, now).into_iter().map(|sent| (true, sent)).collect();
            if let Some(b) = b.as_mut() {
                outgoing.extend(drain(b, now).into_iter().map(|sent| (false, sent)));
            }
            now += 10;
            let (arriving, rest): (Vec<_>, Vec<_>) = wire.into_iter().partition(|(at, _, _)| *at <= now);
            wire = rest;
            for (_, to_b, sent) in arriving {
                if !to_b {
                    a.segment(&sent.segment(), now);
                    outgoing.extend(drain(&mut a, now).into_iter().map(|sent| (true, sent)));
                } else if let Some(b) = b.as_mut() {
                    b.segment(&sent.segment(), now);
                    outgoing.extend(drain(b, now).into_iter().map(|sent| (false, sent)));
                } else if sent.out.flags == SYN {
                    b = Some(Connection::accept(dice.roll(1 << 32) as u32, &sent.segment(), buffers));
                }
            }
            for (to_b, sent) in outgoing {
                if dice.roll(100) < lost {
                    continue;
                }
                let late = if dice.roll(10) == 0 { 10 * dice.roll(8) } else { 0 };
                if dice.roll(25) == 0 {
                    wire.push((now + 10 + late + 20, to_b, sent.clone()));
                }
                wire.push((now + 10 + late, to_b, sent));
            }
            a.tick(now);
            if let Some(b) = b.as_mut() {
                b.tick(now);
            }
            let b_closed = b.as_ref().is_some_and(|b| b.state() == State::Closed);
            if a.state() == State::Closed && b_closed {
                break;
            }
            assert!(now < 3_600_000, "seed {seed}: not done after an hour: {:?} {:?}", a.state(), b.as_ref().map(|b| b.state()));
        }
        let b = b.unwrap();
        assert_eq!((a.failure(), b.failure()), (None, None), "seed {seed} at {now}: {} of {} there, {} of {} back", got_b.len(), there.len(), got_a.len(), back.len());
        assert!(got_b == there, "seed {seed}: what was sent is not what arrived ({} of {} bytes)", got_b.len(), there.len());
        assert!(got_a == back, "seed {seed}: what was sent back is not what arrived ({} of {} bytes)", got_a.len(), back.len());
        (now, a.retransmitted() + b.retransmitted())
    }

    #[test]
    fn everything_arrives_once_and_in_order_over_a_network_that_loses_doubles_and_delays() {
        let (there, back) = (pattern(60_000, 7), pattern(45_000, 8));
        // A good network first: nothing needs sending twice... except
        // what the delays put out of order.
        let (took, _) = across_a_bad_network(1, 0, &there, &back, Buffers::default());
        assert!(took < 60_000, "{took} ms");
        // Ten segments in a hundred lost: about 75 segments a run,
        // so some seven of them to send again - and it must not take
        // for ever.
        let (mut again, mut longest) = (0, 0);
        for seed in 2..12 {
            let (took, sent_again) = across_a_bad_network(seed, 10, &there, &back, Buffers::default());
            again += sent_again;
            longest = longest.max(took);
        }
        assert!((50..400).contains(&again), "ten in a hundred lost, and {again} sent again?");
        assert!(longest < 120_000, "{longest} ms");
        // A quarter lost: slow, and still whole.
        for seed in 40..44 {
            across_a_bad_network(seed, 25, &there[..20_000], &back[..10_000], Buffers::default());
        }
        // Small buffers: windows that shut and open all the time.
        for seed in 20..26 {
            across_a_bad_network(seed, 5, &there[..9000], &back[..9000], Buffers { send: 700, receive: 300 });
        }
        // Nothing to send at all, one way or both.
        across_a_bad_network(30, 10, &[], &back[..2000], Buffers::default());
        across_a_bad_network(31, 10, &[], &[], Buffers::default());
    }
}
