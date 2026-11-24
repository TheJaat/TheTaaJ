//! A simulated xHCI controller, with simulated USB devices on its ports.
//!
//! The controller follows the specification where a driver can go
//! wrong, and panics at the first rule broken:
//!   - nothing but USBCMD/USBSTS is touched while a reset is going on;
//!   - running needs the device context array, the command ring and the
//!     event ring set up;
//!   - rings are read by their cycle bits and followed through Link
//!     TRBs, which flip the cycle when they say so;
//!   - writing a port's "enabled" bit as 1 disables it (as on real
//!     hardware), and the change bits clear only when written as 1;
//!   - Address Device needs the slot and endpoint 0 contexts flagged,
//!     filled in, and a device context in the array;
//!   - a device whose endpoint 0 sends bigger packets than its context
//!     says causes "babble", and the endpoint halts; so does a request
//!     the device refuses (a stall) - and a halted endpoint does nothing
//!     until Reset Endpoint and Set TR Dequeue Pointer;
//!   - an event written over one the driver has not read is an overflow.
//!
//! The devices answer GET_DESCRIPTOR, SET_CONFIGURATION, SET_PROTOCOL
//! and SET_IDLE, and send reports from their interrupt IN endpoints when
//! a test gives them one (`report`). A keyboard not switched to the boot
//! protocol sends report-protocol reports (with a report ID in front), so
//! a driver that forgets SET_PROTOCOL reads garbage.
//!
//! A device with a `disk` is a USB stick: its bulk endpoints carry the
//! Bulk-Only Transport to a simulated SCSI disk (`Disk`), which checks
//! each CBW, can stall a data step, garble a CSW, or have no medium.

use crate::xhci::{trb, Bus, Setup};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::vec::Vec;

pub const PHYSICAL: u64 = 0x0100_0000;
const OP: usize = 0x40;
const RT: usize = 0x1000;
const DB: usize = 0x2000;
const XECP: usize = 0x3000;

/// A device on a port.
#[derive(Clone, Default)]
pub struct Device {
    /// Speed ID: 1 full, 2 low, 3 high, 4 super.
    pub speed: u8,
    pub descriptor: Vec<u8>,
    pub config: Vec<u8>,
    pub configured: Option<u8>,
    /// Interface -> protocol set (0 boot, 1 report).
    pub protocol: HashMap<u8, u16>,
    pub idle_stall: bool,
    pub idles: u32,
    /// Reports waiting, by endpoint address.
    pub reports: Vec<(u8, Vec<u8>)>,
    /// Every request, in order.
    pub requests: Vec<Setup>,
    /// A USB stick's insides.
    pub disk: Option<Disk>,
}

#[derive(Clone, Copy, Default)]
pub struct Port {
    pub device: Option<usize>,
    pub usb3: bool,
    pub enabled: bool,
    pub changes: u32,
    pub resets: u32,
}

#[derive(Clone, Default)]
struct Endpoint {
    dequeue: u64,
    cycle: bool,
    packet: u32,
    halted: bool,
}

#[derive(Clone, Default)]
struct SlotState {
    device: usize,
    port: u8,
    endpoints: HashMap<u8, Endpoint>,
}

#[derive(Default)]
pub struct State {
    pub memory: Vec<u8>,
    pub time: u64,
    /// How much of the register space the driver is given (Bus::size).
    pub registers_size: usize,
    pub context_size: usize,
    pub scratchpads: usize,
    /// The BIOS holds the controller until this many ticks after the OS
    /// asks; None: no legacy capability at all.
    pub bios_release: Option<u64>,
    bios_owned: bool,
    os_asked_at: Option<u64>,
    pub legacy_control: u32,
    usbcmd: u32,
    halted: bool,
    reset_until: Option<u64>,
    pub resets: u32,
    crcr: u64,
    dcbaap: u64,
    config: u32,
    iman: u32,
    erstsz: u32,
    erstba: u64,
    erdp: u64,
    halves: HashMap<usize, u32>,
    command_dequeue: u64,
    command_cycle: bool,
    event_index: usize,
    event_cycle: bool,
    pub event_interrupts: u32,
    pub ports: Vec<Port>,
    pub devices: Vec<Device>,
    slots: HashMap<u8, SlotState>,
    /// Commands run: (type, slot).
    pub commands: Vec<(u32, u8)>,
    pub stops: u32,
}

#[derive(Clone)]
pub struct Sim(pub Rc<RefCell<State>>);

impl Sim {
    pub fn new(ports: usize, usb3_from: usize) -> Sim {
        let mut state = State {
            memory: std::vec![0u8; crate::xhci::layout::STORAGE_END],
            context_size: 32,
            registers_size: 0x4000,
            halted: true,
            ..State::default()
        };
        for n in 0..ports {
            state.ports.push(Port { usb3: n >= usb3_from, ..Port::default() });
        }
        Sim(Rc::new(RefCell::new(state)))
    }

    /// Plugs `device` into port `port` (from 1). If the controller runs,
    /// it says so with an event.
    pub fn plug(&self, port: u8, device: Device) -> usize {
        let mut s = self.0.borrow_mut();
        s.devices.push(device);
        let index = s.devices.len() - 1;
        let p = &mut s.ports[usize::from(port) - 1];
        p.device = Some(index);
        p.changes |= 1 << 17;
        p.enabled = p.usb3; // a USB 3 port trains its link by itself
        if !s.halted {
            s.post_port(port);
        }
        index
    }

