//! Intel's gigabit Ethernet cards, the e1000 family: how one sends and
//! receives frames.
//!
//! The 82540EM (2002) is the card QEMU and VirtualBox give a guest by
//! default (`-device e1000`). Most drivers' first network card is this
//! one, and later Intel cards - the 82574L (`-device e1000e`), and many
//! a PC's onboard Ethernet - keep its registers and its descriptors as
//! their "legacy" mode. One driver for all of them is this crate.
//!
//! How it works:
//!
//!   - Registers in memory (BAR 0, 128 KiB): control and status, the
//!     interrupt cause and mask, and two descriptor rings' places.
//!   - A **descriptor ring** in memory for each direction: an array of
//!     16-byte descriptors, each naming one buffer, and two indices in
//!     registers - the card's *head* and the driver's *tail*.
//!       - Receive: the descriptors from head up to (not including)
//!         tail are the card's, empty buffers it may fill. It fills the
//!         one at head, writes the frame's length and "done" (DD) into
//!         the descriptor, and moves head on. The driver takes done
//!         descriptors in order and gives each back by moving tail.
//!       - Transmit: the driver writes a descriptor for each frame at
//!         tail and moves tail on; the card sends from head to tail,
//!         marks each done, moves head on.
//!
//!     Both are rings: after the last descriptor comes the first. One
//!     descriptor is always left empty - head == tail must mean "none
//!     are the card's", never "all are".
//!   - The card checks every frame's destination against its own
//!     address (RAL/RAH, loaded from its EEPROM at reset), so it does not
//!     hand over frames for others - unlike a virtio card, which hands
//!     over everything on the wire.
//!   - The interrupt cause register (ICR) says why the card interrupted,
//!     and reading it clears it. The mask (IMS to set bits, IMC to clear
//!     them) says which causes interrupt at all.
//!
//! This crate is everything that is not a system call: the registers,
//! the descriptors, and `Card` - resetting a card, setting it up,
//! sending and receiving - written against `Bus`, so all of it is tested
//! on the host against a simulated card (`tests.rs`). The program that
//! drives a real one is `user/src/bin/e1000.rs`.

#![cfg_attr(not(test), no_std)]

use core::sync::atomic::{fence, Ordering};

#[cfg(test)]
mod tests;

/// Intel's PCI vendor number.
pub const VENDOR: u16 = 0x8086;

/// The cards driven here: (PCI device number, model). These are the
/// ones QEMU has, so the ones tested; other 8254x cards are the same
/// to their legacy registers, and can be added once one has been seen.
pub const MODELS: [(u16, &str); 4] = [(0x100E, "82540EM"), (0x100F, "82545EM"), (0x1004, "82543GC"), (0x10D3, "82574L")];

/// The model of a card with these PCI numbers, if it is one of `MODELS`.
pub fn model(vendor: u16, device: u16) -> Option<&'static str> {
    MODELS.iter().find(|&&(id, _)| vendor == VENDOR && id == device).map(|&(_, name)| name)
}

/// The registers: offsets into BAR 0.
pub mod reg {
    /// Device control: reset, link.
    pub const CTRL: usize = 0x0000;
    /// Device status: link up, speed.
    pub const STATUS: usize = 0x0008;
    /// Interrupt cause (read clears it), mask set, mask clear.
    pub const ICR: usize = 0x00C0;
    pub const IMS: usize = 0x00D0;
    pub const IMC: usize = 0x00D8;
    /// Receive control.
    pub const RCTL: usize = 0x0100;
    /// Transmit control, and the gaps between frames.
    pub const TCTL: usize = 0x0400;
    pub const TIPG: usize = 0x0410;
    /// The receive ring: its address (low, high), length in bytes, head,
    /// tail.
    pub const RDBAL: usize = 0x2800;
    pub const RDBAH: usize = 0x2804;
    pub const RDLEN: usize = 0x2808;
    pub const RDH: usize = 0x2810;
    pub const RDT: usize = 0x2818;
    /// The transmit ring, the same way.
    pub const TDBAL: usize = 0x3800;
    pub const TDBAH: usize = 0x3804;
    pub const TDLEN: usize = 0x3808;
    pub const TDH: usize = 0x3810;
    pub const TDT: usize = 0x3818;
    /// The multicast table: 128 words, a bit per hash of an address.
    pub const MTA: usize = 0x5200;
    pub const MTA_WORDS: usize = 128;
    /// The card's own address: the low four bytes, then the high two
    /// and "address valid" (bit 31).
    pub const RAL0: usize = 0x5400;
    pub const RAH0: usize = 0x5404;
    pub const RAH_VALID: u32 = 1 << 31;

