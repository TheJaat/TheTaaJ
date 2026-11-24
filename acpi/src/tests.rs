//! `\_S5` read out of AML as firmware writes it, and the switching-off
//! sequence against a simulated chipset.

use super::*;
use std::vec::Vec;

/// A DSDT: its header (36 bytes, signature and length), then `body`.
fn dsdt(body: &[u8]) -> Vec<u8> {
    let mut bytes = std::vec![0u8; 36];
    bytes[0..4].copy_from_slice(b"DSDT");
    bytes.extend_from_slice(body);
    let length = bytes.len() as u32;
    bytes[4..8].copy_from_slice(&length.to_le_bytes());
    bytes
}

/// Some AML around the name, as a real DSDT has: a scope, a device, a
/// string that happens to hold the four bytes.
fn surrounded(s5: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&[0x10, 0x0B, b'\\', b'_', b'S', b'B', b'_', 0x5B, 0x82, 0x05, b'P', b'C', b'I', b'0']);
    body.extend_from_slice(&[0x0D, b'_', b'S', b'5', b'_', b'?', 0x00]); // a string "_S5_?"
    body.extend_from_slice(s5);
    body.extend_from_slice(&[0x14, 0x08, b'_', b'P', b'T', b'S', 0x01, 0xA3]); // Method (_PTS, 1) { Noop }
    body
}

#[test]
fn qemus_s5_is_read() {
    // QEMU (pc and q35): Name (\_S5, Package (4) { Zero, Zero, Zero, Zero })
    let s5 = [0x08, b'\\', b'_', b'S', b'5', b'_', 0x12, 0x06, 0x04, 0x00, 0x00, 0x00, 0x00];
    assert_eq!(sleep_type(&dsdt(&surrounded(&s5)), 5), Ok((0, 0)));
}

#[test]
fn every_encoding_of_a_number_is_read() {
    // Name (_S5, Package (4) { 0x07, 0x07, Zero, Zero }) - a typical
    // Intel chipset's, with BytePrefix.
    let s5 = [0x08, b'_', b'S', b'5', b'_', 0x12, 0x08, 0x04, 0x0A, 0x07, 0x0A, 0x07, 0x00, 0x00];
    assert_eq!(sleep_type(&dsdt(&surrounded(&s5)), 5), Ok((7, 7)));
    // One, a word, a double word, a quad word.
    let s5 = [0x08, b'_', b'S', b'5', b'_', 0x12, 0x09, 0x02, 0x01, 0x0B, 0x05, 0x00];
    assert_eq!(sleep_type(&dsdt(&s5), 5), Ok((1, 5)));
    let s5 = [0x08, b'_', b'S', b'5', b'_', 0x12, 0x10, 0x02, 0x0C, 0x06, 0, 0, 0, 0x0E, 0x03, 0, 0, 0, 0, 0, 0, 0];
    assert_eq!(sleep_type(&dsdt(&s5), 5), Ok((6, 3)));
    // A two-byte package length (lead byte 0x40 | low nibble, then a byte).
    let s5 = [0x08, b'_', b'S', b'5', b'_', 0x12, 0x46, 0x00, 0x02, 0x0A, 0x05, 0x00];
    assert_eq!(sleep_type(&dsdt(&s5), 5), Ok((5, 0)));
    // The old form: one element, a in the low byte, b in the next.
    let s5 = [0x08, b'_', b'S', b'5', b'_', 0x12, 0x05, 0x01, 0x0B, 0x07, 0x05];
    assert_eq!(sleep_type(&dsdt(&s5), 5), Ok((7, 5)));
    // Only three bits count.
    let s5 = [0x08, b'_', b'S', b'5', b'_', 0x12, 0x06, 0x02, 0x0A, 0x0F, 0x0A, 0x09];
    assert_eq!(sleep_type(&dsdt(&s5), 5), Ok((7, 1)));
}