    pub fn unplug(&self, port: u8) {
        let mut s = self.0.borrow_mut();
        let p = &mut s.ports[usize::from(port) - 1];
        p.device = None;
        p.enabled = false;
        p.changes |= 1 << 17;
        if !s.halted {
            s.post_port(port);
        }
    }

    /// The device sends `bytes` from endpoint `address`, when the host
    /// next asks.
    pub fn report(&self, device: usize, address: u8, bytes: &[u8]) {
        let mut s = self.0.borrow_mut();
        s.devices[device].reports.push((address, bytes.to_vec()));
        s.pump();
    }
}

impl State {
    fn at(&mut self, physical: u64, length: usize) -> &mut [u8] {
        let start = physical.checked_sub(PHYSICAL).expect("an address below the DMA memory") as usize;
        assert!(start + length <= self.memory.len(), "an address past the DMA memory: {physical:#x}+{length}");
        &mut self.memory[start..start + length]
    }

    fn word(&mut self, physical: u64) -> u32 {
        u32::from_le_bytes(self.at(physical, 4).try_into().unwrap())
    }

    fn quad(&mut self, physical: u64) -> u64 {
        u64::from_le_bytes(self.at(physical, 8).try_into().unwrap())
    }

    fn trb(&mut self, physical: u64) -> (u64, u32, u32) {
        (self.quad(physical), self.word(physical + 8), self.word(physical + 12))
    }

    fn post(&mut self, parameter: u64, status: u32, control: u32) {
        let base = self.quad(self.erstba);
        let size = self.word(self.erstba + 8) as usize;
        let read_up_to = ((self.erdp & !0xF) - base) as usize / 16;
        assert!((self.event_index + 1) % size != read_up_to, "event ring overflow: the driver is not reading");
        let at = base + 16 * self.event_index as u64;
        self.at(at, 8).copy_from_slice(&parameter.to_le_bytes());
        self.at(at + 8, 4).copy_from_slice(&status.to_le_bytes());
        let control = (control & !1) | u32::from(self.event_cycle);
        self.at(at + 12, 4).copy_from_slice(&control.to_le_bytes());
        self.event_index += 1;
        if self.event_index == size {
            self.event_index = 0;
            self.event_cycle = !self.event_cycle;
        }
        if self.iman & 2 != 0 {
            self.iman |= 1;
            self.event_interrupts += 1;
        }
    }

    fn post_port(&mut self, port: u8) {
        self.post(u64::from(port) << 24, 1 << 24, trb::PORT_EVENT << 10);
    }

    fn complete_command(&mut self, at: u64, code: u8, slot: u8) {
        self.post(at, u32::from(code) << 24, trb::COMMAND_EVENT << 10 | u32::from(slot) << 24);
    }

    fn complete_transfer(&mut self, at: u64, code: u8, remaining: u32, slot: u8, dci: u8) {
        self.post(at, u32::from(code) << 24 | remaining, trb::TRANSFER_EVENT << 10 | u32::from(dci) << 16 | u32::from(slot) << 24);
    }

    fn reset(&mut self) {
        self.resets += 1;
        self.usbcmd = 0;
        self.halted = true;
        self.crcr = 0;
        self.dcbaap = 0;
        self.config = 0;
        self.iman = 0;
        self.erstsz = 0;
        self.erstba = 0;
        self.erdp = 0;
        self.slots.clear();
        self.event_index = 0;
        self.event_cycle = true;
        for p in &mut self.ports {
            p.enabled = p.usb3 && p.device.is_some();
            if p.device.is_some() {
                p.changes |= 1 << 17;
            }
        }
        self.reset_until = Some(self.time + 3);
    }

    fn run(&mut self) {
        assert!(self.dcbaap != 0, "running without a device context array");
        assert!(self.crcr != 0, "running without a command ring");
        assert!(self.erstsz == 1 && self.erstba != 0 && self.erdp != 0, "running without an event ring");
        assert!(self.config >= 1, "running with no slots enabled");
        if self.scratchpads > 0 {
            let array = self.quad(self.dcbaap);
            assert!(array != 0, "the controller asked for scratchpad pages and got none");
            for n in 0..self.scratchpads as u64 {
                let page = self.quad(array + 8 * n);
                assert!(page != 0 && page.is_multiple_of(4096), "scratchpad page {n} missing or not a page");
            }
        }
        self.command_dequeue = self.crcr & !0x3F;
        self.command_cycle = self.crcr & 1 != 0;
        self.halted = false;
        let ports: Vec<u8> = (1..=self.ports.len() as u8).filter(|&n| self.ports[usize::from(n) - 1].device.is_some()).collect();
        for port in ports {
            self.post_port(port);
        }
    }

    fn portsc(&self, port: usize) -> u32 {
        let p = &self.ports[port];
        let speed = p.device.map_or(0, |d| u32::from(self.devices[d].speed));
        u32::from(p.device.is_some()) | u32::from(p.enabled) << 1 | 1 << 9 | speed << 10 | p.changes
    }

    fn write_portsc(&mut self, port: usize, value: u32) {
        let number = port as u8 + 1;
        let p = &mut self.ports[port];
        // Writing PED as 1 disables the port - a driver that writes the
        // register back as read does this by accident.
        if value & 2 != 0 {
            p.enabled = false;
        }
        p.changes &= !(value & (0x7F << 17));
        if value & (1 << 4) != 0 {
            p.resets += 1;
            if p.device.is_some() {
                p.enabled = true;
                p.changes |= 1 << 21;
                self.post_port(number);
            }
        }
    }

    fn input(&self, input: u64, index: usize) -> u64 {
        input + (index * self.context_size) as u64
    }

