//! The driver's sequence, run against a simulated e1000.
//!
//! `Sim` is a card as the manual describes it: a reset forgets the
//! rings and reloads the address from the EEPROM; the tail registers
//! hand descriptors over, and the card works between its head and the
//! driver's tail - sending what the transmit descriptors name, filling
//! receive buffers with frames from "the wire" (`arrive`). It checks the
//! rules as it goes (rings set up and switched on before use, sizes, the
//! transmit command bits) and panics at the first one the driver breaks.

use super::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::vec::Vec;

const PHYSICAL: u64 = 0x0080_0000;
const MAC: [u8; 6] = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];

#[derive(Default)]
struct State {
    registers: HashMap<usize, u32>,
    memory: Vec<u8>,
    time: u64,
    /// The address the EEPROM holds; None: an empty EEPROM.
    eeprom: Option<[u8; 6]>,
    /// Ticks a reset takes; u64::MAX: it never ends.
    reset_ticks: u64,
    reset_until: Option<u64>,
    resets: u32,
    /// Frames sent, as they left (padded, without the CRC).
    wire: Vec<Vec<u8>>,
    /// The card does not send: transmit descriptors stay its own.
    hold_transmit: bool,
    /// Frames that found no free receive descriptor.
    missed: u32,
    /// Frames bigger than one buffer are split across descriptors.
    split_big: bool,
}

#[derive(Clone)]
struct Sim(Rc<RefCell<State>>);

impl State {
    fn register(&self, register: usize) -> u32 {
        self.registers.get(&register).copied().unwrap_or(0)
    }

    fn at(&mut self, physical: u64, length: usize) -> &mut [u8] {
        let start = physical.checked_sub(PHYSICAL).expect("an address below the DMA memory") as usize;
        assert!(start + length <= self.memory.len(), "an address past the DMA memory: {physical:#x}+{length}");
        &mut self.memory[start..start + length]
    }

    fn ring(&self, low: usize, high: usize, length: usize) -> (u64, usize) {
        let base = u64::from(self.register(low)) | u64::from(self.register(high)) << 32;
        let bytes = self.register(length) as usize;
        assert!(base.is_multiple_of(16), "a ring not on 16 bytes");
        assert!(bytes > 0 && bytes.is_multiple_of(128), "a ring length that is not a multiple of 128: {bytes}");
        (base, bytes / desc::SIZE)
    }

    fn reset(&mut self) {
        self.resets += 1;
        self.registers.clear();
        if let Some(mac) = self.eeprom {
            self.registers.insert(reg::RAL0, u32::from_le_bytes([mac[0], mac[1], mac[2], mac[3]]));
            self.registers.insert(reg::RAH0, u32::from(mac[4]) | u32::from(mac[5]) << 8 | reg::RAH_VALID);
        }
        // Link up after reset, as QEMU's card is.
        self.registers.insert(reg::STATUS, reg::STATUS_LU);
        self.reset_until = Some(self.time.saturating_add(self.reset_ticks));
    }

    fn cause(&mut self, why: u32) {
        let icr = self.register(reg::ICR) | why;
        self.registers.insert(reg::ICR, icr);
    }

    /// Sends what the transmit descriptors from head to tail name.
    fn transmit(&mut self) {
        if self.hold_transmit {
            return;
        }
        let tail = self.register(reg::TDT) as usize;
        let mut head = self.register(reg::TDH) as usize;
        if head == tail {
            return; // nothing handed over (the tail set to 0 at setup)
        }
        assert!(self.register(reg::TCTL) & reg::TCTL_EN != 0, "frames given before the transmitter was on");
        let (base, count) = self.ring(reg::TDBAL, reg::TDBAH, reg::TDLEN);
        assert!(tail < count, "a transmit tail past the ring");
        while head != tail {
            let d = base + (head * desc::SIZE) as u64;
            let descriptor = self.at(d, 16).to_vec();
            let address = u64::from_le_bytes(descriptor[0..8].try_into().unwrap());
            let length = usize::from(u16::from_le_bytes([descriptor[8], descriptor[9]]));
            let command = descriptor[desc::TX_COMMAND];
            assert!(command & desc::CMD_EOP != 0, "a frame in more than one descriptor");
            assert!(command & desc::CMD_IFCS != 0, "a frame sent without its CRC");
            assert!(command & desc::CMD_RS != 0, "no status asked for: the driver would never know it was sent");
            assert!((WIRE_MIN..=1514).contains(&length), "a frame of {length} bytes on the wire");
            let frame = self.at(address, length).to_vec();
            self.wire.push(frame);
            self.at(d + desc::TX_STATUS as u64, 1)[0] |= desc::DD;
            head = (head + 1) % count;
        }
        self.registers.insert(reg::TDH, head as u32);
        self.cause(reg::INT_TXDW);
    }