    /// CTRL: auto speed detection, set link up, reset (clears itself),
    /// and three the driver must leave clear: link reset, "invert loss
    /// of signal", VLAN mode, PHY reset.
    pub const CTRL_ASDE: u32 = 1 << 5;
    pub const CTRL_SLU: u32 = 1 << 6;
    pub const CTRL_RST: u32 = 1 << 26;
    pub const CTRL_LRST: u32 = 1 << 3;
    pub const CTRL_ILOS: u32 = 1 << 7;
    pub const CTRL_VME: u32 = 1 << 30;
    pub const CTRL_PHY_RST: u32 = 1 << 31;
    /// STATUS: link up.
    pub const STATUS_LU: u32 = 1 << 1;

    /// RCTL: receiver on, take broadcasts, strip the CRC. The buffer
    /// size bits (17:16) are left 0: 2048 bytes. Promiscuous modes off -
    /// only frames for this card's address, and broadcasts.
    pub const RCTL_EN: u32 = 1 << 1;
    pub const RCTL_BAM: u32 = 1 << 15;
    pub const RCTL_SECRC: u32 = 1 << 26;

    /// TCTL: transmitter on, pad short frames; the collision threshold
    /// (bits 11:4) and distance (21:12) the manual gives for full duplex.
    pub const TCTL_EN: u32 = 1 << 1;
    pub const TCTL_PSP: u32 = 1 << 3;
    pub const TCTL_CT: u32 = 0x0F << 4;
    pub const TCTL_COLD: u32 = 0x40 << 12;
    /// TIPG: the manual's gaps for copper (IPGT 10, IPGR1 8, IPGR2 6).
    pub const TIPG_COPPER: u32 = 10 | 8 << 10 | 6 << 20;

    /// Interrupt causes: a frame sent (descriptor written back), the
    /// link changed, receive descriptors running low, receive overrun
    /// (a frame lost: no descriptor), a frame received.
    pub const INT_TXDW: u32 = 1 << 0;
    pub const INT_LSC: u32 = 1 << 2;
    pub const INT_RXDMT0: u32 = 1 << 4;
    pub const INT_RXO: u32 = 1 << 6;
    pub const INT_RXT0: u32 = 1 << 7;
    /// The ones the driver wants to hear about.
    pub const INTERRUPTS: u32 = INT_TXDW | INT_LSC | INT_RXDMT0 | INT_RXO | INT_RXT0;
}

/// A descriptor's fields: offsets inside its 16 bytes, and bits.
pub mod desc {
    pub const SIZE: usize = 16;
    /// Both kinds: the buffer's physical address (8 bytes), its length
    /// (2: the frame's, written by the card on receive, by the driver
    /// on transmit).
    pub const ADDRESS: usize = 0;
    pub const LENGTH: usize = 8;
    /// Receive: status, errors.
    pub const RX_STATUS: usize = 12;
    pub const RX_ERRORS: usize = 13;
    /// Transmit: the command, and the status the card writes back.
    pub const TX_COMMAND: usize = 11;
    pub const TX_STATUS: usize = 12;

    /// Status: done (the card has finished with it), end of packet (the
    /// last buffer of a frame).
    pub const DD: u8 = 1 << 0;
    pub const EOP: u8 = 1 << 1;
    /// Transmit command: end of packet, insert the CRC, report status
    /// (write DD back when sent).
    pub const CMD_EOP: u8 = 1 << 0;
    pub const CMD_IFCS: u8 = 1 << 1;
    pub const CMD_RS: u8 = 1 << 3;
}

/// The rings' sizes. A ring's length in bytes must be a multiple of 128:
/// eight descriptors.
pub const RX_DESCRIPTORS: usize = 32;
pub const TX_DESCRIPTORS: usize = 16;
/// One buffer per descriptor, of 2048 bytes: a whole frame (1518 at
/// most, without the CRC the card strips) fits one.
pub const BUFFER: usize = 2048;
/// The shortest frame on the wire, without its CRC; a shorter one is
/// filled up with zeroes.
pub const WIRE_MIN: usize = 60;

