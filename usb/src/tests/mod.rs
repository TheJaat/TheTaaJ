//! The xHCI driver and enumeration, against `sim`.

mod sim;

use crate::descriptor;
use crate::device::{self, Kind};
use crate::xhci::{self, layout, trb, Controller, Dma, Event, Speed};
use sim::{make, Sim, PHYSICAL};
use std::vec::Vec;

fn start(sim: &Sim) -> Result<Controller<Sim>, &'static str> {
    let (base, size) = {
        let mut s = sim.0.borrow_mut();
        (s.memory.as_mut_ptr(), s.memory.len())
    };
    // SAFETY: the simulator's memory, which lives as long as `sim`.
    let dma = unsafe { Dma::new(base, PHYSICAL, size) };
    Controller::start(sim.clone(), dma, true)
}

/// Every event now waiting.
fn events(controller: &mut Controller<Sim>) -> Vec<Event> {
    std::iter::from_fn(|| controller.next_event()).collect()
}

/// The reports that came from `slot`, each asked for again - until no
/// more come (asking again may bring the next one at once).
fn reports(controller: &mut Controller<Sim>, attached: &device::Attached) -> Vec<(Kind, Vec<u8>)> {
    let mut out = Vec::new();
    while let Some(event) = controller.next_event() {
        if let Event::Transfer { slot, endpoint, code, remaining, .. } = event {
            assert_eq!(slot, attached.slot);
            assert!(code == xhci::code::SUCCESS || code == xhci::code::SHORT_PACKET, "code {code}");
            let f = attached.function(endpoint).expect("a report from an endpoint set up");
            let length = usize::from(f.packet) - remaining as usize;
            out.push((f.kind, controller.endpoint_data(slot, endpoint, length).to_vec()));
            controller.queue_in(slot, endpoint, usize::from(f.packet)).unwrap();
        }
    }
    out
}

#[test]
fn the_controller_is_taken_from_the_firmware_reset_and_started() {
    let sim = Sim::new(4, 2);
    sim.bios_owns(5);
    sim.0.borrow_mut().scratchpads = 4;
    let mut controller = start(&sim).expect("it starts");
    let s = sim.0.borrow();
    assert_eq!(s.resets, 1);
    assert_eq!(s.legacy_control & 0xE01F, 0x0000, "the BIOS's SMIs switched off");
    assert_eq!(s.legacy_control >> 29, 0x7, "and its SMI events cleared");
    drop(s);
    assert_eq!(controller.ports, 4);
    assert_eq!(controller.scratchpads, 4);
    controller.noop().expect("the command ring works");
    // 300 commands: round the 256-TRB ring, through its Link TRB.
    for _ in 0..300 {
        controller.noop().unwrap();
    }
    assert_eq!(sim.0.borrow().commands.len(), 301);
}

#[test]
fn a_controller_that_will_not_be_had_is_refused() {
    let sim = Sim::new(4, 2);
    sim.bios_owns(u64::MAX);
    assert_eq!(start(&sim).err(), Some("the firmware would not give the controller up"));
    let sim = Sim::new(4, 2);
    sim.0.borrow_mut().scratchpads = 40;
    assert_eq!(start(&sim).err(), Some("the controller asks for more scratchpad pages than this driver has"));
}

#[test]
fn registers_cut_short_are_refused() {
    // The kernel keeps an MSI-X table that shares the BAR: what is left
    // must hold every register used. The sim's doorbells are at 0x2000.
    let sim = Sim::new(4, 2);
    sim.0.borrow_mut().registers_size = 0x2000;
    assert_eq!(start(&sim).err(), Some("some of its registers are beyond the memory given for them"));
    let sim = Sim::new(4, 2);
    sim.0.borrow_mut().registers_size = 0x2000 + 4 * (usize::from(xhci::SLOTS) + 1);
    assert!(start(&sim).is_ok(), "enough for a doorbell per slot");
}

