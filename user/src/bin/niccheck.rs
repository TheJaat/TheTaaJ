//! niccheck: a test of the network card's driver from outside it, as a
//! client - with someone on the other end of the wire.
//!
//! A card cannot be tested alone: what it sends has to arrive
//! somewhere, and what it receives has to come from somewhere. The
//! boot test connects the machine's card to a small program on the
//! host (`kernel64/scripts/wire.py`) that sees every frame this
//! machine sends and answers the ones below. On any other network
//! nobody answers, and this program says so and changes nothing.
//!
//! The frames are of a type set aside for experiments (0x88B5), so
//! nothing else on a network would take them for its own:
//!
//!   bytes 0-5    destination        6-11  source       12-13  0x88B5
//!   byte 14      what is asked      15    a number     16-    data
//!
//!   ECHO       the peer sends the frame back to this card alone, the
//!              kind turned to ECHOED and every data byte flipped - so
//!              an answer is proof that the bytes went out and others
//!              came in, not that something looped
//!   BURST n    the peer sends n frames of 100 bytes, numbered, at once
//!   STRANGERS  the peer sends a frame to another card, one to a group
//!              of cards, and then one to everyone
//!   LONG n     as BURST, of frames as long as a frame can be
//!
//! `niccheck restart` is the other test: the driver is asked to answer
//! nothing for a while (a driver that hangs), init must stop it and
//! start another, and the card must work again with the new one.
//!
//! `niccheck stall` is the first half of that, and needs nobody on the
//! wire: the driver is made to hang, and this program waits until
//! another answers. For testing what everything *else* does when the
//! driver is replaced under it.
//!
//! Exit code 0 if everything held, 77 if nobody answered, else the
//! line of the failed check.

#![no_std]
#![no_main]

use abi::nic::{self, Stats, FRAME_MAX, FRAME_MIN, PIECE_MAX, WIRE_MIN};
use taaj::nic::{Frames, Nic, NicError};
use taaj::println;

taaj::program!(main);

const TYPE: [u8; 2] = [0x88, 0xB5];
const ECHO: u8 = 1;
const BURST: u8 = 2;
const STRANGERS: u8 = 3;
const LONG_BURST: u8 = 4;
/// In frames that must never reach the wire.
const NEVER: u8 = 9;
const ECHOED: u8 = 0x81;
const BURST_FRAME: u8 = 0x82;
const AFTER_STRANGERS: u8 = 0x83;
const FLIP: u8 = 0x5A;
const BURST_LENGTH: usize = 100;

/// Not part of the protocol: nobody answered on the wire.
const NO_PEER: i64 = 77;

/// How long an answer may take, in ticks.
const WAIT: u64 = 300;

fn pattern(number: u8, i: usize) -> u8 {
    (number as usize).wrapping_mul(31).wrapping_add(i * 7).wrapping_add(i >> 8) as u8
}

/// A frame of `length` bytes from `from` to everyone, asking `kind`.
fn frame(buffer: &mut [u8; FRAME_MAX + 1], from: &[u8; 6], kind: u8, number: u8, length: usize) -> usize {
    buffer[..6].copy_from_slice(&nic::BROADCAST);
    buffer[6..12].copy_from_slice(from);
    buffer[12..14].copy_from_slice(&TYPE);
    for (i, byte) in buffer[14..length.max(16)].iter_mut().enumerate() {
        *byte = match i {
            0 => kind,
            1 => number,
            _ => pattern(number, i),
        };
    }
    length
}

/// The next frame of the experiment's type, of kind `kind`: its length.
/// Anything else on the wire is passed over.
fn next(frames: &mut Frames, into: &mut [u8; FRAME_MAX], kind: u8, deadline: u64) -> Option<usize> {
    loop {
        match frames.next_until(into, deadline) {
            Ok(Some(length)) if length >= 16 && into[12..14] == TYPE && into[14] == kind => return Some(length),
            Ok(Some(_)) => {}
            _ => return None,
        }
    }
}