    /// Runs every command between the dequeue pointer and the first TRB
    /// whose cycle is not the controller's.
    fn commands(&mut self) {
        loop {
            let at = self.command_dequeue;
            let (parameter, _, control) = self.trb(at);
            if (control & 1 != 0) != self.command_cycle {
                return;
            }
            let kind = trb::kind(control);
            let slot = (control >> 24) as u8;
            if kind == trb::LINK {
                self.command_dequeue = parameter;
                if control & trb::TOGGLE != 0 {
                    self.command_cycle = !self.command_cycle;
                }
                continue;
            }
            self.command_dequeue += 16;
            self.commands.push((kind, slot));
            let (code, answer) = self.command(kind, parameter, control, slot);
            self.complete_command(at, code, answer);
        }
    }

    fn command(&mut self, kind: u32, parameter: u64, control: u32, slot: u8) -> (u8, u8) {
        match kind {
            trb::NOOP_COMMAND => (1, 0),
            trb::ENABLE_SLOT => {
                let free = (1..=self.config as u8).find(|n| !self.slots.contains_key(n));
                match free {
                    Some(n) => {
                        self.slots.insert(n, SlotState::default());
                        (1, n)
                    }
                    None => (9, 0), // no slots available
                }
            }
            trb::DISABLE_SLOT => {
                self.slots.remove(&slot);
                (1, slot)
            }
            trb::ADDRESS_DEVICE => {
                assert!(self.slots.contains_key(&slot), "Address Device for a slot not enabled");
                let output = self.quad(self.dcbaap + 8 * u64::from(slot));
                assert!(output != 0, "Address Device with no device context in the array");
                let add = self.word(parameter + 4);
                assert_eq!(add, 0b11, "Address Device must add the slot and endpoint 0 contexts, nothing else");
                let ctx = self.input(parameter, 1);
                let (s0, s1) = (self.word(ctx), self.word(ctx + 4));
                let port = (s1 >> 16) as u8;
                let p = self.ports[usize::from(port) - 1];
                assert!(p.enabled, "Address Device for a port that is not enabled");
                let device = p.device.expect("a device on the port");
                assert_eq!((s0 >> 20) & 0xF, u32::from(self.devices[device].speed), "the slot context's speed");
                assert!(s0 >> 27 >= 1, "the slot context's context entries");
                let ep0 = self.input(parameter, 2);
                let (e1, dequeue) = (self.word(ep0 + 4), self.quad(ep0 + 8));
                assert_eq!((e1 >> 3) & 7, 4, "endpoint 0 is a control endpoint");
                assert!(dequeue & 1 == 1 && dequeue & !0xF != 0, "endpoint 0's ring, cycle 1");
                let endpoint = Endpoint { dequeue: dequeue & !0xF, cycle: true, packet: e1 >> 16, halted: false };
                let s = self.slots.get_mut(&slot).unwrap();
                s.device = device;
                s.port = port;
                s.endpoints.insert(1, endpoint);
                (1, slot)
            }
            trb::EVALUATE_CONTEXT => {
                let add = self.word(parameter + 4);
                assert_eq!(add & 0b10, 0b10, "Evaluate Context for endpoint 0");
                let ep0 = self.input(parameter, 2);
                let packet = self.word(ep0 + 4) >> 16;
                self.slots.get_mut(&slot).expect("a slot").endpoints.get_mut(&1).unwrap().packet = packet;
                (1, slot)
            }
            trb::CONFIGURE_ENDPOINT => {
                let device = self.slots.get(&slot).expect("a slot").device;
                assert!(self.devices[device].configured.is_some(), "endpoints configured before SET_CONFIGURATION");
                let add = self.word(parameter + 4);
                assert!(add & 1 == 1, "Configure Endpoint without the slot context");
                let entries = self.word(self.input(parameter, 1)) >> 27;
                for dci in 2..32u8 {
                    if add & (1 << dci) == 0 {
                        continue;
                    }
                    assert!(u32::from(dci) <= entries, "an endpoint past the slot context's context entries");
                    let ctx = self.input(parameter, 1 + usize::from(dci));
                    let (e1, dequeue) = (self.word(ctx + 4), self.quad(ctx + 8));
                    let kind = (e1 >> 3) & 7;
                    match kind {
                        7 | 6 => assert_eq!(dci % 2, 1, "an IN endpoint has an odd DCI"),
                        2 => assert_eq!(dci % 2, 0, "an OUT endpoint has an even DCI"),
                        other => panic!("an endpoint of type {other}"),
                    }
                    assert!(dequeue & 1 == 1, "a ring starting with cycle 1");
                    let endpoint = Endpoint { dequeue: dequeue & !0xF, cycle: true, packet: e1 >> 16, halted: false };
                    self.slots.get_mut(&slot).unwrap().endpoints.insert(dci, endpoint);
                }
                (1, slot)
            }
            trb::RESET_ENDPOINT => {
                let dci = ((control >> 16) & 0x1F) as u8;
                let ep = self.slots.get_mut(&slot).and_then(|s| s.endpoints.get_mut(&dci)).expect("an endpoint");
                assert!(ep.halted, "Reset Endpoint on one that is not halted");
                ep.halted = false;
                (1, slot)
            }
            trb::STOP_ENDPOINT => {
                let dci = ((control >> 16) & 0x1F) as u8;
                let ep = self.slots.get_mut(&slot).and_then(|s| s.endpoints.get_mut(&dci)).expect("an endpoint");
                assert!(!ep.halted, "Stop Endpoint on a halted one (Reset Endpoint is for those)");
                self.stops += 1;
                (1, slot)
            }
            trb::SET_DEQUEUE => {
                let dci = ((control >> 16) & 0x1F) as u8;
                let ep = self.slots.get_mut(&slot).and_then(|s| s.endpoints.get_mut(&dci)).expect("an endpoint");
                assert!(!ep.halted, "Set TR Dequeue on a halted endpoint");
                ep.dequeue = parameter & !0xF;
                ep.cycle = parameter & 1 != 0;
                (1, slot)
            }
            other => panic!("a command the simulator does not know: {other}"),
        }
    }

