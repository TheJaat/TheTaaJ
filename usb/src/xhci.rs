//! The xHCI host controller.
//!
//! Its registers, in memory (BAR 0), in four groups:
//!
//!   - capability registers, at 0: what the controller has - how many
//!     slots and ports, how big its contexts are, where the other
//!     groups begin;
//!   - operational registers, after them: run and stop, the command
//!     ring's and the device contexts' places, and one set per port;
//!   - runtime registers: the interrupters, each with an event ring;
//!   - doorbells: one per slot (and one for the commands) - "there is
//!     new work on a ring".
//!
//! Everything else is in memory, in **rings of TRBs** (transfer request
//! blocks, 16 bytes each). Three kinds:
//!
//!   - the **command ring**: the driver's requests to the controller -
//!     "enable a slot", "address this device", "configure these
//!     endpoints";
//!   - a **transfer ring** per endpoint: what to send or receive;
//!   - the **event ring**: the controller's answers - a command
//!     completed, a transfer completed, a port changed.
//!
//! Who owns a TRB is told by its **cycle bit**. The producer writes
//! TRBs with its current cycle; the consumer takes them while the bit
//! matches the one it expects. At the end of a ring a *Link TRB* points
//! back to its start, and both sides flip their cycle - so last time
//! round's TRBs no longer look new. The controller produces events the
//! same way, and the driver says how far it has read by writing the
//! event ring's dequeue pointer (ERDP).
//!
//! A device's state lives in its **device context**: a slot context (its
//! speed, its port, how many endpoints) and an endpoint context for each
//! endpoint (its type, its packet size, its ring). The controller keeps
//! it; the driver changes it by handing an *input context* (the same,
//! plus flags saying which parts to take) to a command.

use core::sync::atomic::{fence, Ordering};

/// The PCI class of an xHCI controller: serial bus (0C), USB (03),
/// xHCI (programming interface 30). Any vendor's.
pub const CLASS: u8 = 0x0C;
pub const SUBCLASS: u8 = 0x03;
pub const INTERFACE: u8 = 0x30;

/// Capability registers: offsets from BAR 0.
pub mod cap {
    /// CAPLENGTH (byte 0: where the operational registers begin) and
    /// HCIVERSION (bytes 2-3).
    pub const CAPLENGTH: usize = 0x00;
    /// Slots (7:0), interrupters (18:8), ports (31:24).
    pub const HCSPARAMS1: usize = 0x04;
    /// Scratchpad buffers: the high five bits in 25:21, the low five in
    /// 31:27.
    pub const HCSPARAMS2: usize = 0x08;
    /// 64-bit addresses (bit 0), 64-byte contexts (bit 2), and where the
    /// extended capabilities begin (31:16, in words).
    pub const HCCPARAMS1: usize = 0x10;
    pub const DBOFF: usize = 0x14;
    pub const RTSOFF: usize = 0x18;
}

/// Operational registers: offsets from CAPLENGTH.
pub mod op {
    pub const USBCMD: usize = 0x00;
    pub const USBSTS: usize = 0x04;
    pub const PAGESIZE: usize = 0x08;
    /// The command ring: its address, and the cycle it starts with.
    pub const CRCR: usize = 0x18;
    /// The device context base address array: where each slot's device
    /// context is.
    pub const DCBAAP: usize = 0x30;
    /// How many slots are enabled.
    pub const CONFIG: usize = 0x38;
    /// Port n's registers (n from 1): PORTSC, then three others.
    pub const fn portsc(port: u8) -> usize {
        0x400 + 0x10 * (port as usize - 1)
    }

    /// USBCMD: run, reset the controller, let it interrupt.
    pub const CMD_RUN: u32 = 1 << 0;
    pub const CMD_RESET: u32 = 1 << 1;
    pub const CMD_INTERRUPTS: u32 = 1 << 2;
    /// USBSTS: halted, an event to interrupt for (write 1 to clear),
    /// not ready yet (after a reset), a fatal error.
    pub const STS_HALTED: u32 = 1 << 0;
    pub const STS_FATAL: u32 = 1 << 2;
    pub const STS_EVENT: u32 = 1 << 3;
    pub const STS_NOT_READY: u32 = 1 << 11;
}

/// A port's status and control register.
pub mod port {
    pub const CONNECTED: u32 = 1 << 0;
    /// Enabled: writing 1 *disables* it - never written back as read.
    pub const ENABLED: u32 = 1 << 1;
    pub const RESET: u32 = 1 << 4;
    pub const POWER: u32 = 1 << 9;
    pub const SPEED_SHIFT: u32 = 10;
    /// What changed: connection, enable, warm reset, over-current,
    /// reset done, link state, config error. Each is cleared by writing
    /// it as 1.
    pub const CHANGES: u32 = 0x7F << 17;
    pub const RESET_CHANGE: u32 = 1 << 21;
    /// The bits written back as read, so as not to change them: power,
    /// the indicator, the wake enables.
    pub const KEEP: u32 = POWER | 3 << 14 | 7 << 25;
}

/// The interrupter 0's registers: offsets from the runtime registers.
pub mod int {
    /// Pending (bit 0: write 1 to clear) and enable (bit 1).
    pub const IMAN: usize = 0x20;
    pub const IMOD: usize = 0x24;
    /// The event ring segment table: its size, its address.
    pub const ERSTSZ: usize = 0x28;
    pub const ERSTBA: usize = 0x30;
    /// How far the driver has read the event ring; bit 3 (EHB, write 1
    /// to clear) says the controller may interrupt again.
    pub const ERDP: usize = 0x38;
    pub const IMAN_PENDING: u32 = 1 << 0;
    pub const IMAN_ENABLE: u32 = 1 << 1;
    pub const ERDP_BUSY: u64 = 1 << 3;
}

/// TRB types (control bits 15:10), and the bits around them.
pub mod trb {
    pub const NORMAL: u32 = 1;
    pub const SETUP: u32 = 2;
    pub const DATA: u32 = 3;
    pub const STATUS: u32 = 4;
    pub const LINK: u32 = 6;
    pub const ENABLE_SLOT: u32 = 9;
    pub const DISABLE_SLOT: u32 = 10;
    pub const ADDRESS_DEVICE: u32 = 11;
    pub const CONFIGURE_ENDPOINT: u32 = 12;
    pub const EVALUATE_CONTEXT: u32 = 13;
    pub const RESET_ENDPOINT: u32 = 14;
    pub const STOP_ENDPOINT: u32 = 15;
    pub const SET_DEQUEUE: u32 = 16;
    pub const NOOP_COMMAND: u32 = 23;
    pub const TRANSFER_EVENT: u32 = 32;
    pub const COMMAND_EVENT: u32 = 33;
    pub const PORT_EVENT: u32 = 34;

