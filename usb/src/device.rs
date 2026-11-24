//! Enumeration: from a device on a port to one in use.
//!
//! What `attach` does, in order (the module comment of `lib.rs` says
//! why each step is there):
//!
//!   1. reset the port if it is not enabled (a USB 2 port); a USB 3 port
//!      has enabled itself;
//!   2. a slot, and an address, with endpoint 0's packet size guessed
//!      from the speed;
//!   3. the device descriptor's first 8 bytes - all a guessed packet
//!      size is sure to allow - for the real packet size; then all 18;
//!   4. the configuration: first its 9 bytes for its length, then all
//!      of it;
//!   5. for each keyboard or mouse interface that speaks the boot
//!      protocol: its interrupt IN endpoint;
//!   6. the configuration chosen, the endpoints set up, each interface
//!      put in the boot protocol and told to report only changes;
//!   7. each endpoint asked for its first report.
//!
//! A device that has no keyboard or mouse keeps its slot and address
//! (it answers, and can be told about), but nothing more: the caller
//! may give the slot back.

use crate::descriptor::{self, Configuration};
use crate::xhci::{Bus, Controller, Speed};

/// What a function of a device is, as this driver uses it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Keyboard,
    Mouse,
}

/// A keyboard or a mouse inside a device: its interface and the
/// endpoint its reports come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Function {
    pub kind: Kind,
    pub interface: u8,
    pub dci: u8,
    pub packet: u16,
}

/// A device, attached.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Attached {
    pub slot: u8,
    pub port: u8,
    pub speed: Speed,
    pub vendor: u16,
    pub product: u16,
    /// The first interface's class, for saying what a device is that is
    /// not driven.
    pub class: u8,
    pub functions: [Option<Function>; 2],
}

impl Attached {
    pub fn function(&self, dci: u8) -> Option<Function> {
        self.functions.iter().flatten().find(|f| f.dci == dci).copied()
    }
}

/// The most a report is asked for: a boot keyboard's is 8 bytes, a
/// mouse's 3 or 4; a device may say its endpoint moves more.
pub const REPORT_MAX: usize = 64;

/// Enumerates the device on `port`; see the module comment.
pub fn attach<B: Bus>(controller: &mut Controller<B>, port: u8) -> Result<Attached, &'static str> {
    let status = controller.port_status(port);
    if !status.connected {
        return Err("nothing on the port");
    }
    let speed = if status.enabled { status.speed } else { controller.reset_port(port)? };
    controller.port_seen(port);
    let slot = controller.enable_slot()?;
    match set_up(controller, slot, port, speed) {
        Ok(attached) => Ok(attached),
        Err(why) => {
            controller.disable_slot(slot);
            Err(why)
        }
    }
}

fn set_up<B: Bus>(controller: &mut Controller<B>, slot: u8, port: u8, speed: Speed) -> Result<Attached, &'static str> {
    controller.address_device(slot, port, speed)?;

    let read = |controller: &mut Controller<B>, kind: u8, length: u16| -> Result<usize, &'static str> {
        controller.control(slot, descriptor::get_descriptor(kind, length), &[]).map_err(|_| "the device would not give a descriptor")
    };
    // The real packet size of endpoint 0 (USB 3: always 512).
    if read(controller, descriptor::DEVICE, 8)? < 8 {
        return Err("the device descriptor is too short");
    }
    let packet0 = controller.control_data(8)[7];
    if !matches!(speed, Speed::Super) {
        if !matches!(packet0, 8 | 16 | 32 | 64) {
            return Err("the device's endpoint 0 packet size is not one USB allows");
        }
        controller.set_ep0_packet(slot, u16::from(packet0))?;
    }
    let n = read(controller, descriptor::DEVICE, 18)?;
    let device = descriptor::device(controller.control_data(n)).ok_or("the device descriptor is not one")?;

    let n = read(controller, descriptor::CONFIGURATION, 9)?;
    let total = descriptor::configuration_length(controller.control_data(n)).ok_or("the configuration descriptor is not one")?;
    let n = read(controller, descriptor::CONFIGURATION, total.min(4096))?;
    let config = descriptor::configuration(controller.control_data(n)).ok_or("the configuration descriptor is not one")?;

    let mut attached = Attached {
        slot,
        port,
        speed,
        vendor: device.vendor,
        product: device.product,
        class: config.interfaces[0].map_or(device.class, |i| i.class),
        functions: [None; 2],
    };
    let wanted = boot_functions(&config);
    if wanted.iter().all(Option::is_none) {
        return Ok(attached); // nothing here this driver uses
    }

    controller
        .control(slot, descriptor::set_configuration(config.value), &[])
        .map_err(|_| "the device would not take its configuration")?;
    let mut endpoints = [(0u8, 0u16, 0u8); 2];
    let mut count = 0;
    for (kind, interface) in wanted.iter().flatten() {
        endpoints[count] = interface.interrupt_in.expect("boot_functions takes only interfaces with one");
        let _ = kind;
        count += 1;
    }
    let dcis = controller.configure_interrupt_in(slot, &endpoints[..count])?;
    for (n, (kind, interface)) in wanted.iter().flatten().enumerate() {
        controller
            .control(slot, descriptor::set_boot_protocol(interface.number), &[])
            .map_err(|_| "the device would not switch to the boot protocol")?;
        // Some devices refuse SET_IDLE (a stall); they report as they
        // please, which is fine.
        let _ = controller.control(slot, descriptor::set_idle(interface.number), &[]);
        let packet = endpoints[n].1 & 0x7FF;
        attached.functions[n] = Some(Function { kind: *kind, interface: interface.number, dci: dcis[n], packet });
        controller.queue_in(slot, dcis[n], usize::from(packet).min(REPORT_MAX))?;
    }
    Ok(attached)
}

/// The keyboards and mice in a configuration that speak the boot
/// protocol and have an interrupt IN endpoint: at most two.
pub fn boot_functions(config: &Configuration) -> [Option<(Kind, descriptor::Interface)>; 2] {
    let mut found = [None; 2];
    let mut count = 0;
    for interface in config.interfaces.iter().flatten() {
        if interface.class != descriptor::CLASS_HID || interface.subclass != descriptor::HID_BOOT || interface.interrupt_in.is_none() {
            continue;
        }
        let kind = match interface.protocol {
            descriptor::HID_KEYBOARD => Kind::Keyboard,
            descriptor::HID_MOUSE => Kind::Mouse,
            _ => continue,
        };
        if count < found.len() {
            found[count] = Some((kind, *interface));
            count += 1;
        }
    }
    found
}