    /// The next TRB on an endpoint's ring, following Link TRBs; None if
    /// the driver has not given one.
    fn next_trb(&mut self, slot: u8, dci: u8) -> Option<(u64, u64, u32, u32)> {
        loop {
            let ep = self.slots.get(&slot)?.endpoints.get(&dci)?.clone();
            if ep.halted {
                return None;
            }
            let (parameter, status, control) = self.trb(ep.dequeue);
            if (control & 1 != 0) != ep.cycle {
                return None;
            }
            let e = self.slots.get_mut(&slot).unwrap().endpoints.get_mut(&dci).unwrap();
            if trb::kind(control) == trb::LINK {
                e.dequeue = parameter;
                if control & trb::TOGGLE != 0 {
                    e.cycle = !e.cycle;
                }
                continue;
            }
            e.dequeue += 16;
            return Some((ep.dequeue, parameter, status, control));
        }
    }

    fn halt(&mut self, slot: u8, dci: u8) {
        self.slots.get_mut(&slot).unwrap().endpoints.get_mut(&dci).unwrap().halted = true;
    }

    /// Endpoint 0: Setup, perhaps Data, Status - each request whole.
    fn control(&mut self, slot: u8) {
        while let Some((at, parameter, status, control)) = self.next_trb(slot, 1) {
            assert_eq!(trb::kind(control), trb::SETUP, "a control transfer starts with a Setup TRB");
            assert!(control & trb::IMMEDIATE != 0 && status & 0x1FFFF == 8, "the setup packet is in the TRB, 8 bytes");
            let setup = Setup::from_u64(parameter);
            let _ = at;
            let mut data = None;
            let (status_trb, status_control) = loop {
                let (at, parameter, status, control) = self.next_trb(slot, 1).expect("a control transfer cut short");
                match trb::kind(control) {
                    trb::DATA => data = Some((at, parameter, status & 0x1FFFF, control)),
                    trb::STATUS => break (at, control),
                    other => panic!("TRB type {other} inside a control transfer"),
                }
            };
            let device = self.slots[&slot].device;
            self.devices[device].requests.push(setup);
            let wants_in = setup.request_type & 0x80 != 0;
            if let Some((_, _, length, control)) = data {
                assert_eq!(control & trb::IN != 0, wants_in, "the data stage's direction");
                assert_eq!(length, u32::from(setup.length), "the data stage's length");
            }
            assert_eq!(status_control & trb::IN != 0, !(wants_in && data.is_some()), "the status stage goes the other way");
            let answer = self.answer(device, setup);
            let Some(answer) = answer else {
                // Refused: a stall, and the endpoint halts.
                self.complete_transfer(data.map_or(status_trb, |d| d.0), 6, 0, slot, 1);
                self.halt(slot, 1);
                return;
            };
            if let Some((data_at, buffer, length, control)) = data {
                if wants_in {
                    let packet = self.slots[&slot].endpoints[&1].packet as usize;
                    let device_packet = usize::from(self.devices[device].descriptor[7]);
                    let first = answer.len().min(length as usize).min(device_packet);
                    if self.devices[device].speed != 4 && first > packet {
                        self.complete_transfer(data_at, 3, length, slot, 1); // babble
                        self.halt(slot, 1);
                        return;
                    }
                    let n = answer.len().min(length as usize);
                    self.at(buffer, n).copy_from_slice(&answer[..n]);
                    if n < length as usize && control & trb::SHORT_EVENT != 0 {
                        self.complete_transfer(data_at, 13, length - n as u32, slot, 1);
                    }
                }
            }
            if status_control & trb::DONE_EVENT != 0 {
                self.complete_transfer(status_trb, 1, 0, slot, 1);
            }
        }
    }

    /// What the device says to a request; None: it refuses (stalls).
    fn answer(&mut self, device: usize, setup: Setup) -> Option<Vec<u8>> {
        let d = &mut self.devices[device];
        match (setup.request_type, setup.request) {
            (0x80, 6) => match (setup.value >> 8) as u8 {
                1 => Some(d.descriptor.clone()),
                2 => Some(d.config.clone()),
                _ => None,
            },
            (0x00, 9) => {
                assert_eq!(setup.value as u8, d.config[5], "SET_CONFIGURATION with a value the device does not have");
                d.configured = Some(setup.value as u8);
                Some(Vec::new())
            }
            (0x21, 0x0B) => {
                d.protocol.insert(setup.index as u8, setup.value);
                Some(Vec::new())
            }
            (0x21, 0x0A) => {
                d.idles += 1;
                (!d.idle_stall).then(Vec::new)
            }
            (0x02, 1) if setup.value == 0 => {
                let disk = d.disk.as_mut()?;
                if setup.index & 0x80 != 0 {
                    disk.halt_cleared();
                } else {
                    disk.clear_halts += 1;
                }
                Some(Vec::new())
            }
            (0x21, 0xFF) => {
                d.disk.as_mut()?.reset();
                Some(Vec::new())
            }
            _ => None,
        }
    }