    pub const CYCLE: u32 = 1 << 0;
    /// Link: toggle the cycle when following it.
    pub const TOGGLE: u32 = 1 << 1;
    /// Transfers: an event if the packet is short; an event when done;
    /// the data is in the TRB itself (Setup).
    pub const SHORT_EVENT: u32 = 1 << 2;
    pub const DONE_EVENT: u32 = 1 << 5;
    pub const IMMEDIATE: u32 = 1 << 6;
    /// Data and Status: the direction (1 = IN, to the host).
    pub const IN: u32 = 1 << 16;

    pub const fn kind(control: u32) -> u32 {
        (control >> 10) & 0x3F
    }
}

/// Completion codes.
pub mod code {
    pub const SUCCESS: u8 = 1;
    pub const BABBLE: u8 = 3;
    pub const TRANSACTION: u8 = 4;
    pub const STALL: u8 = 6;
    pub const SHORT_PACKET: u8 = 13;
}

/// Endpoint types, for an endpoint context.
pub mod ep {
    pub const BULK_OUT: u32 = 2;
    pub const CONTROL: u32 = 4;
    pub const INTERRUPT_IN: u32 = 7;
    pub const BULK_IN: u32 = 6;
}

/// Where everything lives in the driver's DMA memory: a page each, then
/// four pages for each device, then the scratchpad pages.
pub mod layout {
    pub const PAGE: usize = 4096;
    /// The device context base address array (DCBAA).
    pub const DCBAA: usize = 0x0000;
    pub const COMMAND_RING: usize = 0x1000;
    pub const EVENT_RING: usize = 0x2000;
    /// The event ring segment table (one entry), then the scratchpad
    /// buffer array.
    pub const ERST: usize = 0x3000;
    pub const SCRATCHPAD_ARRAY: usize = 0x3100;
    /// What a control transfer reads or writes: a page.
    pub const CONTROL_DATA: usize = 0x4000;
    /// The devices: slot n (from 1) has DEVICE_PAGES pages from here -
    /// its device context, its input context, endpoint 0's ring, and a
    /// page shared by its other endpoints (rings, then buffers).
    pub const DEVICES: usize = 0x5000;
    pub const DEVICE_PAGES: usize = 4;
    pub const fn device(slot: u8) -> usize {
        DEVICES + (slot as usize - 1) * DEVICE_PAGES * PAGE
    }
    /// The endpoint page: up to two endpoints, each a ring of 32 TRBs
    /// (512 bytes) and a buffer of 1 KiB.
    pub const ENDPOINT_RING: usize = 0x000;
    pub const ENDPOINT_BUFFER: usize = 0x800;
    pub const ENDPOINT_SPAN: usize = 0x400;
    /// After the devices: the scratchpad pages the controller asks for.
    pub const fn scratchpads(slots: u8) -> usize {
        device(slots + 1)
    }
    /// How much DMA memory `slots` devices and `scratchpads` scratchpad
    /// pages need.
    pub const fn needed(slots: u8, pages: usize) -> usize {
        scratchpads(slots) + pages * PAGE
    }
    /// Mass storage, after the most scratchpad pages there can be: a page
    /// for the command (CBW) and the status (CSW), then the data - 128
    /// KiB, abi::block::SECTORS_MAX sectors. Without this much DMA memory
    /// a stick is named, not driven.
    pub const STORAGE_CBW: usize = needed(super::SLOTS, super::SCRATCHPADS_MAX);
    pub const STORAGE_CSW: usize = STORAGE_CBW + 0x100;
    pub const STORAGE_DATA: usize = STORAGE_CBW + PAGE;
    pub const STORAGE_DATA_SIZE: usize = 128 * 1024;
    pub const STORAGE_END: usize = STORAGE_DATA + STORAGE_DATA_SIZE;
}

/// The most devices driven at once: slots 1 to SLOTS.
pub const SLOTS: u8 = 8;
/// The most scratchpad pages a controller may ask for here.
pub const SCRATCHPADS_MAX: usize = 32;
/// TRBs per ring: a page's worth for the command and event rings (256),
/// 32 for an endpoint's.
const RING_PAGE: usize = 256;
const ENDPOINT_RING_TRBS: usize = 32;
/// How long a command or a transfer may take: 1 s. A reset, 1 s too.
pub const WAIT: u64 = 100;
/// How long a bulk transfer may take: 10 s. A stick may stop to tidy
/// its flash in the middle of a write.
pub const BULK_WAIT: u64 = 1000;
/// The most one TRB moves, and the boundary none may cross: 64 KiB.
const TRB_MAX: usize = 0x10000;

/// The controller as the driver reaches it: its registers (offsets from
/// BAR 0), and a way to wait.
pub trait Bus {
    fn read(&mut self, register: usize) -> u32;
    fn write(&mut self, register: usize, value: u32);
    /// Waits a little: until the controller interrupts, or a tick.
    fn pause(&mut self);
    /// The time, in ticks (100 a second).
    fn ticks(&mut self) -> u64;
    /// How many bytes of registers there are to read and write: the
    /// whole BAR, or less when the kernel kept an MSI-X table at its end.
    fn size(&self) -> usize;

    /// A 64-bit register, as two 32-bit accesses: the low half first.
    fn write64(&mut self, register: usize, value: u64) {
        self.write(register, value as u32);
        self.write(register + 4, (value >> 32) as u32);
    }
}

/// Memory both the driver and the controller use.
pub struct Dma {
    base: *mut u8,
    physical: u64,
    size: usize,
}

impl Dma {
    /// # Safety
    /// `base` must be a mapping of `size` bytes, writable, that nothing
    /// else uses for as long as the `Dma` lives, aligned to 8 bytes (it is
    /// read and written in words: a page, as DMA memory always is);
    /// `physical` is where those bytes are for the controller (one
    /// unbroken range).
    pub unsafe fn new(base: *mut u8, physical: u64, size: usize) -> Dma {
        assert!((base as usize).is_multiple_of(8), "DMA memory must be aligned to 8 bytes");
        Dma { base, physical, size }
    }

    pub fn physical(&self, offset: usize) -> u64 {
        self.physical + offset as u64
    }