/// The driver made to hang, and nothing more: returns when another
/// driver answers.
fn stall(card: &Nic) -> i64 {
    let Ok(mut frames) = card.listen(&[u16::from_be_bytes(TYPE)]) else {
        println!("[niccheck] FAILED: cannot listen");
        return line!() as i64;
    };
    let mut request = [0u8; 9];
    request[0] = nic::STALL;
    request[1..].copy_from_slice(&2000u64.to_le_bytes());
    if card.raw(&request).is_err() {
        println!("[niccheck] FAILED: the driver cannot be asked to hang");
        return line!() as i64;
    }
    // As in `restart`: the channel closes when the old driver is stopped.
    let mut back = [0u8; FRAME_MAX];
    if frames.next_until(&mut back, taaj::ticks() + 1000) != Err(taaj::Error::PeerClosed) {
        println!("[niccheck] FAILED: a driver that hangs was not stopped");
        return line!() as i64;
    }
    let deadline = taaj::ticks() + 1000;
    while card.info().is_err() {
        if taaj::ticks() >= deadline {
            println!("[niccheck] FAILED: no new driver answers");
            return line!() as i64;
        }
        taaj::sleep(10);
    }
    println!("[niccheck] stall: the driver was stopped, and another answers");
    0
}

/// The driver made to hang: init notices, stops it and starts another,
/// which must find the card as the first did.
fn restart(card: &mut Nic) -> i64 {
    let mut checks = 0u32;
    macro_rules! check {
        ($cond:expr, $what:literal) => {
            if !$cond {
                println!("[niccheck] FAILED: {}", $what);
                return line!() as i64;
            }
            checks += 1;
        };
    }
    let mut out = [0u8; FRAME_MAX + 1];
    let mut back = [0u8; FRAME_MAX];

    let before = card.info();
    check!(before.is_ok(), "the driver answers before it is made to hang");
    let ours = before.map_or([0; 6], |info| info.address);
    let Ok(mut frames) = card.listen(&[u16::from_be_bytes(TYPE)]) else {
        println!("[niccheck] FAILED: cannot listen");
        return line!() as i64;
    };
    frame(&mut out, &ours, ECHO, 1, 100);
    check!(card.send(&out[..100]).is_ok(), "a frame can be given to the driver");
    if next(&mut frames, &mut back, ECHOED, taaj::ticks() + WAIT).is_none() {
        println!("[niccheck] nobody answers on this network: the card was asked to send one frame, and nothing more");
        return NO_PEER;
    }

    // Twenty seconds of nothing: far longer than init waits.
    let mut stall = [0u8; 9];
    stall[0] = nic::STALL;
    stall[1..].copy_from_slice(&2000u64.to_le_bytes());
    check!(card.raw(&stall).is_ok(), "the driver can be asked to hang");
    // The old driver held the other end of `frames`: when it is
    // stopped, that end closes. A driver that merely woke up again
    // would have kept it.
    let gone = frames.next_until(&mut back, taaj::ticks() + 1000);
    check!(gone == Err(taaj::Error::PeerClosed), "a driver that hangs is stopped");

    // The new one, as soon as it answers.
    let deadline = taaj::ticks() + 1000;
    let mut after = card.info();
    while after.is_err() && taaj::ticks() < deadline {
        taaj::sleep(10);
        after = card.info();
    }
    check!(after == before, "the new driver finds the same card");
    let sent_before = card.stats().map_or(u64::MAX, |stats| stats.sent);
    check!(sent_before == 0, "and it is a new driver: it has sent nothing yet");
    let Ok(mut frames) = card.listen(&[u16::from_be_bytes(TYPE)]) else {
        println!("[niccheck] FAILED: cannot listen to the new driver");
        return line!() as i64;
    };
    frame(&mut out, &ours, ECHO, 2, 700);
    check!(card.send(&out[..700]).is_ok(), "a frame can be given to the new driver");
    check!(next(&mut frames, &mut back, ECHOED, taaj::ticks() + WAIT) == Some(700), "the card works again: a frame out and its answer back");
    check!(back[15] == 2 && (16..700).all(|i| back[i] == out[i] ^ FLIP), "and the answer is whole");

    println!("[niccheck] restart: {checks} checks, all passed: the driver was stopped, another started, and the card works again");
    0
}