    /// Every interrupt IN endpoint with a TRB given and a report waiting.
    fn pump(&mut self) {
        let mut slots: Vec<u8> = self.slots.keys().copied().collect();
        slots.sort();
        for slot in slots {
            let mut dcis: Vec<u8> = self.slots[&slot].endpoints.keys().copied().filter(|&d| d > 1).collect();
            dcis.sort();
            for dci in dcis {
                loop {
                    let device = self.slots[&slot].device;
                    let address = 0x80 | ((dci - 1) / 2);
                    let Some(n) = self.devices[device].reports.iter().position(|(a, _)| *a == address) else { break };
                    let Some((at, buffer, status, control)) = self.next_trb(slot, dci) else { break };
                    assert_eq!(trb::kind(control), trb::NORMAL, "an interrupt endpoint is read with Normal TRBs");
                    let (_, mut bytes) = self.devices[device].reports.remove(n);
                    // A keyboard in the report protocol puts its report
                    // ID first.
                    let interface = 0;
                    if self.devices[device].protocol.get(&interface) != Some(&0) && address == 0x81 {
                        bytes.insert(0, 1);
                    }
                    let length = (status & 0x1FFFF) as usize;
                    let n = bytes.len().min(length);
                    self.at(buffer, n).copy_from_slice(&bytes[..n]);
                    let remaining = (length - n) as u32;
                    let code = if remaining > 0 && control & trb::SHORT_EVENT != 0 { 13 } else { 1 };
                    if code == 13 || control & trb::DONE_EVENT != 0 {
                        self.complete_transfer(at, code, remaining, slot, dci);
                    }
                }
            }
        }
    }

    /// A stick's bulk endpoint: OUT TRBs carry a CBW or a write's data
    /// to the disk; IN TRBs take what the disk has to send, as long as
    /// it has some. (An IN TRB the disk has nothing for waits.)
    fn bulk(&mut self, slot: u8, dci: u8) {
        let device = self.slots[&slot].device;
        loop {
            let saved = self.slots[&slot].endpoints[&dci].clone();
            let Some((at, buffer, status, control)) = self.next_trb(slot, dci) else { return };
            assert_eq!(trb::kind(control), trb::NORMAL, "a bulk endpoint is used with Normal TRBs");
            let length = (status & 0x1FFFF) as usize;
            assert!(length <= 0x10000, "a TRB of more than 64 KiB");
            assert!(buffer / 0x10000 == (buffer + length as u64 - 1) / 0x10000, "a TRB crossing a 64 KiB boundary");
            if dci.is_multiple_of(2) {
                let bytes = self.at(buffer, length).to_vec();
                self.devices[device].disk.as_mut().unwrap().receive(&bytes);
                if control & trb::DONE_EVENT != 0 {
                    self.complete_transfer(at, 1, 0, slot, dci);
                }
                continue;
            }
            match self.devices[device].disk.as_mut().unwrap().send(length) {
                None => {
                    // Nothing yet: the TRB stays the controller's.
                    *self.slots.get_mut(&slot).unwrap().endpoints.get_mut(&dci).unwrap() = saved;
                    return;
                }
                Some(Err(())) => {
                    self.complete_transfer(at, 6, length as u32, slot, dci);
                    self.halt(slot, dci);
                    return;
                }
                Some(Ok(bytes)) => {
                    self.at(buffer, bytes.len()).copy_from_slice(&bytes);
                    let remaining = (length - bytes.len()) as u32;
                    let code = if remaining > 0 && control & trb::SHORT_EVENT != 0 { 13 } else { 1 };
                    if code == 13 || control & trb::DONE_EVENT != 0 {
                        self.complete_transfer(at, code, remaining, slot, dci);
                    }
                }
            }
        }
    }

    fn write(&mut self, register: usize, value: u32) {
        if self.reset_until.is_some() {
            assert!(register == OP || register == OP + 4, "register {register:#x} written during the controller's reset");
        }
        match register {
            OP => {
                if value & 2 != 0 {
                    self.reset();
                    return;
                }
                let was_running = self.usbcmd & 1 != 0;
                self.usbcmd = value;
                if value & 1 != 0 && !was_running {
                    self.run();
                } else if value & 1 == 0 {
                    self.halted = true;
                }
            }
            r if r == OP + 4 => {} // status: write-1-to-clear bits only
            r if r == OP + 0x38 => self.config = value,
            r if (OP + 0x400..OP + 0x400 + 0x10 * self.ports.len()).contains(&r) && (r - OP - 0x400).is_multiple_of(0x10) => {
                self.write_portsc((r - OP - 0x400) / 0x10, value);
            }
            r if r == RT + 0x20 => self.iman = (self.iman & !2 | value & 2) & !(value & 1),
            r if r == RT + 0x24 => {}
            r if r == RT + 0x28 => self.erstsz = value,
            r if (DB..DB + 4 * 256).contains(&r) => {
                assert!(!self.halted, "a doorbell rung while the controller is stopped");
                let slot = ((r - DB) / 4) as u8;
                if slot == 0 {
                    self.commands();
                } else {
                    let dci = value as u8;
                    if dci == 1 {
                        self.control(slot);
                    } else if self.devices[self.slots[&slot].device].disk.is_some() {
                        self.bulk(slot, dci);
                    } else {
                        self.pump();
                    }
                }
            }
            r if r == XECP => {
                if value & (1 << 24) != 0 && self.os_asked_at.is_none() {
                    self.os_asked_at = Some(self.time);
                }
            }
            r if r == XECP + 4 => self.legacy_control = value,
            // 64-bit registers: kept until the high half comes.
            r if [OP + 0x18, OP + 0x30, RT + 0x30, RT + 0x38].contains(&r) => {
                self.halves.insert(r, value);
            }
            r if [OP + 0x1C, OP + 0x34, RT + 0x34, RT + 0x3C].contains(&r) => {
                let low = u64::from(self.halves.remove(&(r - 4)).expect("a 64-bit register's high half before its low half"));
                let full = low | u64::from(value) << 32;
                match r - 4 {
                    x if x == OP + 0x18 => self.crcr = full,
                    x if x == OP + 0x30 => self.dcbaap = full,
                    x if x == RT + 0x30 => self.erstba = full,
                    _ => self.erdp = full & !0xF,
                }
            }
            other => panic!("a write to register {other:#x}"),
        }
    }