/// Where everything lives in the driver's DMA memory.
pub mod layout {
    use super::{BUFFER, RX_DESCRIPTORS, TX_DESCRIPTORS};
    pub const PAGE: usize = 4096;
    pub const RX_RING: usize = 0x0000;
    pub const TX_RING: usize = 0x1000;
    pub const RX_BUFFERS: usize = 0x2000;
    pub const TX_BUFFERS: usize = RX_BUFFERS + RX_DESCRIPTORS * BUFFER;
    /// How much DMA memory the driver needs.
    pub const NEEDED: usize = TX_BUFFERS + TX_DESCRIPTORS * BUFFER;
}

const _: () = assert!(RX_DESCRIPTORS * desc::SIZE <= layout::TX_RING && TX_DESCRIPTORS * desc::SIZE <= layout::PAGE);
const _: () = assert!((RX_DESCRIPTORS * desc::SIZE).is_multiple_of(128) && (TX_DESCRIPTORS * desc::SIZE).is_multiple_of(128));

/// The card as the driver reaches it: its registers, and a way to wait.
pub trait Bus {
    fn read(&mut self, register: usize) -> u32;
    fn write(&mut self, register: usize, value: u32);
    /// Waits a little (a tick, or less).
    fn pause(&mut self);
    /// The time, in ticks (100 a second).
    fn ticks(&mut self) -> u64;
}

/// How long a reset may take: 1 s (the manual says microseconds).
pub const RESET_WAIT: u64 = 100;

/// Memory both the driver and the card use: where it is mapped here, and
/// where it is for the card.
pub struct Dma {
    base: *mut u8,
    physical: u64,
    size: usize,
}

impl Dma {
    /// # Safety
    /// `base` must be a mapping of `size` bytes, writable, that nothing
    /// else uses for as long as the `Dma` lives; `physical` is where
    /// those bytes are for the card (one unbroken range).
    pub unsafe fn new(base: *mut u8, physical: u64, size: usize) -> Dma {
        Dma { base, physical, size }
    }

    fn physical(&self, offset: usize) -> u64 {
        self.physical + offset as u64
    }

    /// Volatile, every access: the card reads and writes this memory
    /// behind the compiler's back.
    fn write8(&mut self, offset: usize, value: u8) {
        assert!(offset < self.size);
        // SAFETY: inside the mapping (checked above), which is ours.
        unsafe { self.base.add(offset).write_volatile(value) }
    }

    fn read8(&self, offset: usize) -> u8 {
        assert!(offset < self.size);
        // SAFETY: as in `write8`.
        unsafe { self.base.add(offset).read_volatile() }
    }

    fn write_bytes(&mut self, offset: usize, bytes: &[u8]) {
        for (n, &byte) in bytes.iter().enumerate() {
            self.write8(offset + n, byte);
        }
    }

    fn read16(&self, offset: usize) -> u16 {
        u16::from_le_bytes([self.read8(offset), self.read8(offset + 1)])
    }

    /// A buffer the card has handed back: plain memory again until it is
    /// given back, so read as a slice.
    fn slice(&self, offset: usize, length: usize) -> &[u8] {
        assert!(offset + length <= self.size);
        // SAFETY: inside the mapping (checked above); the card does not
        // write this buffer until the driver gives it back, which needs
        // `&mut self` - so not while the slice lives.
        unsafe { core::slice::from_raw_parts(self.base.add(offset), length) }
    }
}

/// An Ethernet card of the e1000 family, set up and running.
pub struct Card<B: Bus> {
    bus: B,
    dma: Dma,
    /// The next receive descriptor the card will fill.
    rx_next: usize,
    /// Throwing away the rest of a frame that did not fit one buffer.
    rx_discarding: bool,
    /// The next transmit descriptor to fill, and the oldest the card
    /// has not finished with.
    tx_tail: usize,
    tx_clean: usize,
    pub address: [u8; 6],
}