fn main() -> i64 {
    let Some(requests) = taaj::services().and_then(|services| services.lookup("nic").ok()) else {
        println!("[niccheck] no network card to test (no service called \"nic\" for this program)");
        return 1;
    };
    let mut card = Nic::new(requests);
    let arguments = taaj::arguments();
    match arguments.first().map(|argument| &argument[..]) {
        None => {}
        Some("restart") if arguments.len() == 1 => return restart(&mut card),
        Some("stall") if arguments.len() == 1 => return stall(&card),
        Some(_) => {
            println!("usage: niccheck, niccheck restart, niccheck stall");
            return 2;
        }
    }
    let mut checks = 0u32;

    macro_rules! check {
        ($cond:expr, $what:literal) => {
            if !$cond {
                println!("[niccheck] FAILED: {}", $what);
                return line!() as i64;
            }
            checks += 1;
        };
    }

    let info = match card.info() {
        Ok(info) => info,
        Err(error) => {
            println!("[niccheck] FAILED: {}", error.describe());
            return line!() as i64;
        }
    };
    let ours = info.address;
    check!(ours != [0; 6] && ours[0] & 1 == 0, "the card has an address of its own, for one card");
    check!(info.link_up, "the link is up");
    let Ok(before) = card.stats() else {
        println!("[niccheck] FAILED: the driver does not count");
        return line!() as i64;
    };
    let Ok(mut frames) = card.listen(&[u16::from_be_bytes(TYPE)]) else {
        println!("[niccheck] FAILED: cannot listen");
        return line!() as i64;
    };

    let mut out = [0u8; FRAME_MAX + 1];
    let mut back = [0u8; FRAME_MAX];
    let mut sent = 0u64;

    // Is anyone there? One frame, and an answer or none.
    let length = frame(&mut out, &ours, ECHO, 0, 100);
    check!(card.send(&out[..length]).is_ok(), "a frame can be given to the driver");
    sent += 1;
    if next(&mut frames, &mut back, ECHOED, taaj::ticks() + WAIT).is_none() {
        println!("[niccheck] nobody answers on this network: the card was asked to send one frame, and nothing more");
        return NO_PEER;
    }

    // Frames of every awkward length, out and back: the shortest a
    // frame can be (the wire wants more: the driver fills it up), the
    // wire's own shortest, both sides of where a frame is cut into two
    // messages, and the longest.
    let began = taaj::ticks();
    let lengths = [FRAME_MIN + 2, WIRE_MIN - 1, WIRE_MIN, WIRE_MIN + 1, 100, PIECE_MAX - 1, PIECE_MAX, PIECE_MAX + 1, 1500, FRAME_MAX - 1, FRAME_MAX];
    for (n, &length) in lengths.iter().enumerate() {
        let number = n as u8 + 1;
        frame(&mut out, &ours, ECHO, number, length);
        check!(card.send(&out[..length]).is_ok(), "a frame can be sent");
        sent += 1;
        let answer = next(&mut frames, &mut back, ECHOED, taaj::ticks() + WAIT);
        check!(answer == Some(length.max(WIRE_MIN)), "a frame sent comes back, as long as it was on the wire");
        check!(back[..6] == ours && back[15] == number, "the answer is to this card, and to this frame");
        let whole = (16..length).all(|i| back[i] == out[i] ^ FLIP);
        check!(whole, "every byte of the frame went out and came back");
        // (The peer flips those too: a zero comes back as FLIP.)
        check!(back[length.max(16)..length.max(WIRE_MIN)].iter().all(|&byte| byte == FLIP), "a short frame was filled up with zeroes");
    }

    // The card says when a frame has come: nobody has to ask it. (A
    // driver that never hears the card still finds each frame the next
    // time something else wakes it - seconds later, not at once.)
    check!(taaj::ticks() - began < 100, "eleven frames out and their answers back take less than a second");

    // What is not a frame is not sent: too short, too long, and pieces
    // that were given up. Nothing of these may reach the wire - the
    // peer looks for their mark.
    let refused_before = card.stats().map_or(0, |stats| stats.refused);
    frame(&mut out, &ours, NEVER, 0, FRAME_MAX + 1);
    check!(card.send(&out[..FRAME_MIN - 1]).is_ok(), "a frame too short can be handed over");
    check!(card.send(&out[..FRAME_MAX + 1]).is_ok(), "a frame too long can be handed over");
    let mut part = [0u8; 1 + 200];
    part[0] = nic::PART;
    part[1..].copy_from_slice(&out[..200]);
    check!(card.raw_piece(&part).is_ok() && card.raw_piece(&[nic::DROP]).is_ok(), "pieces can be given up");
    check!(card.raw_piece(&[]).is_ok() && card.raw_piece(&[nic::INFO]).is_ok(), "nonsense can be handed over");
    // Requests a careless client could send, with a channel to answer on.
    check!(card.raw(&[99]) == Err(NicError::Refused(nic::BAD_REQUEST)), "an unknown request is refused");
    check!(card.raw(&[nic::INFO, 0]) == Err(NicError::Refused(nic::BAD_REQUEST)), "a request with too much in it is refused");
    check!(card.raw(&[nic::LAST, 1, 2, 3]) == Err(NicError::Refused(nic::BAD_REQUEST)), "a frame with a channel to answer on is refused");
    let refused = card.stats().map_or(0, |stats| stats.refused) - refused_before;
    check!(refused == 4, "the driver counts what it would not send: too short, too long, and two pieces of nonsense");
    // And after all that the driver still works, and the next frame is whole.
    frame(&mut out, &ours, ECHO, 50, 300);
    check!(card.send(&out[..300]).is_ok(), "a frame can be sent after the nonsense");
    sent += 1;
    check!(next(&mut frames, &mut back, ECHOED, taaj::ticks() + WAIT) == Some(300), "the driver survives careless clients");
    check!((16..300).all(|i| back[i] == out[i] ^ FLIP), "and what was given up is not in the next frame");

    // Frames for others are not passed on: one to another card and one
    // to a group go by, and the one for everyone after them arrives.
    // Who keeps them back depends on the card: a virtio card hands the
    // driver everything on the wire, and the driver counts the two as
    // not ours; an Intel card compares addresses itself, and the
    // driver never sees them - only the one for everyone.
    let (strangers_before, received_before) = card.stats().map_or((0, 0), |stats| (stats.not_ours, stats.received));
    frame(&mut out, &ours, STRANGERS, 0, 100);
    check!(card.send(&out[..100]).is_ok(), "the peer can be asked for frames to others");
    sent += 1;
    check!(next(&mut frames, &mut back, AFTER_STRANGERS, taaj::ticks() + WAIT).is_some(), "a frame for everyone arrives");
    check!(back[..6] == nic::BROADCAST, "and is the one for everyone");
    let (strangers, seen) =
        card.stats().map_or((0, 0), |stats| (stats.not_ours - strangers_before, stats.received - received_before));
    check!(
        (strangers, seen) == (2, 3) || (strangers, seen) == (0, 1),
        "two frames for others were kept back: by the driver (seen, and counted) or by the card (never seen)"
    );

    // Many at once, read as they come: all of them, in order.
    const SMALL: u8 = 20;
    let length = frame(&mut out, &ours, BURST, SMALL, 100);
    check!(card.send(&out[..length]).is_ok(), "the peer can be asked for a burst");
    sent += 1;
    for number in 0..SMALL {
        let answer = next(&mut frames, &mut back, BURST_FRAME, taaj::ticks() + WAIT);
        check!(answer == Some(BURST_LENGTH) && back[15] == number, "the frames of a burst arrive, each once, in order");
        check!((16..BURST_LENGTH).all(|i| back[i] == pattern(number, i)), "and whole");
    }

    // Many at once, and nobody reading: the driver must not wait. What
    // does not fit this program's queue is dropped, and counted - and
    // what does arrive is the first of them, in order, each whole.
    const LARGE: u8 = 100;
    let (dropped_before, received_before) = card.stats().map_or((0, 0), |stats| (stats.dropped, stats.received));
    frame(&mut out, &ours, BURST, LARGE, 100);
    check!(card.send(&out[..100]).is_ok(), "the peer can be asked for a large burst");
    sent += 1;
    // Until the driver has seen them all, one way or the other.
    let mut seen = 0;
    let deadline = taaj::ticks() + 2 * WAIT;
    while seen < u64::from(LARGE) && taaj::ticks() < deadline {
        taaj::sleep(5);
        seen = card.stats().map_or(0, |stats| stats.received) - received_before;
    }
    check!(seen == u64::from(LARGE), "the card took in a burst nobody was reading");
    let dropped = card.stats().map_or(0, |stats| stats.dropped) - dropped_before;
    let mut arrived = 0u64;
    while let Some(length) = next(&mut frames, &mut back, BURST_FRAME, taaj::ticks() + 20) {
        check!(length == BURST_LENGTH && u64::from(back[15]) == arrived, "what arrived of the burst is its beginning, in order");
        arrived += 1;
    }
    check!(arrived == abi::CHANNEL_QUEUE_MAX as u64, "as many arrived as this program's queue holds");
    check!(arrived + dropped == u64::from(LARGE), "and every other one was dropped, and counted");

    // A long frame travels in two messages, so a queue can fill up
    // between the two. With nobody reading, one short frame and then
    // long ones: the short one and fifteen long fill all but one place
    // of this program's queue, the sixteenth's first half takes it, and
    // its second half has nowhere to go. The driver must then say so
    // (`nic::DROP`) before the next frame - or that half would be
    // glued to the front of it.
    const LONG: u8 = 20;
    const LONG_FIT: u8 = (abi::CHANNEL_QUEUE_MAX as u8 - 1) / 2;
    let (dropped_before, received_before) = card.stats().map_or((0, 0), |stats| (stats.dropped, stats.received));
    frame(&mut out, &ours, BURST, 1, 100);
    check!(card.send(&out[..100]).is_ok(), "the peer can be asked for one frame");
    frame(&mut out, &ours, LONG_BURST, LONG, 100);
    check!(card.send(&out[..100]).is_ok(), "the peer can be asked for a burst of long frames");
    sent += 2;
    let mut seen = 0;
    let deadline = taaj::ticks() + 2 * WAIT;
    while seen < 1 + u64::from(LONG) && taaj::ticks() < deadline {
        taaj::sleep(5);
        seen = card.stats().map_or(0, |stats| stats.received) - received_before;
    }
    check!(seen == 1 + u64::from(LONG), "the card took in the long frames nobody was reading");
    check!(next(&mut frames, &mut back, BURST_FRAME, taaj::ticks() + 20) == Some(BURST_LENGTH), "the short frame before them arrives");
    for number in 0..LONG_FIT {
        let answer = next(&mut frames, &mut back, BURST_FRAME, taaj::ticks() + 20);
        check!(answer == Some(FRAME_MAX) && back[15] == number, "the long frames that fit arrive, in order");
        check!((16..FRAME_MAX).all(|i| back[i] == pattern(number, i)), "and whole: both halves, the right way round");
    }
    check!(next(&mut frames, &mut back, BURST_FRAME, taaj::ticks() + 20).is_none(), "half a frame is not a frame");
    let dropped = card.stats().map_or(0, |stats| stats.dropped) - dropped_before;
    check!(dropped == u64::from(LONG - LONG_FIT), "the long frames that did not fit were dropped, and counted");
    frame(&mut out, &ours, ECHO, 70, 1500);
    check!(card.send(&out[..1500]).is_ok(), "a frame can be sent after that");
    sent += 1;
    check!(next(&mut frames, &mut back, ECHOED, taaj::ticks() + WAIT) == Some(1500), "the frame after half a frame arrives");
    check!(back[15] == 70 && (16..1500).all(|i| back[i] == out[i] ^ FLIP), "and the half is not in it");

    // The same from this side: the driver is kept busy for half a
    // second, and frames are handed over until its queue is full - in
    // the middle of a long one. This program is told (QueueFull), the
    // frames before it all go out when the driver comes back, and the
    // half is forgotten: `Nic::send` says so before the next frame.
    let mut stall = [0u8; 9];
    stall[0] = nic::STALL;
    stall[1..].copy_from_slice(&50u64.to_le_bytes());
    check!(card.raw(&stall).is_ok(), "the driver can be kept busy");
    frame(&mut out, &ours, ECHO, 100, 100);
    check!(card.send(&out[..100]).is_ok(), "a frame can be handed to a busy driver");
    let mut taken = 0u8;
    let mut outcome = Ok(());
    while outcome.is_ok() && taken < LONG {
        frame(&mut out, &ours, ECHO, taken, FRAME_MAX);
        outcome = card.send(&out[..FRAME_MAX]);
        taken += u8::from(outcome.is_ok());
    }
    check!(outcome == Err(taaj::Error::QueueFull), "a driver that is not keeping up is not waited for: its client is told");
    check!(taken == LONG_FIT, "as many frames were taken as its queue holds");
    sent += 1 + u64::from(taken);
    check!(next(&mut frames, &mut back, ECHOED, taaj::ticks() + WAIT) == Some(100) && back[15] == 100, "the frames handed over meanwhile go out afterwards");
    for number in 0..taken {
        let answer = next(&mut frames, &mut back, ECHOED, taaj::ticks() + WAIT);
        check!(answer == Some(FRAME_MAX) && back[15] == number, "all of them, in order");
        check!((16..FRAME_MAX).all(|i| back[i] == pattern(number, i - 14) ^ FLIP), "and whole");
    }
    frame(&mut out, &ours, ECHO, 101, 300);
    check!(card.send(&out[..300]).is_ok(), "a frame can be sent after one was given up half-way");
    sent += 1;
    check!(next(&mut frames, &mut back, ECHOED, taaj::ticks() + WAIT) == Some(300), "and it goes out alone, without the half before it");
    check!(back[15] == 101 && (16..300).all(|i| back[i] == out[i] ^ FLIP), "and whole");

    // A second listener takes over: the first hears no more.
    let Ok(mut second) = card.listen(&[u16::from_be_bytes(TYPE)]) else {
        println!("[niccheck] FAILED: cannot listen a second time");
        return line!() as i64;
    };
    frame(&mut out, &ours, ECHO, 60, 200);
    check!(card.send(&out[..200]).is_ok(), "a frame can be sent with a new listener");
    sent += 1;
    check!(next(&mut second, &mut back, ECHOED, taaj::ticks() + WAIT) == Some(200), "the new listener gets the answer");
    check!(
        frames.next_until(&mut back, taaj::ticks() + 10) == Err(taaj::Error::PeerClosed),
        "the old listener is told that it is one no longer"
    );

    // The counters add up.
    let Ok(after) = card.stats() else {
        println!("[niccheck] FAILED: the driver stopped counting");
        return line!() as i64;
    };
    let delta = Stats {
        received: after.received - before.received,
        delivered: after.delivered - before.delivered,
        not_ours: after.not_ours - before.not_ours,
        dropped: after.dropped - before.dropped,
        sent: after.sent - before.sent,
        refused: after.refused - before.refused,
    };
    check!(delta.sent == sent, "every frame handed over and not refused was sent");
    check!(delta.received == delta.delivered + delta.not_ours + delta.dropped, "every frame received was passed on, kept back or dropped");

    println!(
        "[niccheck] {checks} checks, all passed: card {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}, {} frames out, {} in ({} passed on, {} for others, {} dropped)",
        ours[0], ours[1], ours[2], ours[3], ours[4], ours[5], delta.sent, delta.received, delta.delivered, delta.not_ours, delta.dropped
    );
    0
}