    fn read(&mut self, register: usize) -> u32 {
        if self.reset_until.is_some_and(|until| self.time >= until) {
            self.reset_until = None;
        }
        if let (Some(at), Some(after)) = (self.os_asked_at, self.bios_release) {
            if self.time >= at + after {
                self.bios_owned = false;
            }
        }
        match register {
            0x00 => 0x40 | 0x0110 << 16,
            0x04 => 16 | 1 << 8 | (self.ports.len() as u32) << 24,
            0x08 => ((self.scratchpads >> 5) as u32) << 21 | ((self.scratchpads & 0x1F) as u32) << 27,
            0x10 => 1 | u32::from(self.context_size == 64) << 2 | if self.bios_release.is_some() { ((XECP / 4) as u32) << 16 } else { 0 },
            0x14 => DB as u32,
            0x18 => RT as u32,
            OP => self.usbcmd | if self.reset_until.is_some() { 2 } else { 0 },
            r if r == OP + 4 => u32::from(self.halted) | if self.reset_until.is_some() { 1 << 11 } else { 0 },
            r if r == OP + 8 => 1,
            r if (OP + 0x400..OP + 0x400 + 0x10 * self.ports.len()).contains(&r) => self.portsc((r - OP - 0x400) / 0x10),
            r if r == RT + 0x20 => self.iman,
            r if r == XECP => 1 | u32::from(self.bios_owned) << 16 | u32::from(self.os_asked_at.is_some()) << 24,
            r if r == XECP + 4 => self.legacy_control,
            other => panic!("a read of register {other:#x}"),
        }
    }
}

impl Bus for Sim {
    fn read(&mut self, register: usize) -> u32 {
        self.0.borrow_mut().read(register)
    }

    fn write(&mut self, register: usize, value: u32) {
        self.0.borrow_mut().write(register, value);
    }

    fn pause(&mut self) {
        self.0.borrow_mut().time += 1;
    }

    fn ticks(&mut self) -> u64 {
        self.0.borrow().time
    }

    fn size(&self) -> usize {
        self.0.borrow().registers_size
    }
}

impl Sim {
    /// The BIOS owns the controller until asked, then lets go after
    /// `after` ticks.
    pub fn bios_owns(&self, after: u64) {
        let mut s = self.0.borrow_mut();
        s.bios_release = Some(after);
        s.bios_owned = true;
        s.legacy_control = 0x0000_E011; // SMIs on, as a BIOS leaves them
    }
}

/// A simulated USB stick's insides: a SCSI disk behind the Bulk-Only
/// Transport.
#[derive(Clone, Default)]
pub struct Disk {
    /// Sectors written so far; the rest read as zeroes.
    pub sectors: HashMap<u64, Vec<u8>>,
    /// How many sectors it says it has, and how big each is.
    pub count: u64,
    pub sector_size: u32,
    pub write_protected: bool,
    pub no_medium: bool,
    /// TEST UNIT READY fails ("unit attention") this many more times.
    pub unit_attention: u32,
    /// The next data-in step stalls, and the command fails.
    pub stall_next_data: bool,
    /// The next CSW is not one (a wrong signature).
    pub garble_next_status: bool,
    /// The next data-in step never comes: the device has gone quiet.
    pub hang_next_data: bool,
    /// What it was asked: each command's opcode, in order.
    pub commands: Vec<u8>,
    pub bulk_resets: u32,
    pub clear_halts: u32,
    phase: Phase,
    /// (key, code, qualifier) for the next REQUEST SENSE.
    sense: (u8, u8, u8),
}

#[derive(Clone, Default)]
enum Phase {
    /// Waiting for a CBW.
    #[default]
    Command,
    /// Bytes to send in, then the CSW (tag, status, and the length the
    /// host asked for - for the residue).
    DataIn { bytes: Vec<u8>, sent: usize, asked: usize, tag: u32, status: u8 },
    /// Bytes expected out; what to do with them when all have come.
    DataOut { bytes: Vec<u8>, asked: usize, tag: u32, write: Option<u64> },
    /// The CSW, waiting for an IN TRB.
    Status(Vec<u8>),
    /// The data-in endpoint stalled; the CSW follows once it is cleared.
    Stalled { tag: u32, asked: usize },
    /// Silent until a Bulk-Only reset.
    Hung,
}

impl Disk {
    pub fn new(count: u64) -> Disk {
        Disk { count, sector_size: 512, unit_attention: 1, ..Disk::default() }
    }