    /// Volatile, every access: the controller reads and writes this
    /// memory behind the compiler's back.
    pub fn write32(&mut self, offset: usize, value: u32) {
        assert!(offset + 4 <= self.size && offset.is_multiple_of(4));
        // SAFETY: inside the mapping (checked above), aligned, ours.
        unsafe { self.base.add(offset).cast::<u32>().write_volatile(value) }
    }

    pub fn read32(&self, offset: usize) -> u32 {
        assert!(offset + 4 <= self.size && offset.is_multiple_of(4));
        // SAFETY: as in `write32`.
        unsafe { self.base.add(offset).cast::<u32>().read_volatile() }
    }

    pub fn write64(&mut self, offset: usize, value: u64) {
        self.write32(offset, value as u32);
        self.write32(offset + 4, (value >> 32) as u32);
    }

    pub fn read64(&self, offset: usize) -> u64 {
        u64::from(self.read32(offset)) | u64::from(self.read32(offset + 4)) << 32
    }

    pub fn zero(&mut self, offset: usize, length: usize) {
        for at in (offset..offset + length).step_by(4) {
            self.write32(at, 0);
        }
    }

    /// Bytes the controller wrote, once it has said it is done.
    pub fn bytes(&self, offset: usize, length: usize) -> &[u8] {
        assert!(offset + length <= self.size);
        // SAFETY: inside the mapping (checked above); the controller is
        // done with these bytes until the driver hands them over again,
        // which needs `&mut self`.
        unsafe { core::slice::from_raw_parts(self.base.add(offset), length) }
    }

    pub fn bytes_mut(&mut self, offset: usize, length: usize) -> &mut [u8] {
        assert!(offset + length <= self.size, "DMA bytes {offset:#x}+{length:#x} past the memory");
        // SAFETY: inside the memory (checked above), which this owns;
        // the &mut self borrow keeps any other view away while it lives.
        unsafe { core::slice::from_raw_parts_mut(self.base.add(offset), length) }
    }

    pub fn write_bytes(&mut self, offset: usize, bytes: &[u8]) {
        assert!(offset + bytes.len() <= self.size);
        for (n, &byte) in bytes.iter().enumerate() {
            // SAFETY: inside the mapping (checked above).
            unsafe { self.base.add(offset + n).write_volatile(byte) }
        }
    }
}

/// A ring the driver produces TRBs on: the command ring, or an
/// endpoint's transfer ring.
#[derive(Clone, Copy, Debug)]
pub struct Ring {
    base: usize,
    /// TRBs in it, the Link TRB at the end among them.
    size: usize,
    enqueue: usize,
    cycle: bool,
}

impl Ring {
    pub const fn new(base: usize, size: usize) -> Ring {
        Ring { base, size, enqueue: 0, cycle: true }
    }

    /// Where the next TRB goes, and the cycle it will have: for Set TR
    /// Dequeue Pointer, and for an endpoint context.
    pub fn dequeue_for_controller(&self, dma: &Dma) -> u64 {
        dma.physical(self.base + 16 * self.enqueue) | u64::from(self.cycle)
    }

    /// Writes one TRB and returns its address. The control word, which
    /// holds the cycle bit, is written last: until then the TRB is not
    /// the controller's.
    fn push(&mut self, dma: &mut Dma, parameter: u64, status: u32, control: u32) -> u64 {
        let at = self.base + 16 * self.enqueue;
        dma.write64(at, parameter);
        dma.write32(at + 8, status);
        fence(Ordering::SeqCst);
        dma.write32(at + 12, (control & !trb::CYCLE) | u32::from(self.cycle));
        let address = dma.physical(at);
        self.enqueue += 1;
        if self.enqueue == self.size - 1 {
            // The Link TRB, back to the start; the cycle flips there.
            let link = self.base + 16 * self.enqueue;
            dma.write64(link, dma.physical(self.base));
            dma.write32(link + 8, 0);
            fence(Ordering::SeqCst);
            dma.write32(link + 12, trb::LINK << 10 | trb::TOGGLE | u32::from(self.cycle));
            self.enqueue = 0;
            self.cycle = !self.cycle;
        }
        address
    }
}

/// An event from the controller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// A command finished: which (its TRB's address), how, and the slot
    /// it concerned (Enable Slot's answer is the new slot).
    Command { trb: u64, code: u8, slot: u8 },
    /// A transfer finished: its TRB, how, how many bytes were *not*
    /// moved, which slot and endpoint (by DCI).
    Transfer { trb: u64, code: u8, remaining: u32, slot: u8, endpoint: u8 },
    /// Something changed on a port.
    Port { port: u8 },
    Other,
}

impl Event {
    fn from_trb(parameter: u64, status: u32, control: u32) -> Event {
        let code = (status >> 24) as u8;
        let slot = (control >> 24) as u8;
        match trb::kind(control) {
            trb::COMMAND_EVENT => Event::Command { trb: parameter, code, slot },
            trb::TRANSFER_EVENT => Event::Transfer {
                trb: parameter,
                code,
                remaining: status & 0x00FF_FFFF,
                slot,
                endpoint: ((control >> 16) & 0x1F) as u8,
            },
            trb::PORT_EVENT => Event::Port { port: (parameter >> 24) as u8 },
            _ => Event::Other,
        }
    }
}

/// A port's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PortStatus {
    pub connected: bool,
    pub enabled: bool,
    pub speed: Speed,
}

/// How fast a device talks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Speed {
    Low,
    Full,
    High,
    Super,
    Unknown(u8),
}

impl Speed {
    /// From a port's speed field (the default speed IDs).
    pub const fn from_id(id: u8) -> Speed {
        match id {
            1 => Speed::Full,
            2 => Speed::Low,
            3 => Speed::High,
            4 | 5 => Speed::Super,
            other => Speed::Unknown(other),
        }
    }

    pub const fn id(self) -> u8 {
        match self {
            Speed::Full => 1,
            Speed::Low => 2,
            Speed::High => 3,
            Speed::Super => 4,
            Speed::Unknown(id) => id,
        }
    }

    /// Endpoint 0's packet size until the device says its own.
    pub const fn default_packet(self) -> u16 {
        match self {
            Speed::Low | Speed::Full | Speed::Unknown(_) => 8,
            Speed::High => 64,
            Speed::Super => 512,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Speed::Low => "low speed",
            Speed::Full => "full speed",
            Speed::High => "high speed",
            Speed::Super => "SuperSpeed",
            Speed::Unknown(_) => "unknown speed",
        }
    }
}

/// A control request's eight bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Setup {
    pub request_type: u8,
    pub request: u8,
    pub value: u16,
    pub index: u16,
    pub length: u16,
}