#[test]
fn other_states_and_other_names_are_not_taken_for_s5() {
    let mut body = Vec::new();
    body.extend_from_slice(&[0x08, b'_', b'S', b'3', b'_', 0x12, 0x06, 0x02, 0x0A, 0x01, 0x0A, 0x01]);
    body.extend_from_slice(&[0x08, b'_', b'S', b'5', b'_', 0x12, 0x06, 0x02, 0x0A, 0x02, 0x0A, 0x03]);
    let table = dsdt(&body);
    assert_eq!(sleep_type(&table, 3), Ok((1, 1)));
    assert_eq!(sleep_type(&table, 5), Ok((2, 3)));
    assert_eq!(sleep_type(&table, 4), Err("no \\_S5 in the DSDT: the firmware says nothing about switching off"));
    assert!(sleep_type(&table, 6).is_err());
}

#[test]
fn what_is_not_a_constant_package_is_refused() {
    assert_eq!(
        sleep_type(&dsdt(&surrounded(&[])), 5),
        Err("no \\_S5 in the DSDT: the firmware says nothing about switching off"),
        "only in a string"
    );
    // Method (_S5, 0) { Return (Package () { 5, 5 }) }
    let method = [0x14, 0x0E, b'_', b'S', b'5', b'_', 0x00, 0xA4, 0x12, 0x06, 0x02, 0x0A, 0x05, 0x0A, 0x05];
    assert_eq!(sleep_type(&dsdt(&method), 5), Err("\\_S5 is a method, not a package: it needs an AML interpreter"));
    // Name (_S5, Zero) - not a package.
    assert_eq!(sleep_type(&dsdt(&[0x08, b'_', b'S', b'5', b'_', 0x00]), 5), Err("\\_S5 is not a package of numbers"));
    // A package whose element is a variable (Local0, 0x60), not a number.
    let s5 = [0x08, b'_', b'S', b'5', b'_', 0x12, 0x05, 0x02, 0x60, 0x0A, 0x05];
    assert_eq!(sleep_type(&dsdt(&s5), 5), Err("\\_S5 is not a package of numbers"));
    // Cut short.
    let s5 = [0x08, b'_', b'S', b'5', b'_', 0x12, 0x06, 0x02, 0x0A];
    assert_eq!(sleep_type(&dsdt(&s5), 5), Err("\\_S5 is not a package of numbers"));
    let s5 = [0x08, b'_', b'S', b'5', b'_', 0x12, 0x02, 0x00];
    assert_eq!(sleep_type(&dsdt(&s5), 5), Err("\\_S5 is not a package of numbers"), "no elements");
}

/// A chipset: PM1a and PM1b control registers, an SMI command port,
/// a reset port. ACPI mode comes `mode_delay` ticks after the SMI write
/// of the right value.
#[derive(Default)]
struct Chipset {
    pm1a: u16,
    pm1b: u16,
    time: u64,
    mode_at: Option<u64>,
    mode_delay: u64,
    /// Every write: (port, value).
    writes: Vec<(u16, u16)>,
    /// The sleep type entered, by PM1a (the register that acts).
    entered: Option<u16>,
    reset_with: Option<u8>,
    /// The SMI handler is broken: ACPI mode never comes.
    stuck: bool,
}

const PM1A: u16 = 0x604;
const PM1B: u16 = 0x704;
const SMI: u16 = 0xB2;
const RESET: u16 = 0xCF9;

fn controls() -> Controls {
    Controls { pm1a_control: PM1A, pm1b_control: 0, smi_command: SMI, acpi_enable: 0xF1, reset_port: RESET, reset_value: 0x06 }
}

impl Ports for Chipset {
    fn read16(&mut self, port: u16) -> u16 {
        if self.mode_at.is_some_and(|at| self.time >= at) {
            self.pm1a |= pm1::SCI_ENABLE;
            self.pm1b |= pm1::SCI_ENABLE;
        }
        match port {
            PM1A => self.pm1a,
            PM1B => self.pm1b,
            _ => panic!("a read of port {port:#x}"),
        }
    }