    fn csw(tag: u32, residue: usize, status: u8) -> Vec<u8> {
        let mut c = Vec::new();
        c.extend_from_slice(&0x5342_5355u32.to_le_bytes());
        c.extend_from_slice(&tag.to_le_bytes());
        c.extend_from_slice(&(residue as u32).to_le_bytes());
        c.push(status);
        c
    }

    fn fail(&mut self, key: u8, code: u8) -> u8 {
        self.sense = (key, code, 0);
        1
    }

    fn sector(&self, n: u64) -> Vec<u8> {
        self.sectors.get(&n).cloned().unwrap_or_else(|| std::vec![0; self.sector_size as usize])
    }

    /// A CBW arrived: what happens next.
    fn command(&mut self, cbw: &[u8]) {
        assert_eq!(cbw.len(), 31, "a CBW is 31 bytes");
        assert_eq!(&cbw[0..4], b"USBC", "a CBW starts with USBC");
        let tag = u32::from_le_bytes(cbw[4..8].try_into().unwrap());
        let asked = u32::from_le_bytes(cbw[8..12].try_into().unwrap()) as usize;
        let inward = cbw[12] & 0x80 != 0;
        let cdb = &cbw[15..15 + usize::from(cbw[14])];
        self.commands.push(cdb[0]);
        let be32 = |b: &[u8]| u32::from_be_bytes(b.try_into().unwrap());
        let be64 = |b: &[u8]| u64::from_be_bytes(b.try_into().unwrap());
        // (data to send in, status), or a write's (first sector, count).
        let mut data_in: Option<(Vec<u8>, u8)> = None;
        let mut write: Option<(u64, u64)> = None;
        let mut status = 0;
        match cdb[0] {
            0x12 => {
                let mut a = std::vec![0x00, 0x80, 0x04, 0x02, 31, 0, 0, 0];
                a.extend_from_slice(b"TaajSim Simulated stick 1.00");
                data_in = Some((a, 0));
            }
            0x00 => {
                if self.no_medium {
                    status = self.fail(0x2, 0x3A);
                } else if self.unit_attention > 0 {
                    self.unit_attention -= 1;
                    status = self.fail(0x6, 0x28);
                }
            }
            0x03 => {
                let (key, code, qualifier) = core::mem::take(&mut self.sense);
                let mut s = std::vec![0u8; 18];
                s[0] = 0x70;
                s[2] = key;
                s[7] = 10;
                s[12] = code;
                s[13] = qualifier;
                data_in = Some((s, 0));
            }
            0x25 => {
                let last = (self.count - 1).min(u64::from(u32::MAX)) as u32;
                let mut a = last.to_be_bytes().to_vec();
                a.extend_from_slice(&self.sector_size.to_be_bytes());
                data_in = Some((a, 0));
            }
            0x9E if cdb[1] & 0x1F == 0x10 => {
                let mut a = (self.count - 1).to_be_bytes().to_vec();
                a.extend_from_slice(&self.sector_size.to_be_bytes());
                a.resize(32, 0);
                data_in = Some((a, 0));
            }
            0x1A => data_in = Some((std::vec![3, 0, if self.write_protected { 0x80 } else { 0 }, 0], 0)),
            0x28 | 0x88 => {
                let (first, count) = if cdb[0] == 0x28 {
                    (u64::from(be32(&cdb[2..6])), u64::from(u16::from_be_bytes([cdb[7], cdb[8]])))
                } else {
                    (be64(&cdb[2..10]), u64::from(be32(&cdb[10..14])))
                };
                if first + count > self.count {
                    data_in = Some((Vec::new(), self.fail(0x5, 0x21)));
                } else {
                    let bytes = (first..first + count).flat_map(|n| self.sector(n)).collect();
                    data_in = Some((bytes, 0));
                }
            }
            0x2A | 0x8A => {
                let (first, count) = if cdb[0] == 0x2A {
                    (u64::from(be32(&cdb[2..6])), u64::from(u16::from_be_bytes([cdb[7], cdb[8]])))
                } else {
                    (be64(&cdb[2..10]), u64::from(be32(&cdb[10..14])))
                };
                write = Some((first, count));
            }
            _ => {
                status = self.fail(0x5, 0x20); // invalid command
            }
        }
        if let Some((first, count)) = write {
            assert!(!inward, "a WRITE with its data going in");
            assert_eq!(asked, (count * u64::from(self.sector_size)) as usize, "a WRITE's CBW length");
            let ok = !self.write_protected && first + count <= self.count;
            self.phase = Phase::DataOut { bytes: Vec::new(), asked, tag, write: ok.then_some(first) };
            if !ok {
                self.sense = if self.write_protected { (0x7, 0x27, 0) } else { (0x5, 0x21, 0) };
            }
            return;
        }
        match data_in {
            Some((bytes, status)) if asked > 0 => {
                assert!(inward, "a command that sends data in, with its CBW saying out");
                if self.stall_next_data {
                    self.stall_next_data = false;
                    self.sense = (0x3, 0x11, 0); // unrecovered read error
                    self.phase = Phase::Stalled { tag, asked };
                    return;
                }
                if self.hang_next_data {
                    self.hang_next_data = false;
                    self.phase = Phase::Hung;
                    return;
                }
                self.phase = Phase::DataIn { bytes, sent: 0, asked, tag, status };
            }
            _ => {
                assert_eq!(asked, 0, "a command without data, with a CBW asking for some");
                self.phase = Phase::Status(Disk::csw(tag, 0, status));
            }
        }
    }