    /// A frame from the wire: filtered as the card filters, then put in
    /// the next receive buffer (or more than one, when `split_big`).
    fn arrive(&mut self, frame: &[u8]) {
        let rctl = self.register(reg::RCTL);
        if rctl & reg::RCTL_EN == 0 {
            return;
        }
        let ours = frame[..6] == MAC || (frame[..6] == [0xFF; 6] && rctl & reg::RCTL_BAM != 0);
        if !ours {
            return;
        }
        // The CRC is on the wire; the card keeps it unless told to strip.
        let mut bytes = frame.to_vec();
        if rctl & reg::RCTL_SECRC == 0 {
            bytes.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
        }
        let (base, count) = self.ring(reg::RDBAL, reg::RDBAH, reg::RDLEN);
        let pieces: Vec<Vec<u8>> = if self.split_big { bytes.chunks(BUFFER).map(<[u8]>::to_vec).collect() } else { std::vec![bytes] };
        // All the descriptors it needs, or the frame is missed.
        let head = self.register(reg::RDH) as usize;
        let tail = self.register(reg::RDT) as usize;
        let free = (tail + count - head) % count;
        if free < pieces.len() {
            self.missed += 1;
            self.cause(reg::INT_RXO);
            return;
        }
        let mut head = head;
        for (n, piece) in pieces.iter().enumerate() {
            assert!(piece.len() <= BUFFER, "a frame bigger than the buffer size the driver set");
            let d = base + (head * desc::SIZE) as u64;
            let address = u64::from_le_bytes(self.at(d, 8).try_into().unwrap());
            self.at(address, piece.len()).copy_from_slice(piece);
            let descriptor = self.at(d + 8, 8);
            descriptor[0..2].copy_from_slice(&(piece.len() as u16).to_le_bytes());
            descriptor[4] = desc::DD | if n + 1 == pieces.len() { desc::EOP } else { 0 };
            descriptor[5] = 0;
            head = (head + 1) % count;
        }
        self.registers.insert(reg::RDH, head as u32);
        self.cause(reg::INT_RXT0);
    }
}

impl Bus for Sim {
    fn read(&mut self, register: usize) -> u32 {
        let mut s = self.0.borrow_mut();
        if register == reg::CTRL && s.reset_until.is_some_and(|until| s.time >= until) {
            let control = s.register(reg::CTRL) & !reg::CTRL_RST;
            s.registers.insert(reg::CTRL, control);
            s.reset_until = None;
        }
        let value = s.register(register);
        if register == reg::ICR {
            s.registers.insert(reg::ICR, 0);
        }
        value
    }

    fn write(&mut self, register: usize, value: u32) {
        let mut s = self.0.borrow_mut();
        assert!(s.reset_until.is_none() || register == reg::CTRL, "register {register:#x} written during the reset");
        match register {
            reg::CTRL if value & reg::CTRL_RST != 0 => {
                s.reset();
                s.registers.insert(reg::CTRL, value);
            }
            reg::IMS => {
                let mask = s.register(reg::IMS) | value;
                s.registers.insert(reg::IMS, mask);
            }
            reg::IMC => {
                let mask = s.register(reg::IMS) & !value;
                s.registers.insert(reg::IMS, mask);
            }
            reg::RDT => {
                let (_, count) = s.ring(reg::RDBAL, reg::RDBAH, reg::RDLEN);
                assert!((value as usize) < count, "a receive tail past the ring");
                s.registers.insert(reg::RDT, value);
            }
            reg::TDT => {
                s.registers.insert(reg::TDT, value);
                s.transmit();
            }
            _ => {
                s.registers.insert(register, value);
            }
        }
    }

    fn pause(&mut self) {
        self.0.borrow_mut().time += 1;
    }

    fn ticks(&mut self) -> u64 {
        self.0.borrow().time
    }
}

fn sim() -> Sim {
    Sim(Rc::new(RefCell::new(State { memory: std::vec![0u8; layout::NEEDED], eeprom: Some(MAC), reset_ticks: 2, ..State::default() })))
}

fn start(sim: &Sim) -> Result<Card<Sim>, &'static str> {
    let base = sim.0.borrow_mut().memory.as_mut_ptr();
    // SAFETY: the simulator's memory, which lives as long as `sim`, the
    // card's only other user.
    let dma = unsafe { Dma::new(base, PHYSICAL, layout::NEEDED) };
    Card::start(sim.clone(), dma)
}

/// A frame for `to`, `length` bytes, its payload counting from `seed`.
fn frame(to: [u8; 6], length: usize, seed: u8) -> Vec<u8> {
    let mut f = to.to_vec();
    f.extend_from_slice(&[0x02, 0, 0, 0, 0, 1, 0x88, 0xB5]);
    f.extend((0..length - 14).map(|n| seed.wrapping_add(n as u8)));
    f
}