impl Setup {
    pub const fn to_u64(self) -> u64 {
        self.request_type as u64
            | (self.request as u64) << 8
            | (self.value as u64) << 16
            | (self.index as u64) << 32
            | (self.length as u64) << 48
    }

    pub const fn from_u64(v: u64) -> Setup {
        Setup { request_type: v as u8, request: (v >> 8) as u8, value: (v >> 16) as u16, index: (v >> 32) as u16, length: (v >> 48) as u16 }
    }

    /// Data flows to the host.
    pub const fn is_in(self) -> bool {
        self.request_type & 0x80 != 0
    }
}

/// Why a transfer or command failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    /// The controller answered with this completion code.
    Code(u8),
    /// No answer in time.
    TimedOut,
}

/// What the driver keeps for each slot.
#[derive(Clone, Copy)]
struct Slot {
    port: u8,
    speed: Speed,
    ep0_packet: u16,
    control: Ring,
    /// Endpoints beyond 0: their DCI and ring (at most two).
    endpoints: [Option<(u8, Ring)>; 2],
    /// The highest DCI configured.
    last_dci: u8,
    /// Endpoints the controller has halted after an error (bit n: DCI
    /// n), until Reset Endpoint.
    halted: u32,
}

/// Events taken while waiting for another, kept for later.
const STASH: usize = 32;

/// An xHCI controller, started.
pub struct Controller<B: Bus> {
    bus: B,
    dma: Dma,
    operational: usize,
    runtime: usize,
    doorbells: usize,
    context_size: usize,
    pub ports: u8,
    pub version: u16,
    pub scratchpads: usize,
    command: Ring,
    event_index: usize,
    event_cycle: bool,
    stash: [Option<Event>; STASH],
    slots: [Option<Slot>; SLOTS as usize],
}

impl<B: Bus> Controller<B> {
    /// Takes the controller over and starts it:
    ///
    ///   1. take it from the firmware (the "legacy support" capability:
    ///      the BIOS may be using it for its own keyboard support);
    ///   2. stop it, reset it, wait until it is ready - whatever an
    ///      earlier driver left in it is forgotten;
    ///   3. give it the device context array (with the scratchpad pages
    ///      it asks for), the command ring, and an event ring;
    ///   4. run.
    pub fn start(bus: B, dma: Dma, interrupts: bool) -> Result<Controller<B>, &'static str> {
        let mut bus = bus;
        let caplength = bus.read(cap::CAPLENGTH);
        let operational = (caplength & 0xFF) as usize;
        let version = (caplength >> 16) as u16;
        let params1 = bus.read(cap::HCSPARAMS1);
        let params2 = bus.read(cap::HCSPARAMS2);
        let cparams = bus.read(cap::HCCPARAMS1);
        let ports = (params1 >> 24) as u8;
        let slots = (params1 & 0xFF) as u8;
        let scratchpads = ((params2 >> 21) & 0x1F) as usize * 32 + ((params2 >> 27) & 0x1F) as usize;
        if operational < 0x20 || ports == 0 || slots == 0 {
            return Err("these are not an xHCI controller's registers");
        }
        if scratchpads > SCRATCHPADS_MAX {
            return Err("the controller asks for more scratchpad pages than this driver has");
        }
        if dma.size < layout::needed(SLOTS, scratchpads) || !dma.physical.is_multiple_of(layout::PAGE as u64) {
            return Err("too little DMA memory, or not page-aligned");
        }
        if cparams & 1 == 0 && dma.physical + dma.size as u64 > 1 << 32 {
            return Err("the controller reaches only 32-bit addresses, and the DMA memory is above 4 GiB");
        }
        let mut controller = Controller {
            bus,
            dma,
            operational,
            runtime: 0,
            doorbells: 0,
            context_size: if cparams & (1 << 2) != 0 { 64 } else { 32 },
            ports,
            version,
            scratchpads,
            command: Ring::new(layout::COMMAND_RING, RING_PAGE),
            event_index: 0,
            event_cycle: true,
            stash: [None; STASH],
            slots: [None; SLOTS as usize],
        };
        controller.runtime = (controller.bus.read(cap::RTSOFF) & !0x1F) as usize;
        controller.doorbells = (controller.bus.read(cap::DBOFF) & !0x3) as usize;
        // Every register used must be within reach: the ports', the
        // first interrupter's, and a doorbell per slot.
        let ends = [
            controller.operational + op::portsc(ports) + 0x10,
            controller.runtime + 0x20 + 0x20,
            controller.doorbells + 4 * (usize::from(SLOTS) + 1),
        ];
        if ends.iter().any(|&end| end > controller.bus.size()) {
            return Err("some of its registers are beyond the memory given for them");
        }
        controller.take_from_firmware((cparams >> 16) as usize * 4)?;

        // Stop, then reset.
        let usbcmd = controller.operational + op::USBCMD;
        let usbsts = controller.operational + op::USBSTS;
        let command = controller.bus.read(usbcmd);
        controller.bus.write(usbcmd, command & !(op::CMD_RUN | op::CMD_INTERRUPTS));
        if !controller.wait(|bus| bus.read(usbsts) & op::STS_HALTED != 0) {
            return Err("the controller would not stop");
        }
        controller.bus.write(usbcmd, op::CMD_RESET);
        if !controller.wait(|bus| bus.read(usbcmd) & op::CMD_RESET == 0 && bus.read(usbsts) & op::STS_NOT_READY == 0) {
            return Err("the controller did not come out of its reset");
        }
        if controller.bus.read(controller.operational + op::PAGESIZE) & 1 == 0 {
            return Err("the controller does not take 4 KiB pages");
        }

        // Memory: everything the controller will read starts zeroed.
        let all = controller.dma.size;
        controller.dma.zero(0, all.min(layout::needed(SLOTS, scratchpads)));
        controller.bus.write(controller.operational + op::CONFIG, u32::from(SLOTS.min(slots)));
        if scratchpads > 0 {
            for n in 0..scratchpads {
                let page = controller.dma.physical(layout::scratchpads(SLOTS) + n * layout::PAGE);
                controller.dma.write64(layout::SCRATCHPAD_ARRAY + 8 * n, page);
            }
            let array = controller.dma.physical(layout::SCRATCHPAD_ARRAY);
            controller.dma.write64(layout::DCBAA, array);
        }
        let dcbaa = controller.dma.physical(layout::DCBAA);
        controller.bus.write64(controller.operational + op::DCBAAP, dcbaa);
        let ring = controller.command.dequeue_for_controller(&controller.dma);
        controller.bus.write64(controller.operational + op::CRCR, ring);