#[test]
fn a_keyboard_on_a_usb2_port_is_enumerated_and_reports() {
    let sim = Sim::new(4, 2);
    let kbd = sim.plug(1, make::keyboard(1));
    let mut controller = start(&sim).unwrap();
    assert_eq!(events(&mut controller), [Event::Port { port: 1 }]);
    let attached = device::attach(&mut controller, 1).expect("the keyboard attaches");
    assert_eq!((attached.slot, attached.port, attached.speed), (1, 1, Speed::Full));
    assert_eq!((attached.vendor, attached.product), (0x0627, 0x0001));
    let f = attached.functions[0].expect("a keyboard");
    assert_eq!((f.kind, f.interface, f.dci, f.packet), (Kind::Keyboard, 0, 3, 8));
    assert_eq!(attached.functions[1], None);
    {
        let s = sim.0.borrow();
        assert_eq!(s.ports[0].resets, 1, "a USB 2 port is reset");
        let d = &s.devices[kbd];
        assert_eq!(d.configured, Some(1));
        assert_eq!(d.protocol.get(&0), Some(&0), "the boot protocol");
        assert_eq!(d.idles, 1);
        let asked: Vec<(u8, u16)> = d.requests.iter().map(|r| (r.request, r.length)).collect();
        assert_eq!(asked, [(6, 8), (6, 18), (6, 9), (6, d.config.len() as u16), (9, 0), (0x0B, 0), (0x0A, 0)]);
    }
    // Keys.
    sim.report(kbd, 0x81, &[0x02, 0, 0x04, 0, 0, 0, 0, 0]);
    sim.report(kbd, 0x81, &[0, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(
        reports(&mut controller, &attached),
        [(Kind::Keyboard, std::vec![0x02, 0, 0x04, 0, 0, 0, 0, 0]), (Kind::Keyboard, std::vec![0; 8])]
    );
}

#[test]
fn hundreds_of_reports_go_round_every_ring() {
    let sim = Sim::new(2, 2);
    let kbd = sim.plug(2, make::keyboard(1));
    let mut controller = start(&sim).unwrap();
    events(&mut controller);
    let attached = device::attach(&mut controller, 2).unwrap();
    // 300: the endpoint's 32-TRB ring nine times, the event ring once.
    let mut got = Vec::new();
    for n in 0..300u32 {
        sim.report(kbd, 0x81, &[0, 0, (n % 200) as u8 + 4, 0, 0, 0, 0, 0]);
        got.extend(reports(&mut controller, &attached));
    }
    assert_eq!(got.len(), 300);
    assert!(got.iter().enumerate().all(|(n, (_, r))| r[2] == (n % 200) as u8 + 4), "in order, none lost");
}

#[test]
fn a_device_with_bigger_packets_gets_endpoint_0_resized() {
    // A full-speed mouse whose endpoint 0 takes 64 bytes: without the
    // Evaluate Context, reading its 18-byte descriptor is babble.
    let sim = Sim::new(2, 2);
    let mouse = sim.plug(1, make::mouse(1, 64));
    let mut controller = start(&sim).unwrap();
    let attached = device::attach(&mut controller, 1).expect("the mouse attaches");
    assert_eq!(attached.functions[0].map(|f| (f.kind, f.packet)), Some((Kind::Mouse, 4)));
    assert!(sim.0.borrow().commands.iter().any(|&(kind, _)| kind == trb::EVALUATE_CONTEXT));
    // A three-byte report into a four-byte request: a short packet.
    sim.report(mouse, 0x81, &[1, 5, 0xFB]);
    assert_eq!(reports(&mut controller, &attached), [(Kind::Mouse, std::vec![1, 5, 0xFB])]);
}

#[test]
fn usb3_ports_and_64_byte_contexts() {
    let sim = Sim::new(4, 2);
    sim.0.borrow_mut().context_size = 64;
    let kbd = sim.plug(3, make::keyboard(4));
    let mut controller = start(&sim).unwrap();
    let attached = device::attach(&mut controller, 3).unwrap();
    assert_eq!(attached.speed, Speed::Super);
    assert_eq!(sim.0.borrow().ports[2].resets, 0, "a USB 3 port is not reset");
    sim.report(kbd, 0x81, &[0, 0, 0x05, 0, 0, 0, 0, 0]);
    assert_eq!(reports(&mut controller, &attached)[0].1[2], 0x05);
}

#[test]
fn a_keyboard_and_mouse_in_one_device_are_both_used() {
    // A wireless receiver: two interfaces, two endpoints.
    let sim = Sim::new(2, 2);
    let combo = sim::Device {
        speed: 1,
        descriptor: make::device(8, 0x046D, 0xC52B),
        config: make::config(1, &[(3, 1, 1, Some((0x81, 8))), (3, 1, 2, Some((0x82, 8)))]),
        ..sim::Device::default()
    };
    let d = sim.plug(1, combo);
    let mut controller = start(&sim).unwrap();
    let attached = device::attach(&mut controller, 1).unwrap();
    let kinds: Vec<(Kind, u8)> = attached.functions.iter().flatten().map(|f| (f.kind, f.dci)).collect();
    assert_eq!(kinds, [(Kind::Keyboard, 3), (Kind::Mouse, 5)]);
    let configures = sim.0.borrow().commands.iter().filter(|&&(kind, _)| kind == trb::CONFIGURE_ENDPOINT).count();
    assert_eq!(configures, 1, "both endpoints in one command");
    sim.report(d, 0x82, &[2, 1, 1]);
    sim.report(d, 0x81, &[0, 0, 0x06, 0, 0, 0, 0, 0]);
    let mut got = reports(&mut controller, &attached);
    got.sort_by_key(|(kind, _)| *kind == Kind::Mouse);
    assert_eq!(got, [(Kind::Keyboard, std::vec![0, 0, 0x06, 0, 0, 0, 0, 0]), (Kind::Mouse, std::vec![2, 1, 1])]);
}

#[test]
fn a_device_that_is_not_a_keyboard_or_mouse_is_left_alone() {
    let sim = Sim::new(2, 2);
    let disk = sim::Device {
        speed: 3,
        descriptor: make::device(64, 0x0781, 0x5567),
        config: make::config(1, &[(8, 6, 0x50, None)]),
        ..sim::Device::default()
    };
    let d = sim.plug(1, disk);
    let mut controller = start(&sim).unwrap();
    let attached = device::attach(&mut controller, 1).unwrap();
    assert_eq!(attached.class, descriptor::CLASS_STORAGE);
    assert!(attached.functions.iter().all(Option::is_none));
    assert_eq!(sim.0.borrow().devices[d].configured, None, "not configured");
    controller.disable_slot(attached.slot);
    assert!(sim.0.borrow().commands.contains(&(trb::DISABLE_SLOT, attached.slot)));
}

#[test]
fn a_refused_request_halts_endpoint_0_and_it_is_recovered() {
    let sim = Sim::new(2, 2);
    let mut kbd = make::keyboard(1);
    kbd.idle_stall = true;
    let d = sim.plug(1, kbd);
    let mut controller = start(&sim).unwrap();
    let attached = device::attach(&mut controller, 1).expect("SET_IDLE refused is not fatal");
    let commands = sim.0.borrow().commands.clone();
    assert!(commands.contains(&(trb::RESET_ENDPOINT, attached.slot)) && commands.contains(&(trb::SET_DEQUEUE, attached.slot)));
    // Endpoint 0 works again.
    let n = controller.control(attached.slot, descriptor::get_descriptor(descriptor::DEVICE, 18), &[]).expect("a request after the stall");
    assert_eq!(descriptor::device(controller.control_data(n)).map(|d| d.product), Some(0x0001));
    sim.report(d, 0x81, &[0, 0, 0x07, 0, 0, 0, 0, 0]);
    assert_eq!(reports(&mut controller, &attached).len(), 1);
}

#[test]
fn devices_come_and_go() {
    let sim = Sim::new(2, 2);
    let mut controller = start(&sim).unwrap();
    assert!(events(&mut controller).is_empty());
    let kbd = sim.plug(2, make::keyboard(2)); // low speed
    assert_eq!(events(&mut controller), [Event::Port { port: 2 }]);
    let attached = device::attach(&mut controller, 2).unwrap();
    assert_eq!(attached.speed, Speed::Low);
    sim.report(kbd, 0x81, &[0, 0, 0x08, 0, 0, 0, 0, 0]);
    assert_eq!(reports(&mut controller, &attached).len(), 1);
    sim.unplug(2);
    assert_eq!(events(&mut controller), [Event::Port { port: 2 }]);
    assert!(!controller.port_status(2).connected);
    controller.port_seen(2);
    controller.disable_slot(attached.slot);
    // Plugged in again: a slot again (the same number, free once more).
    sim.plug(2, make::keyboard(1));
    events(&mut controller);
    let again = device::attach(&mut controller, 2).unwrap();
    assert_eq!(again.slot, attached.slot);
}

#[test]
fn events_that_come_while_waiting_are_kept() {
    let sim = Sim::new(2, 2);
    let kbd = sim.plug(1, make::keyboard(1));
    let mut controller = start(&sim).unwrap();
    let attached = device::attach(&mut controller, 1).unwrap();
    // A report arrives, then a second device is plugged in: its
    // enumeration's commands wait past the report's event.
    sim.report(kbd, 0x81, &[0, 0, 0x09, 0, 0, 0, 0, 0]);
    sim.plug(2, make::mouse(1, 8));
    let second = device::attach(&mut controller, 2).unwrap();
    assert_eq!(second.slot, 2);
    let kept: Vec<Event> = events(&mut controller);
    assert!(kept.iter().any(|e| matches!(e, Event::Transfer { slot: 1, endpoint: 3, .. })), "the report's event was kept: {kept:?}");
    assert!(kept.contains(&Event::Port { port: 2 }));
    let _ = attached;
}

#[test]
fn configurations_are_read_carefully() {
    let good = make::config(2, &[(3, 1, 1, Some((0x81, 8))), (8, 6, 0x50, None)]);
    let c = descriptor::configuration(&good).unwrap();
    assert_eq!(c.value, 2);
    assert_eq!(c.interfaces[0].unwrap().interrupt_in, Some((0x81, 8, 10)));
    assert_eq!(c.interfaces[1].unwrap().class, 8);
    // A zero-length descriptor in the middle ends the walk; it does not loop.
    let mut bad = good.clone();
    bad[9] = 0;
    assert!(descriptor::configuration(&bad).unwrap().interfaces.iter().all(Option::is_none));
    // A descriptor running past the end is not read.
    let mut cut = good.clone();
    cut.truncate(20);
    let c = descriptor::configuration(&cut).unwrap();
    assert_eq!(c.interfaces[0].unwrap().interrupt_in, None);
    // An alternate setting's endpoints are not the interface's; an OUT
    // endpoint is not an IN one.
    let mut alt = make::config(1, &[(3, 1, 1, None)]);
    alt.extend_from_slice(&[9, 4, 0, 1, 1, 3, 1, 1, 0, 7, 5, 0x81, 3, 8, 0, 10]);
    alt.extend_from_slice(&[]);
    let total = (alt.len() as u16).to_le_bytes();
    alt[2] = total[0];
    alt[3] = total[1];
    assert_eq!(descriptor::configuration(&alt).unwrap().interfaces[0].unwrap().interrupt_in, None);
    let out = make::config(1, &[(3, 1, 1, Some((0x01, 8)))]);
    assert_eq!(descriptor::configuration(&out).unwrap().interfaces[0].unwrap().interrupt_in, None);
    assert!(descriptor::configuration(&[9, 4, 0, 0, 0, 0, 0, 0, 0]).is_none(), "not a configuration");
}

#[test]
fn intervals_are_converted() {
    assert_eq!(xhci::xhci_interval(Speed::Full, 10), 6); // 10 ms -> 8 ms
    assert_eq!(xhci::xhci_interval(Speed::Low, 1), 3); // 1 ms
    assert_eq!(xhci::xhci_interval(Speed::Full, 255), 10); // the most
    assert_eq!(xhci::xhci_interval(Speed::High, 4), 3); // 1 ms
    assert_eq!(xhci::xhci_interval(Speed::High, 0), 0);
    assert_eq!(layout::device(1), layout::DEVICES);
}