impl<B: Bus> Card<B> {
    /// Takes the card over:
    ///
    ///   1. reset it - whatever the firmware, or a driver before this
    ///      one, left in it (rings, interrupts) is forgotten - and mask
    ///      every interrupt;
    ///   2. read its address, which the reset loaded from the EEPROM;
    ///   3. link up; no multicast;
    ///   4. the receive ring, every descriptor with an empty buffer, all
    ///      but one of them the card's; then the receiver on;
    ///   5. the transmit ring, empty; then the transmitter on.
    pub fn start(bus: B, dma: Dma) -> Result<Card<B>, &'static str> {
        if dma.size < layout::NEEDED || !dma.physical.is_multiple_of(layout::PAGE as u64) {
            return Err("too little DMA memory, or not page-aligned");
        }
        let mut card = Card { bus, dma, rx_next: 0, rx_discarding: false, tx_tail: 0, tx_clean: 0, address: [0; 6] };
        let bus = &mut card.bus;

        let control = bus.read(reg::CTRL);
        bus.write(reg::CTRL, control | reg::CTRL_RST);
        let deadline = bus.ticks() + RESET_WAIT;
        while bus.read(reg::CTRL) & reg::CTRL_RST != 0 {
            if bus.ticks() >= deadline {
                return Err("the card did not come out of its reset");
            }
            bus.pause();
        }
        bus.write(reg::IMC, u32::MAX);
        let _ = bus.read(reg::ICR);

        let (low, high) = (bus.read(reg::RAL0), bus.read(reg::RAH0));
        if high & reg::RAH_VALID == 0 || (low == 0 && high & 0xFFFF == 0) {
            return Err("the card has no Ethernet address (its EEPROM gave none)");
        }
        let low = low.to_le_bytes();
        let high = high.to_le_bytes();
        card.address = [low[0], low[1], low[2], low[3], high[0], high[1]];

        let control = bus.read(reg::CTRL);
        let clear = reg::CTRL_LRST | reg::CTRL_ILOS | reg::CTRL_VME | reg::CTRL_PHY_RST;
        bus.write(reg::CTRL, (control & !clear) | reg::CTRL_SLU | reg::CTRL_ASDE);
        for word in 0..reg::MTA_WORDS {
            bus.write(reg::MTA + 4 * word, 0);
        }

        // Receive: every descriptor names its buffer, status clear.
        for n in 0..RX_DESCRIPTORS {
            let at = layout::RX_RING + n * desc::SIZE;
            let buffer = card.dma.physical(layout::RX_BUFFERS + n * BUFFER);
            card.dma.write_bytes(at, &buffer.to_le_bytes());
            card.dma.write_bytes(at + 8, &[0; 8]);
        }
        // Transmit: empty, but "done" - free.
        for n in 0..TX_DESCRIPTORS {
            card.dma.write_bytes(layout::TX_RING + n * desc::SIZE, &[0; 16]);
        }
        fence(Ordering::SeqCst);
        let bus = &mut card.bus;
        let rx = card.dma.physical(layout::RX_RING);
        bus.write(reg::RDBAL, rx as u32);
        bus.write(reg::RDBAH, (rx >> 32) as u32);
        bus.write(reg::RDLEN, (RX_DESCRIPTORS * desc::SIZE) as u32);
        bus.write(reg::RDH, 0);
        // All but one: the one before head stays the driver's.
        bus.write(reg::RDT, (RX_DESCRIPTORS - 1) as u32);
        bus.write(reg::RCTL, reg::RCTL_EN | reg::RCTL_BAM | reg::RCTL_SECRC);