        // The event ring: one segment, a page of TRBs.
        let segment = controller.dma.physical(layout::EVENT_RING);
        controller.dma.write64(layout::ERST, segment);
        controller.dma.write32(layout::ERST + 8, RING_PAGE as u32);
        let interrupter = controller.runtime;
        controller.bus.write(interrupter + int::ERSTSZ, 1);
        controller.bus.write64(interrupter + int::ERDP, segment);
        let table = controller.dma.physical(layout::ERST);
        controller.bus.write64(interrupter + int::ERSTBA, table);
        // At most 4000 interrupts a second (250 us apart).
        controller.bus.write(interrupter + int::IMOD, 1000);
        let enable = if interrupts { int::IMAN_ENABLE } else { 0 };
        controller.bus.write(interrupter + int::IMAN, int::IMAN_PENDING | enable);

        let run = op::CMD_RUN | if interrupts { op::CMD_INTERRUPTS } else { 0 };
        controller.bus.write(usbcmd, run);
        if !controller.wait(|bus| bus.read(usbsts) & op::STS_HALTED == 0) {
            return Err("the controller would not run");
        }
        Ok(controller)
    }

    /// The USB Legacy Support capability, if there is one: the BIOS
    /// gives the controller up when the operating system asks.
    fn take_from_firmware(&mut self, mut at: usize) -> Result<(), &'static str> {
        let mut seen = 0;
        while at != 0 && at + 8 <= self.bus.size() && seen < 64 {
            let word = self.bus.read(at);
            if word & 0xFF == 1 {
                const BIOS_OWNED: u32 = 1 << 16;
                const OS_OWNED: u32 = 1 << 24;
                self.bus.write(at, word | OS_OWNED);
                if !self.wait(|bus| bus.read(at) & BIOS_OWNED == 0) {
                    return Err("the firmware would not give the controller up");
                }
                // And no more SMIs: its enables off, its events cleared.
                let control = self.bus.read(at + 4);
                self.bus.write(at + 4, (control & (0x7 << 1 | 0xFF << 5 | 0x7 << 17)) | 0x7 << 29);
                return Ok(());
            }
            let next = ((word >> 8) & 0xFF) as usize;
            at = if next == 0 { 0 } else { at + 4 * next };
            seen += 1;
        }
        Ok(())
    }

    fn wait(&mut self, mut done: impl FnMut(&mut B) -> bool) -> bool {
        let deadline = self.bus.ticks() + WAIT;
        loop {
            if done(&mut self.bus) {
                return true;
            }
            if self.bus.ticks() >= deadline {
                return false;
            }
            self.bus.pause();
        }
    }

    pub fn port_status(&mut self, port: u8) -> PortStatus {
        let status = self.bus.read(self.operational + op::portsc(port));
        PortStatus {
            connected: status & port::CONNECTED != 0,
            enabled: status & port::ENABLED != 0,
            speed: Speed::from_id(((status >> port::SPEED_SHIFT) & 0xF) as u8),
        }
    }

    /// Clears what a port says changed, so that the next change makes a
    /// new event.
    pub fn port_seen(&mut self, port: u8) {
        let register = self.operational + op::portsc(port);
        let status = self.bus.read(register);
        self.bus.write(register, (status & port::KEEP) | (status & port::CHANGES));
    }

    /// Resets a USB 2 port, which enables it; returns its speed. (A USB 3
    /// port enables itself when a device connects.)
    pub fn reset_port(&mut self, port: u8) -> Result<Speed, &'static str> {
        let register = self.operational + op::portsc(port);
        let status = self.bus.read(register);
        self.bus.write(register, (status & port::KEEP) | port::RESET);
        if !self.wait(|bus| bus.read(register) & port::RESET_CHANGE != 0) {
            return Err("the port did not finish its reset");
        }
        let status = self.bus.read(register);
        self.bus.write(register, (status & port::KEEP) | (status & port::CHANGES));
        if status & port::ENABLED == 0 {
            return Err("the port did not enable after its reset");
        }
        Ok(Speed::from_id(((status >> port::SPEED_SHIFT) & 0xF) as u8))
    }

    /// After an interrupt: says it was seen.
    pub fn interrupt_seen(&mut self) {
        let iman = self.runtime + int::IMAN;
        let value = self.bus.read(iman);
        self.bus.write(iman, value | int::IMAN_PENDING);
        self.bus.write(self.operational + op::USBSTS, op::STS_EVENT);
    }

    /// The next event: one kept from earlier, or a new one from the ring.
    pub fn next_event(&mut self) -> Option<Event> {
        if let Some(kept) = self.stash.iter_mut().find_map(Option::take) {
            return Some(kept);
        }
        self.take_event()
    }

    fn take_event(&mut self) -> Option<Event> {
        let at = layout::EVENT_RING + 16 * self.event_index;
        let control = self.dma.read32(at + 12);
        if (control & trb::CYCLE != 0) != self.event_cycle {
            return None;
        }
        fence(Ordering::Acquire);
        let event = Event::from_trb(self.dma.read64(at), self.dma.read32(at + 8), control);
        self.event_index += 1;
        if self.event_index == RING_PAGE {
            self.event_index = 0;
            self.event_cycle = !self.event_cycle;
        }
        // Read up to here: the controller may write over what is behind.
        let dequeue = self.dma.physical(layout::EVENT_RING + 16 * self.event_index) | int::ERDP_BUSY;
        self.bus.write64(self.runtime + int::ERDP, dequeue);
        Some(event)
    }

    /// Waits for the event `wanted` picks; keeps the others for later.
    fn wait_for(&mut self, wanted: impl Fn(&Event) -> bool) -> Result<Event, Failure> {
        self.wait_for_within(WAIT, wanted)
    }

    fn wait_for_within(&mut self, ticks: u64, wanted: impl Fn(&Event) -> bool) -> Result<Event, Failure> {
        let deadline = self.bus.ticks() + ticks;
        loop {
            // Said seen before looking: an event that comes after the
            // look interrupts again, instead of waiting unseen.
            self.interrupt_seen();
            while let Some(event) = self.take_event() {
                if wanted(&event) {
                    return Ok(event);
                }
                if let Some(free) = self.stash.iter_mut().find(|e| e.is_none()) {
                    *free = Some(event);
                }
                // Full: the oldest kept event is lost - only port events
                // and reports pile up, and a port is looked at again.
            }
            if self.bus.ticks() >= deadline {
                return Err(Failure::TimedOut);
            }
            self.bus.pause();
        }
    }

    fn ring_doorbell(&mut self, slot: u8, target: u8) {
        fence(Ordering::SeqCst);
        self.bus.write(self.doorbells + 4 * slot as usize, u32::from(target));
    }

    /// Runs one command and waits for its answer: the slot it names.
    fn command(&mut self, parameter: u64, control: u32) -> Result<u8, Failure> {
        let mut ring = self.command;
        let address = ring.push(&mut self.dma, parameter, 0, control);
        self.command = ring;
        self.ring_doorbell(0, 0);
        match self.wait_for(|e| matches!(e, Event::Command { trb, .. } if *trb == address))? {
            Event::Command { code: code::SUCCESS, slot, .. } => Ok(slot),
            Event::Command { code, .. } => Err(Failure::Code(code)),
            _ => Err(Failure::TimedOut),
        }
    }

    /// A command that does nothing: is the controller listening?
    pub fn noop(&mut self) -> Result<(), Failure> {
        self.command(0, trb::NOOP_COMMAND << 10).map(|_| ())
    }

    /// A new slot, for a device that has just been reset.
    pub fn enable_slot(&mut self) -> Result<u8, &'static str> {
        let slot = self.command(0, trb::ENABLE_SLOT << 10).map_err(|_| "the controller would not give a slot")?;
        if slot == 0 || slot > SLOTS {
            let _ = self.command(0, trb::DISABLE_SLOT << 10 | u32::from(slot) << 24);
            return Err("the controller gave a slot beyond the ones this driver keeps");
        }
        Ok(slot)
    }

    /// The device is gone: its slot is given back.
    pub fn disable_slot(&mut self, slot: u8) {
        let _ = self.command(0, trb::DISABLE_SLOT << 10 | u32::from(slot) << 24);
        if let Some(kept) = self.slots.get_mut(usize::from(slot).wrapping_sub(1)) {
            *kept = None;
        }
        self.dma.write64(layout::DCBAA + 8 * usize::from(slot), 0);
    }

    fn slot(&self, slot: u8) -> Option<&Slot> {
        self.slots.get(usize::from(slot).wrapping_sub(1))?.as_ref()
    }

    /// An input context's part `index` (0 the control context, 1 the
    /// slot, 1 + n endpoint DCI n): its offset in the DMA memory.
    fn input(&self, slot: u8, index: usize) -> usize {
        layout::device(slot) + layout::PAGE + index * self.context_size
    }

    /// Fills the input context: which parts are added, the slot context
    /// (speed, port, the highest endpoint), endpoint 0's context.
    fn input_slot_and_ep0(&mut self, slot: u8, add: u32) {
        let s = *self.slot(slot).expect("a slot that was enabled");
        let base = self.input(slot, 0);
        self.dma.zero(base, layout::PAGE);
        self.dma.write32(base + 4, add);
        let ctx = self.input(slot, 1);
        self.dma.write32(ctx, u32::from(s.speed.id()) << 20 | u32::from(s.last_dci) << 27);
        self.dma.write32(ctx + 4, u32::from(s.port) << 16);
        let ep0 = self.input(slot, 2);
        // Three retries on error; a control endpoint; its packet size.
        self.dma.write32(ep0 + 4, 3 << 1 | ep::CONTROL << 3 | u32::from(s.ep0_packet) << 16);
        let ring = s.control.dequeue_for_controller(&self.dma);
        self.dma.write64(ep0 + 8, ring);
        self.dma.write32(ep0 + 16, 8);
    }

    /// Gives the device in `slot` (on `port`, at `speed`) its address,
    /// with endpoint 0 ready for control transfers.
    pub fn address_device(&mut self, slot: u8, port: u8, speed: Speed) -> Result<(), &'static str> {
        let index = usize::from(slot) - 1;
        let control = Ring::new(layout::device(slot) + 2 * layout::PAGE, RING_PAGE);
        self.slots[index] =
            Some(Slot { port, speed, ep0_packet: speed.default_packet(), control, endpoints: [None; 2], last_dci: 1, halted: 0 });
        self.dma.zero(layout::device(slot), layout::DEVICE_PAGES * layout::PAGE);
        let output = self.dma.physical(layout::device(slot));
        self.dma.write64(layout::DCBAA + 8 * usize::from(slot), output);
        self.input_slot_and_ep0(slot, 0b11);
        let input = self.dma.physical(self.input(slot, 0));
        self.command(input, trb::ADDRESS_DEVICE << 10 | u32::from(slot) << 24)
            .map(|_| ())
            .map_err(|_| "the device would not take an address")
    }

    /// Endpoint 0's real packet size, once the device has said it.
    pub fn set_ep0_packet(&mut self, slot: u8, packet: u16) -> Result<(), &'static str> {
        let Some(s) = self.slots.get_mut(usize::from(slot) - 1).and_then(Option::as_mut) else {
            return Err("no such slot");
        };
        if s.ep0_packet == packet {
            return Ok(());
        }
        s.ep0_packet = packet;
        self.input_slot_and_ep0(slot, 0b10);
        let input = self.dma.physical(self.input(slot, 0));
        self.command(input, trb::EVALUATE_CONTEXT << 10 | u32::from(slot) << 24)
            .map(|_| ())
            .map_err(|_| "the controller would not take endpoint 0's packet size")
    }

    /// A control transfer on endpoint 0. For a request that reads, the
    /// bytes are in `control_data` afterwards; returns how many came.
    pub fn control(&mut self, slot: u8, setup: Setup, out: &[u8]) -> Result<usize, Failure> {
        let Some(mut s) = self.slot(slot).copied() else { return Err(Failure::Code(0)) };
        let length = usize::from(setup.length).min(layout::PAGE);
        if !setup.is_in() {
            self.dma.write_bytes(layout::CONTROL_DATA, &out[..length.min(out.len())]);
        }
        // Transfer type: no data (0), OUT (2), IN (3).
        let kind = match (length, setup.is_in()) {
            (0, _) => 0,
            (_, false) => 2,
            (_, true) => 3,
        };
        let ring = &mut s.control;
        ring.push(&mut self.dma, setup.to_u64(), 8, trb::SETUP << 10 | trb::IMMEDIATE | kind << 16);
        let mut data_trb = None;
        if length > 0 {
            let direction = if setup.is_in() { trb::IN } else { 0 };
            let buffer = self.dma.physical(layout::CONTROL_DATA);
            data_trb = Some(ring.push(&mut self.dma, buffer, length as u32, trb::DATA << 10 | trb::SHORT_EVENT | direction));
        }
        // The status stage goes the other way from the data (IN if none).
        let direction = if length > 0 && setup.is_in() { 0 } else { trb::IN };
        let status = ring.push(&mut self.dma, 0, 0, trb::STATUS << 10 | trb::DONE_EVENT | direction);
        self.slots[usize::from(slot) - 1] = Some(s);
        self.ring_doorbell(slot, 1);

        let mut moved = length;
        loop {
            let event = self.wait_for(|e| matches!(e, Event::Transfer { slot: n, endpoint: 1, .. } if *n == slot))?;
            let Event::Transfer { trb: at, code, remaining, .. } = event else { continue };
            match code {
                code::SUCCESS if at == status => return Ok(moved),
                code::SHORT_PACKET if Some(at) == data_trb => moved = length - (remaining as usize).min(length),
                code::SUCCESS => {}
                other => {
                    self.recover(slot, 1);
                    return Err(Failure::Code(other));
                }
            }
        }
    }

    /// What the last control transfer read.
    pub fn control_data(&self, length: usize) -> &[u8] {
        self.dma.bytes(layout::CONTROL_DATA, length.min(layout::PAGE))
    }

    /// After an error (a stall, say) an endpoint is halted: reset it,
    /// and point it past what it did not do.
    fn recover(&mut self, slot: u8, dci: u8) {
        self.recover_as(slot, dci, true);
    }

    /// As `recover`; `halted` false for an endpoint that is running (a
    /// transfer timed out): it is stopped instead of reset, and then
    /// pointed past what it did not do.
    fn recover_as(&mut self, slot: u8, dci: u8, halted: bool) {
        let kind = if halted { trb::RESET_ENDPOINT } else { trb::STOP_ENDPOINT };
        let _ = self.command(0, kind << 10 | u32::from(dci) << 16 | u32::from(slot) << 24);
        if let Some(s) = self.slots.get_mut(usize::from(slot).wrapping_sub(1)).and_then(Option::as_mut) {
            s.halted &= !(1 << dci);
        }
        let ring = match self.slot(slot) {
            Some(s) if dci == 1 => s.control,
            Some(s) => match s.endpoints.iter().flatten().find(|(n, _)| *n == dci) {
                Some(&(_, ring)) => ring,
                None => return,
            },
            None => return,
        };
        let dequeue = ring.dequeue_for_controller(&self.dma);
        let _ = self.command(dequeue, trb::SET_DEQUEUE << 10 | u32::from(dci) << 16 | u32::from(slot) << 24);
    }

    /// Sets up interrupt IN endpoints (address, packet size, and the
    /// device's bInterval) - at most two per device - with one Configure
    /// Endpoint command. Returns their DCIs.
    pub fn configure_interrupt_in(&mut self, slot: u8, endpoints: &[(u8, u16, u8)]) -> Result<[u8; 2], &'static str> {
        let Some(mut s) = self.slot(slot).copied() else { return Err("no such slot") };
        let mut dcis = [0u8; 2];
        let mut add = 0b1; // the slot context
        for (n, &(address, _, _)) in endpoints.iter().take(2).enumerate() {
            let dci = 2 * (address & 0x0F) + 1; // an IN endpoint
            let ring = Ring::new(layout::device(slot) + 3 * layout::PAGE + layout::ENDPOINT_RING + n * layout::ENDPOINT_SPAN, ENDPOINT_RING_TRBS);
            s.endpoints[n] = Some((dci, ring));
            s.last_dci = s.last_dci.max(dci);
            dcis[n] = dci;
            add |= 1 << dci;
        }
        self.slots[usize::from(slot) - 1] = Some(s);
        self.input_slot_and_ep0(slot, add);
        for (n, &(_, packet, interval)) in endpoints.iter().take(2).enumerate() {
            let (dci, ring) = s.endpoints[n].expect("set above");
            let ctx = self.input(slot, 1 + usize::from(dci));
            let packet = u32::from(packet & 0x7FF);
            self.dma.write32(ctx, u32::from(xhci_interval(s.speed, interval)) << 16);
            self.dma.write32(ctx + 4, 3 << 1 | ep::INTERRUPT_IN << 3 | packet << 16);
            let dequeue = ring.dequeue_for_controller(&self.dma);
            self.dma.write64(ctx + 8, dequeue);
            // Average TRB length, and the most one interval moves.
            self.dma.write32(ctx + 16, packet | packet << 16);
        }
        let input = self.dma.physical(self.input(slot, 0));
        self.command(input, trb::CONFIGURE_ENDPOINT << 10 | u32::from(slot) << 24)
            .map_err(|_| "the controller would not configure the endpoints")?;
        Ok(dcis)
    }

    /// Asks endpoint `dci` (the n-th configured) for up to `length`
    /// bytes. The answer comes as a Transfer event; the bytes are then in
    /// `endpoint_data`.
    pub fn queue_in(&mut self, slot: u8, dci: u8, length: usize) -> Result<(), &'static str> {
        let Some(mut s) = self.slot(slot).copied() else { return Err("no such slot") };
        let Some(n) = s.endpoints.iter().position(|e| matches!(e, Some((d, _)) if *d == dci)) else {
            return Err("no such endpoint");
        };
        let Some((_, ring)) = s.endpoints[n].as_mut() else { return Err("no such endpoint") };
        let buffer = self.dma.physical(layout::device(slot) + 3 * layout::PAGE + layout::ENDPOINT_BUFFER + n * layout::ENDPOINT_SPAN);
        let length = length.min(layout::ENDPOINT_SPAN) as u32;
        ring.push(&mut self.dma, buffer, length, trb::NORMAL << 10 | trb::SHORT_EVENT | trb::DONE_EVENT);
        self.slots[usize::from(slot) - 1] = Some(s);
        self.ring_doorbell(slot, dci);
        Ok(())
    }

    /// What endpoint `dci` brought: the first `length` bytes of its
    /// buffer.
    pub fn endpoint_data(&self, slot: u8, dci: u8, length: usize) -> &[u8] {
        let n = self
            .slot(slot)
            .and_then(|s| s.endpoints.iter().position(|e| matches!(e, Some((d, _)) if *d == dci)))
            .unwrap_or(0);
        let at = layout::device(slot) + 3 * layout::PAGE + layout::ENDPOINT_BUFFER + n * layout::ENDPOINT_SPAN;
        self.dma.bytes(at, length.min(layout::ENDPOINT_SPAN))
    }

    /// After a failed transfer on an endpoint: make it usable again.
    pub fn recover_endpoint(&mut self, slot: u8, dci: u8) {
        self.recover(slot, dci);
    }

    /// Whether the controller halted endpoint `dci` after a failed bulk
    /// transfer (and it has not been recovered since).
    pub fn endpoint_halted(&self, slot: u8, dci: u8) -> bool {
        self.slot(slot).is_some_and(|s| s.halted & (1 << dci) != 0)
    }

    /// Whether there is DMA memory for a mass storage device's buffers.
    pub fn has_storage_memory(&self) -> bool {
        self.dma.size >= layout::STORAGE_END
    }

    /// The storage data buffer's first `length` bytes.
    pub fn storage_data(&self, length: usize) -> &[u8] {
        self.dma.bytes(layout::STORAGE_DATA, length.min(layout::STORAGE_DATA_SIZE))
    }

    pub fn storage_data_mut(&mut self, length: usize) -> &mut [u8] {
        self.dma.bytes_mut(layout::STORAGE_DATA, length.min(layout::STORAGE_DATA_SIZE))
    }

    pub(crate) fn dma_write(&mut self, offset: usize, bytes: &[u8]) {
        self.dma.write_bytes(offset, bytes);
    }

    pub(crate) fn dma_read(&self, offset: usize, length: usize) -> &[u8] {
        self.dma.bytes(offset, length)
    }

    /// Sets up a bulk OUT and a bulk IN endpoint (address and packet
    /// size each), with one Configure Endpoint command. Returns their
    /// DCIs, OUT first.
    pub fn configure_bulk(&mut self, slot: u8, out: (u8, u16), inward: (u8, u16)) -> Result<(u8, u8), &'static str> {
        let Some(mut s) = self.slot(slot).copied() else { return Err("no such slot") };
        let out_dci = 2 * (out.0 & 0x0F);
        let in_dci = 2 * (inward.0 & 0x0F) + 1;
        if out_dci == 0 || in_dci == 1 {
            return Err("a bulk endpoint numbered 0");
        }
        let page = layout::device(slot) + 3 * layout::PAGE;
        let rings = [
            (out_dci, Ring::new(page + layout::ENDPOINT_RING, ENDPOINT_RING_TRBS), ep::BULK_OUT, out.1),
            (in_dci, Ring::new(page + layout::ENDPOINT_RING + layout::ENDPOINT_SPAN, ENDPOINT_RING_TRBS), ep::BULK_IN, inward.1),
        ];
        let mut add = 0b1;
        for (n, &(dci, ring, _, _)) in rings.iter().enumerate() {
            s.endpoints[n] = Some((dci, ring));
            s.last_dci = s.last_dci.max(dci);
            add |= 1 << dci;
        }
        self.slots[usize::from(slot) - 1] = Some(s);
        self.input_slot_and_ep0(slot, add);
        for &(dci, ring, kind, packet) in &rings {
            let ctx = self.input(slot, 1 + usize::from(dci));
            // Three retries on error; the type; no bursts; the packet size.
            self.dma.write32(ctx + 4, 3 << 1 | kind << 3 | u32::from(packet & 0x7FF) << 16);
            let dequeue = ring.dequeue_for_controller(&self.dma);
            self.dma.write64(ctx + 8, dequeue);
            // The average TRB length the spec suggests for bulk.
            self.dma.write32(ctx + 16, 3072);
        }
        let input = self.dma.physical(self.input(slot, 0));
        self.command(input, trb::CONFIGURE_ENDPOINT << 10 | u32::from(slot) << 24)
            .map_err(|_| "the controller would not configure the endpoints")?;
        Ok((out_dci, in_dci))
    }

    /// Moves `length` bytes at `offset` in the DMA memory over bulk
    /// endpoint `dci` (odd: in, even: out), and waits until they have
    /// moved. Returns how many did: fewer when the device sent a short
    /// packet, which ends a transfer in.
    ///
    /// Each piece is one TRB, at most 64 KiB and not crossing a 64 KiB
    /// boundary, and waited for before the next is given: after a short
    /// packet nothing more is given, so the next thing the device sends
    /// (a status, say) is not taken for data.
    pub fn bulk(&mut self, slot: u8, dci: u8, offset: usize, length: usize) -> Result<usize, Failure> {
        let mut moved = 0;
        while moved < length {
            let physical = self.dma.physical(offset + moved);
            let piece = (length - moved).min(TRB_MAX - (physical as usize & (TRB_MAX - 1)));
            let Some(mut s) = self.slot(slot).copied() else { return Err(Failure::Code(0)) };
            let Some((_, ring)) = s.endpoints.iter_mut().flatten().find(|(d, _)| *d == dci) else {
                return Err(Failure::Code(0));
            };
            let at = ring.push(&mut self.dma, physical, piece as u32, trb::NORMAL << 10 | trb::SHORT_EVENT | trb::DONE_EVENT);
            self.slots[usize::from(slot) - 1] = Some(s);
            self.ring_doorbell(slot, dci);
            let event = match self.wait_for_within(BULK_WAIT, |e| matches!(e, Event::Transfer { trb, .. } if *trb == at)) {
                Ok(event) => event,
                Err(failure) => {
                    // The TRB is still the controller's: stop the
                    // endpoint and point it past.
                    self.recover_as(slot, dci, false);
                    return Err(failure);
                }
            };
            let Event::Transfer { code, remaining, .. } = event else { return Err(Failure::TimedOut) };
            match code {
                code::SUCCESS => moved += piece,
                code::SHORT_PACKET => return Ok(moved + piece - (remaining as usize).min(piece)),
                other => {
                    if let Some(s) = self.slots.get_mut(usize::from(slot) - 1).and_then(Option::as_mut) {
                        s.halted |= 1 << dci;
                    }
                    return Err(Failure::Code(other));
                }
            }
        }
        Ok(moved)
    }
}

/// An interrupt endpoint's interval, as xHCI counts it: 2^n times
/// 125 us. A low- or full-speed device says bInterval in milliseconds
/// (1-255); a high-speed or faster one as 2^(bInterval-1) times 125 us.
pub fn xhci_interval(speed: Speed, b_interval: u8) -> u8 {
    match speed {
        Speed::Low | Speed::Full | Speed::Unknown(_) => {
            let eighths = u32::from(b_interval.max(1)) * 8;
            (31 - eighths.leading_zeros()).clamp(3, 10) as u8
        }
        Speed::High | Speed::Super => b_interval.clamp(1, 16) - 1,
    }
}