    /// Bytes for an IN TRB of `room` bytes: data, or the CSW. None: nothing
    /// to send yet; Err: the endpoint stalls.
    fn send(&mut self, room: usize) -> Option<Result<Vec<u8>, ()>> {
        match &mut self.phase {
            Phase::Command | Phase::DataOut { .. } | Phase::Hung => None,
            Phase::Stalled { .. } => Some(Err(())),
            Phase::DataIn { bytes, sent, asked, tag, status } => {
                let limit = (*asked).min(bytes.len());
                let n = room.min(limit - *sent);
                let out = bytes[*sent..*sent + n].to_vec();
                *sent += n;
                if *sent == limit {
                    let mut csw = Disk::csw(*tag, *asked - *sent, *status);
                    if self.garble_next_status {
                        self.garble_next_status = false;
                        csw[0] = b'X';
                    }
                    self.phase = Phase::Status(csw);
                }
                Some(Ok(out))
            }
            Phase::Status(csw) => {
                assert!(room >= 13, "the CSW read into fewer than 13 bytes");
                let out = core::mem::take(csw);
                self.phase = Phase::Command;
                Some(Ok(out))
            }
        }
    }

    /// Bytes from an OUT TRB: a CBW, or a write's data.
    fn receive(&mut self, bytes: &[u8]) {
        match &mut self.phase {
            Phase::Command => self.command(bytes),
            Phase::DataOut { bytes: got, asked, tag, write } => {
                got.extend_from_slice(bytes);
                assert!(got.len() <= *asked, "more data out than the CBW said");
                if got.len() == *asked {
                    let (tag, data, write) = (*tag, core::mem::take(got), *write);
                    let status = match write {
                        Some(first) => {
                            for (n, sector) in data.chunks(self.sector_size as usize).enumerate() {
                                self.sectors.insert(first + n as u64, sector.to_vec());
                            }
                            0
                        }
                        None => 1,
                    };
                    self.phase = Phase::Status(Disk::csw(tag, 0, status));
                }
            }
            _ => panic!("bytes out while the device waits to send its status"),
        }
    }

    /// The host cleared the stalled endpoint: the CSW comes next.
    fn halt_cleared(&mut self) {
        self.clear_halts += 1;
        if let Phase::Stalled { tag, asked } = self.phase {
            self.phase = Phase::Status(Disk::csw(tag, asked, 1));
        }
    }

    fn reset(&mut self) {
        self.bulk_resets += 1;
        self.phase = Phase::Command;
    }
}

/// Descriptors for the simulated devices.
pub mod make {
    use std::vec::Vec;

    pub fn device(packet0: u8, vendor: u16, product: u16) -> Vec<u8> {
        let mut d = std::vec![18, 1, 0x00, 0x02, 0, 0, 0, packet0];
        d.extend_from_slice(&vendor.to_le_bytes());
        d.extend_from_slice(&product.to_le_bytes());
        d.extend_from_slice(&[0, 1, 1, 2, 3, 1]);
        d
    }

    /// An interface: class, subclass, protocol, and its interrupt IN
    /// endpoint's address and packet size, if it has one.
    pub type Interface = (u8, u8, u8, Option<(u8, u16)>);

    /// A configuration with these interfaces.
    pub fn config(value: u8, interfaces: &[Interface]) -> Vec<u8> {
        let mut c = std::vec![9, 2, 0, 0, interfaces.len() as u8, value, 0, 0xA0, 50];
        for (n, &(class, subclass, protocol, endpoint)) in interfaces.iter().enumerate() {
            c.extend_from_slice(&[9, 4, n as u8, 0, u8::from(endpoint.is_some()), class, subclass, protocol, 0]);
            if class == 3 {
                // HID's own descriptor: skipped by the parser.
                c.extend_from_slice(&[9, 0x21, 0x11, 0x01, 0, 1, 0x22, 63, 0]);
            }
            if let Some((address, packet)) = endpoint {
                let p = packet.to_le_bytes();
                c.extend_from_slice(&[7, 5, address, 3, p[0], p[1], 10]);
            }
        }
        let total = (c.len() as u16).to_le_bytes();
        c[2] = total[0];
        c[3] = total[1];
        c
    }

    pub fn keyboard(speed: u8) -> super::Device {
        super::Device {
            speed,
            descriptor: device(8, 0x0627, 0x0001),
            config: config(1, &[(3, 1, 1, Some((0x81, 8)))]),
            ..super::Device::default()
        }
    }

    /// A USB stick of `sectors` sectors: one interface, mass storage /
    /// SCSI / Bulk-Only, bulk IN 0x81 and bulk OUT 0x02, 512-byte packets.
    pub fn stick(sectors: u64) -> super::Device {
        let mut c = std::vec![9, 2, 0, 0, 1, 1, 0, 0x80, 100];
        c.extend_from_slice(&[9, 4, 0, 0, 2, 8, 6, 0x50, 0]);
        c.extend_from_slice(&[7, 5, 0x81, 2, 0x00, 0x02, 0]);
        c.extend_from_slice(&[7, 5, 0x02, 2, 0x00, 0x02, 0]);
        let total = (c.len() as u16).to_le_bytes();
        c[2] = total[0];
        c[3] = total[1];
        super::Device {
            speed: 3,
            descriptor: device(64, 0x46F4, 0x0001),
            config: c,
            disk: Some(super::Disk::new(sectors)),
            ..super::Device::default()
        }
    }

    pub fn mouse(speed: u8, packet0: u8) -> super::Device {
        super::Device {
            speed,
            descriptor: device(packet0, 0x0627, 0x0002),
            config: config(1, &[(3, 1, 2, Some((0x81, 4)))]),
            ..super::Device::default()
        }
    }
}