        let tx = card.dma.physical(layout::TX_RING);
        bus.write(reg::TDBAL, tx as u32);
        bus.write(reg::TDBAH, (tx >> 32) as u32);
        bus.write(reg::TDLEN, (TX_DESCRIPTORS * desc::SIZE) as u32);
        bus.write(reg::TDH, 0);
        bus.write(reg::TDT, 0);
        bus.write(reg::TIPG, reg::TIPG_COPPER);
        bus.write(reg::TCTL, reg::TCTL_EN | reg::TCTL_PSP | reg::TCTL_CT | reg::TCTL_COLD);
        Ok(card)
    }

    /// Lets the card interrupt, for the causes the driver handles.
    pub fn enable_interrupts(&mut self) {
        self.bus.write(reg::IMS, reg::INTERRUPTS);
    }

    /// After an interrupt: why it came - and reading it is what tells
    /// the card it was seen.
    pub fn interrupt_seen(&mut self) -> u32 {
        self.bus.read(reg::ICR)
    }

    pub fn link_up(&mut self) -> bool {
        self.bus.read(reg::STATUS) & reg::STATUS_LU != 0
    }

    /// Every frame the card has received since the last look, in order,
    /// to `pass`: the frame, or None for one that arrived damaged (the
    /// card found an error in it) or did not fit a buffer. Each buffer
    /// is given back to the card after `pass` has seen it. Returns how
    /// many descriptors were taken: at most a ring's worth, so that a
    /// card that keeps filling them (or one that misbehaves) cannot keep
    /// the driver here for ever - the rest are taken on the next call.
    pub fn receive(&mut self, mut pass: impl FnMut(Option<&[u8]>)) -> usize {
        let mut taken = 0;
        while taken < RX_DESCRIPTORS {
            let at = layout::RX_RING + self.rx_next * desc::SIZE;
            let status = self.dma.read8(at + desc::RX_STATUS);
            if status & desc::DD == 0 {
                break;
            }
            // The rest of the descriptor, and the buffer, are read only
            // after DD was seen.
            fence(Ordering::Acquire);
            let length = usize::from(self.dma.read16(at + desc::LENGTH)).min(BUFFER);
            let errors = self.dma.read8(at + desc::RX_ERRORS);
            let whole = status & desc::EOP != 0;
            if whole && !self.rx_discarding && errors == 0 {
                pass(Some(self.dma.slice(layout::RX_BUFFERS + self.rx_next * BUFFER, length)));
            } else if whole {
                // The last piece of a frame too big for a buffer, or a
                // damaged one: one frame lost.
                pass(None);
            }
            // Not the end of a frame: its other pieces follow.
            self.rx_discarding = !whole;

            // Given back: status cleared, then it is the card's again.
            self.dma.write8(at + desc::RX_STATUS, 0);
            fence(Ordering::Release);
            self.bus.write(reg::RDT, self.rx_next as u32);
            self.rx_next = (self.rx_next + 1) % RX_DESCRIPTORS;
            taken += 1;
        }
        taken
    }

    /// Takes back the transmit descriptors the card has finished with.
    pub fn take_sent(&mut self) {
        while self.tx_clean != self.tx_tail {
            let at = layout::TX_RING + self.tx_clean * desc::SIZE;
            if self.dma.read8(at + desc::TX_STATUS) & desc::DD == 0 {
                break;
            }
            self.tx_clean = (self.tx_clean + 1) % TX_DESCRIPTORS;
        }
    }

    /// True if every transmit descriptor is the card's: no more can be
    /// sent until it finishes one.
    pub fn transmit_full(&mut self) -> bool {
        self.take_sent();
        (self.tx_tail + 1) % TX_DESCRIPTORS == self.tx_clean
    }

    /// Puts `frame` (without its CRC: the card adds it) on the wire.
    /// False if it is too big, or the card has no room.
    pub fn send(&mut self, frame: &[u8]) -> bool {
        if frame.len() > BUFFER || self.transmit_full() {
            return false;
        }
        let n = self.tx_tail;
        let buffer = layout::TX_BUFFERS + n * BUFFER;
        let length = frame.len().max(WIRE_MIN);
        self.dma.write_bytes(buffer, frame);
        for at in frame.len()..length {
            self.dma.write8(buffer + at, 0);
        }
        let at = layout::TX_RING + n * desc::SIZE;
        let physical = self.dma.physical(buffer);
        self.dma.write_bytes(at, &physical.to_le_bytes());
        self.dma.write_bytes(at + desc::LENGTH, &(length as u16).to_le_bytes());
        // CSO (10), the command (11), status (12), CSS (13), special (14-15).
        self.dma.write_bytes(at + 10, &[0, desc::CMD_EOP | desc::CMD_IFCS | desc::CMD_RS, 0, 0, 0, 0]);
        // The descriptor and the frame are in memory before the card is
        // told.
        fence(Ordering::SeqCst);
        self.tx_tail = (n + 1) % TX_DESCRIPTORS;
        self.bus.write(reg::TDT, self.tx_tail as u32);
        true
    }
}