fn received(card: &mut Card<Sim>) -> Vec<Option<Vec<u8>>> {
    let mut out = Vec::new();
    card.receive(|frame| out.push(frame.map(<[u8]>::to_vec)));
    out
}

#[test]
fn the_models_are_known_by_their_numbers() {
    assert_eq!(model(0x8086, 0x100E), Some("82540EM"));
    assert_eq!(model(0x8086, 0x10D3), Some("82574L"));
    assert_eq!(model(0x8086, 0x2922), None, "an AHCI controller");
    assert_eq!(model(0x1AF4, 0x100E), None, "another vendor's");
}

#[test]
fn starting_resets_the_card_reads_its_address_and_switches_it_on() {
    let sim = sim();
    // What a driver before this one left: interrupts on, a ring.
    sim.0.borrow_mut().registers.insert(reg::IMS, reg::INTERRUPTS);
    let mut card = start(&sim).expect("the card starts");
    let s = sim.0.borrow();
    assert_eq!(s.resets, 1);
    assert_eq!(card.address, MAC);
    assert_eq!(s.register(reg::IMS), 0, "every interrupt masked until the driver wants them");
    let control = s.register(reg::CTRL);
    assert!(control & reg::CTRL_SLU != 0, "link up");
    assert_eq!(control & (reg::CTRL_RST | reg::CTRL_PHY_RST | reg::CTRL_LRST | reg::CTRL_VME | reg::CTRL_ILOS), 0);
    assert_eq!(s.register(reg::RCTL), reg::RCTL_EN | reg::RCTL_BAM | reg::RCTL_SECRC);
    assert_eq!(s.register(reg::TCTL) & (reg::TCTL_EN | reg::TCTL_PSP), reg::TCTL_EN | reg::TCTL_PSP);
    assert_eq!(s.ring(reg::RDBAL, reg::RDBAH, reg::RDLEN), (PHYSICAL + layout::RX_RING as u64, RX_DESCRIPTORS));
    assert_eq!(s.ring(reg::TDBAL, reg::TDBAH, reg::TDLEN), (PHYSICAL + layout::TX_RING as u64, TX_DESCRIPTORS));
    assert_eq!(s.register(reg::RDT) as usize, RX_DESCRIPTORS - 1, "all receive descriptors but one are the card's");
    assert_eq!((s.register(reg::TDH), s.register(reg::TDT)), (0, 0));
    assert!((0..reg::MTA_WORDS).all(|n| s.register(reg::MTA + 4 * n) == 0));
    drop(s);
    assert!(card.link_up());
    card.enable_interrupts();
    assert_eq!(sim.0.borrow().register(reg::IMS), reg::INTERRUPTS);
}

#[test]
fn a_card_that_is_not_ready_is_refused() {
    let sim = sim();
    sim.0.borrow_mut().eeprom = None;
    assert_eq!(start(&sim).err(), Some("the card has no Ethernet address (its EEPROM gave none)"));
    let sim = self::sim();
    sim.0.borrow_mut().reset_ticks = u64::MAX;
    assert_eq!(start(&sim).err(), Some("the card did not come out of its reset"));
    assert!(sim.0.borrow().time >= RESET_WAIT);
}

#[test]
fn frames_are_received_in_order_and_buffers_go_back() {
    let sim = sim();
    let mut card = start(&sim).unwrap();
    // Five rounds of twenty: round the ring three times.
    for round in 0..5u8 {
        for n in 0..20u8 {
            sim.0.borrow_mut().arrive(&frame(MAC, 60 + usize::from(n) * 70, round * 20 + n));
        }
        let got = received(&mut card);
        assert_eq!(got.len(), 20);
        for (n, f) in got.into_iter().enumerate() {
            assert_eq!(f, Some(frame(MAC, 60 + n * 70, round * 20 + n as u8)), "round {round}, frame {n}");
        }
    }
    assert_eq!(sim.0.borrow().missed, 0);
    assert!(received(&mut card).is_empty(), "nothing new");
    // Broadcasts come too; frames for others the card keeps back.
    sim.0.borrow_mut().arrive(&frame([0xFF; 6], 60, 1));
    sim.0.borrow_mut().arrive(&frame([0x52, 0x54, 0, 0, 0, 9], 60, 2));
    assert_eq!(received(&mut card), std::vec![Some(frame([0xFF; 6], 60, 1))]);
}