    fn write16(&mut self, port: u16, value: u16) {
        self.writes.push((port, value));
        // SLP_EN reads as 0: it is an action, not a state.
        let kept = value & !pm1::SLEEP_ENABLE;
        match port {
            PM1A => {
                self.pm1a = kept;
                if value & pm1::SLEEP_ENABLE != 0 {
                    assert!(self.pm1a & pm1::SCI_ENABLE != 0, "SLP_EN before ACPI mode");
                    self.entered = Some((value & pm1::SLEEP_TYPE_MASK) >> pm1::SLEEP_TYPE_SHIFT);
                }
            }
            PM1B => self.pm1b = kept,
            _ => panic!("a 16-bit write of port {port:#x}"),
        }
    }

    fn write8(&mut self, port: u16, value: u8) {
        self.writes.push((port, u16::from(value)));
        match port {
            SMI if value == 0xF1 && !self.stuck => self.mode_at = Some(self.time + self.mode_delay),
            SMI => {}
            RESET => self.reset_with = Some(value),
            _ => panic!("an 8-bit write of port {port:#x}"),
        }
    }

    fn pause(&mut self) {
        self.time += 1;
    }

    fn ticks(&mut self) -> u64 {
        self.time
    }
}

#[test]
fn switching_off_enters_acpi_mode_then_s5() {
    let mut chipset = Chipset { mode_delay: 5, pm1a: 0x0200, ..Chipset::default() }; // some other bit set
    let why = switch_off(&mut chipset, &controls(), (5, 0));
    assert_eq!(why, "the machine is still on after SLP_EN", "the simulator does not stop the program");
    assert_eq!(chipset.entered, Some(5));
    // ACPI_ENABLE to the SMI port; then SLP_TYP alone, then with SLP_EN -
    // the other bits kept, SCI_EN among them.
    assert_eq!(chipset.writes, std::vec![(SMI, 0xF1), (PM1A, 0x0200 | 1 | 5 << 10), (PM1A, 0x0200 | 1 | 5 << 10 | 1 << 13)]);
    assert!(chipset.time >= STOP_WAIT, "it waited before saying it did not work");
}

#[test]
fn a_machine_in_acpi_mode_is_not_switched_again() {
    let mut chipset = Chipset { pm1a: pm1::SCI_ENABLE, ..Chipset::default() };
    switch_off(&mut chipset, &Controls { smi_command: 0, ..controls() }, (0, 0));
    assert_eq!(chipset.entered, Some(0));
    assert!(chipset.writes.iter().all(|&(port, _)| port != SMI));
}

#[test]
fn both_pm1_registers_get_their_own_type() {
    let mut chipset = Chipset { pm1a: pm1::SCI_ENABLE, pm1b: pm1::SCI_ENABLE | 0x1C00, ..Chipset::default() };
    switch_off(&mut chipset, &Controls { pm1b_control: PM1B, ..controls() }, (7, 3));
    assert_eq!(chipset.entered, Some(7));
    assert_eq!(
        chipset.writes,
        std::vec![(PM1A, 1 | 7 << 10), (PM1B, 1 | 3 << 10), (PM1A, 1 | 7 << 10 | 1 << 13), (PM1B, 1 | 3 << 10 | 1 << 13)],
        "the old SLP_TYP in PM1b cleared, not ORed into"
    );
}

#[test]
fn a_chipset_that_stays_in_legacy_mode_is_reported() {
    let mut chipset = Chipset { stuck: true, ..Chipset::default() };
    assert_eq!(switch_off(&mut chipset, &controls(), (0, 0)), "the machine did not switch into ACPI mode");
    assert_eq!(chipset.entered, None, "SLP_EN never written");
    assert!(chipset.time >= ACPI_MODE_WAIT);
    let mut chipset = Chipset::default();
    assert_eq!(
        switch_off(&mut chipset, &Controls { smi_command: 0, ..controls() }, (0, 0)),
        "the machine is not in ACPI mode, and the FADT names no way to switch it"
    );
}

#[test]
fn reset_writes_the_reset_value() {
    let mut chipset = Chipset::default();
    assert_eq!(reset(&mut chipset, &controls()), "the machine is still running after the reset write");
    assert_eq!(chipset.reset_with, Some(0x06));
    assert_eq!(chipset.writes, std::vec![(RESET, 0x06)]);
}