#[test]
fn a_full_ring_loses_frames_until_the_driver_looks() {
    let sim = sim();
    let mut card = start(&sim).unwrap();
    for n in 0..40u8 {
        sim.0.borrow_mut().arrive(&frame(MAC, 100, n));
    }
    assert_eq!(sim.0.borrow().missed, 40 - (RX_DESCRIPTORS as u32 - 1), "one descriptor is always the driver's");
    let got = received(&mut card);
    assert_eq!(got.len(), RX_DESCRIPTORS - 1);
    assert_eq!(got[0], Some(frame(MAC, 100, 0)));
    // Every buffer is the card's again.
    sim.0.borrow_mut().arrive(&frame(MAC, 100, 99));
    assert_eq!(received(&mut card), std::vec![Some(frame(MAC, 100, 99))]);
}

#[test]
fn damaged_and_oversized_frames_are_dropped_whole() {
    let sim = sim();
    let mut card = start(&sim).unwrap();
    // A frame the card found an error in (a bad CRC, say).
    sim.0.borrow_mut().arrive(&frame(MAC, 80, 1));
    let at = PHYSICAL + layout::RX_RING as u64 + desc::RX_ERRORS as u64;
    sim.0.borrow_mut().at(at, 1)[0] = 0x01;
    // One too big for a buffer: in two descriptors.
    sim.0.borrow_mut().split_big = true;
    sim.0.borrow_mut().arrive(&frame(MAC, 3000, 2));
    sim.0.borrow_mut().split_big = false;
    sim.0.borrow_mut().arrive(&frame(MAC, 90, 3));
    assert_eq!(received(&mut card), std::vec![None, None, Some(frame(MAC, 90, 3))]);
}

#[test]
fn frames_are_sent_padded_and_in_order() {
    let sim = sim();
    let mut card = start(&sim).unwrap();
    for n in 0..50u8 {
        assert!(card.send(&frame([0xFF; 6], 100 + usize::from(n), n)), "frame {n}");
    }
    let short = frame([0xFF; 6], 20, 7);
    assert!(card.send(&short));
    let s = sim.0.borrow();
    assert_eq!(s.wire.len(), 51);
    for (n, sent) in s.wire.iter().take(50).enumerate() {
        assert_eq!(sent, &frame([0xFF; 6], 100 + n, n as u8));
    }
    let mut padded = short.clone();
    padded.resize(WIRE_MIN, 0);
    assert_eq!(s.wire[50], padded, "a short frame filled up with zeroes");
    drop(s);
    assert!(!card.send(&std::vec![0u8; BUFFER + 1]), "too big");
}

#[test]
fn a_busy_card_is_waited_for() {
    let sim = sim();
    let mut card = start(&sim).unwrap();
    sim.0.borrow_mut().hold_transmit = true;
    for n in 0..TX_DESCRIPTORS - 1 {
        assert!(card.send(&frame([0xFF; 6], 60, n as u8)), "frame {n}");
    }
    assert!(card.transmit_full());
    assert!(!card.send(&frame([0xFF; 6], 60, 99)), "no room: one descriptor stays empty");
    assert!(sim.0.borrow().wire.is_empty());
    // The card catches up: the tail write makes it send what it holds.
    sim.0.borrow_mut().hold_transmit = false;
    sim.0.borrow_mut().transmit();
    assert!(!card.transmit_full());
    assert!(card.send(&frame([0xFF; 6], 60, 100)));
    let s = sim.0.borrow();
    assert_eq!(s.wire.len(), TX_DESCRIPTORS);
    assert_eq!(s.wire[TX_DESCRIPTORS - 1][14], 100);
}

#[test]
fn reading_the_cause_clears_it() {
    let sim = sim();
    let mut card = start(&sim).unwrap();
    card.enable_interrupts();
    sim.0.borrow_mut().arrive(&frame(MAC, 60, 0));
    assert!(card.send(&frame([0xFF; 6], 60, 0)));
    assert_eq!(card.interrupt_seen(), reg::INT_RXT0 | reg::INT_TXDW);
    assert_eq!(card.interrupt_seen(), 0);
}

#[test]
fn a_second_driver_starts_over() {
    let sim = sim();
    let mut first = start(&sim).unwrap();
    for n in 0..5u8 {
        sim.0.borrow_mut().arrive(&frame(MAC, 60, n));
        first.send(&frame([0xFF; 6], 60, n));
    }
    drop(first);
    // A restarted driver: the same memory, the card reset again.
    let mut second = start(&sim).unwrap();
    assert_eq!(sim.0.borrow().resets, 2);
    assert!(received(&mut second).is_empty(), "the old frames are forgotten with the old ring");
    sim.0.borrow_mut().arrive(&frame(MAC, 60, 42));
    assert_eq!(received(&mut second), std::vec![Some(frame(MAC, 60, 42))]);
    assert!(second.send(&frame([0xFF; 6], 60, 43)));
    assert_eq!(sim.0.borrow().wire.last().unwrap()[14], 43);
}
