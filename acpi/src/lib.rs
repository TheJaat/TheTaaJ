//! ACPI power control, as `power` (the program) does it: switching a PC
//! off and resetting it.
//!
//! The kernel hands `power` the DSDT and the I/O ports the FADT names
//! (kernel64/src/power.rs). What to write to them is here:
//!
//! - **Which sleep type means "off".** ACPI's sleep states are S0 (on)
//!   to S5 ("soft off"). Each chipset has its own number for each state
//!   (the SLP_TYP field of the PM1 control register), and the firmware
//!   says which, in the DSDT, as AML: a named package, `\_S5`, whose
//!   first two elements are SLP_TYPa (for PM1a) and SLP_TYPb (PM1b).
//!   AML is ACPI's byte code: the firmware's program, in which the
//!   operating system is meant to run methods. Reading one named
//!   constant needs no interpreter, only the few encodings a constant
//!   can have - `sleep_type` decodes those and refuses everything else.
//!
//! - **ACPI mode.** A PC starts in "legacy" mode, where the firmware
//!   (in System Management Mode) handles power events itself. Writing
//!   ACPI_ENABLE to the SMI command port hands them to the operating
//!   system; SCI_EN in PM1 control says the switch has happened.
//!
//! - **The sequence** (`switch_off`): ACPI mode on, SLP_TYP set in each
//!   PM1 control register, then SLP_EN with it - which is the moment the
//!   machine goes off. A program that is still running afterwards knows
//!   it did not work.
//!
//! Like the disk drivers, all of it is written against a trait, `Ports`,
//! so it runs on the host against a simulated chipset.

#![no_std]

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod tests;

/// The PM1 control register's bits.
pub mod pm1 {
    /// SCI_EN: power events go to the operating system (ACPI mode).
    pub const SCI_ENABLE: u16 = 1 << 0;
    /// SLP_TYP: which sleep state SLP_EN enters - three bits.
    pub const SLEEP_TYPE_SHIFT: u16 = 10;
    pub const SLEEP_TYPE_MASK: u16 = 0b111 << SLEEP_TYPE_SHIFT;
    /// SLP_EN: enter it, now. Reads as 0.
    pub const SLEEP_ENABLE: u16 = 1 << 13;
}

/// AML's encodings, as far as a named constant goes.
mod aml {
    pub const NAME: u8 = 0x08;
    pub const METHOD: u8 = 0x14;
    pub const PACKAGE: u8 = 0x12;
    pub const ROOT: u8 = b'\\';
    pub const PARENT: u8 = b'^';
    pub const ZERO: u8 = 0x00;
    pub const ONE: u8 = 0x01;
    pub const BYTE: u8 = 0x0A;
    pub const WORD: u8 = 0x0B;
    pub const DWORD: u8 = 0x0C;
    pub const QWORD: u8 = 0x0E;
}

/// The two SLP_TYP values for sleep state `\_Sn`: (for PM1a, for PM1b).
///
/// `aml` is the DSDT (its header may be included: a name is looked for
/// by its four bytes). It is found as AML writes `Name (_S5, Package
/// () { a, b, ... })`: NameOp, the name (perhaps after `\` or `^`),
/// PackageOp, the package's length (1 to 4 bytes), its element count,
/// then the elements, each a constant - Zero, One, or a byte, word,
/// double or quad word after its prefix. A package with one element is
/// the old form, both values in it: a in the low byte, b in the next.
///
/// Errors: no such name; a `\_Sn` that is a method (it would need an
/// interpreter to run); or something else where the package should be.
pub fn sleep_type(aml: &[u8], state: u8) -> Result<(u8, u8), &'static str> {
    if state > 5 {
        return Err("there are no sleep states past S5");
    }
    let name = [b'_', b'S', b'0' + state, b'_'];
    let mut found_method = false;
    for at in (0..aml.len().saturating_sub(4)).filter(|&at| aml[at..at + 4] == name) {
        // What comes before the name: its op, and perhaps a prefix.
        let mut before = at;
        while before > 0 && matches!(aml[before - 1], aml::ROOT | aml::PARENT) {
            before -= 1;
        }
        match before.checked_sub(1).map(|op| aml[op]) {
            Some(aml::NAME) => return package(&aml[at + 4..]),
            // A method's name comes after its op and its length; a
            // `\_S5` method is rare, and not readable without running it.
            _ if at >= 2 && aml[..at].iter().rev().take(5).any(|&b| b == aml::METHOD) => found_method = true,
            _ => {} // the four bytes inside something else: a string, a path
        }
    }
    Err(if found_method {
        "\\_S5 is a method, not a package: it needs an AML interpreter"
    } else {
        "no \\_S5 in the DSDT: the firmware says nothing about switching off"
    })
}

/// The package after the name: its first two elements.
fn package(bytes: &[u8]) -> Result<(u8, u8), &'static str> {
    let bad = "\\_S5 is not a package of numbers";
    let (&op, rest) = bytes.split_first().ok_or(bad)?;
    if op != aml::PACKAGE {
        return Err(bad);
    }
    // PkgLength: the lead byte's top two bits say how many bytes follow.
    let lead = *rest.first().ok_or(bad)?;
    let skip = 1 + usize::from(lead >> 6);
    let rest = rest.get(skip..).ok_or(bad)?;
    let (&count, mut rest) = rest.split_first().ok_or(bad)?;
    if count == 0 {
        return Err(bad);
    }
    let mut values = [0u64; 2];
    for value in values.iter_mut().take(usize::from(count).min(2)) {
        let (number, after) = integer(rest).ok_or(bad)?;
        *value = number;
        rest = after;
    }
    if count == 1 {
        // The old form: both values in one number.
        return Ok(((values[0] & 0x7) as u8, ((values[0] >> 8) & 0x7) as u8));
    }
    Ok(((values[0] & 0x7) as u8, (values[1] & 0x7) as u8))
}

/// One constant: its value and what follows it.
fn integer(bytes: &[u8]) -> Option<(u64, &[u8])> {
    let (&op, rest) = bytes.split_first()?;
    let wide = |n: usize| -> Option<(u64, &[u8])> {
        let (number, after) = (rest.get(..n)?, &rest[n..]);
        Some((number.iter().rev().fold(0u64, |sum, &b| sum << 8 | u64::from(b)), after))
    };
    match op {
        aml::ZERO => Some((0, rest)),
        aml::ONE => Some((1, rest)),
        aml::BYTE => wide(1),
        aml::WORD => wide(2),
        aml::DWORD => wide(4),
        aml::QWORD => wide(8),
        _ => None,
    }
}

/// The machine's I/O ports, as the program reaches them - or a
/// simulated chipset, in the tests.
pub trait Ports {
    fn read16(&mut self, port: u16) -> u16;
    fn write16(&mut self, port: u16, value: u16);
    fn write8(&mut self, port: u16, value: u8);
    /// Waits a little: a tick.
    fn pause(&mut self);
    /// The time, in ticks (100 a second).
    fn ticks(&mut self) -> u64;
}

/// Where this machine's controls are (from the kernel's PowerInfo).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Controls {
    pub pm1a_control: u16,
    /// 0: none.
    pub pm1b_control: u16,
    /// 0: none - the machine is in ACPI mode always.
    pub smi_command: u16,
    pub acpi_enable: u8,
    pub reset_port: u16,
    pub reset_value: u8,
}

/// How long the chipset may take to switch into ACPI mode: 3 s.
pub const ACPI_MODE_WAIT: u64 = 300;
/// How long a machine switched off or reset may take to stop: 2 s.
/// Still running after that, it did not work.
pub const STOP_WAIT: u64 = 200;

/// Switches the chipset into ACPI mode, if it is not in it.
pub fn enable_acpi_mode(ports: &mut impl Ports, controls: &Controls) -> Result<(), &'static str> {
    if ports.read16(controls.pm1a_control) & pm1::SCI_ENABLE != 0 {
        return Ok(());
    }
    if controls.smi_command == 0 {
        return Err("the machine is not in ACPI mode, and the FADT names no way to switch it");
    }
    ports.write8(controls.smi_command, controls.acpi_enable);
    let deadline = ports.ticks() + ACPI_MODE_WAIT;
    while ports.read16(controls.pm1a_control) & pm1::SCI_ENABLE == 0 {
        if ports.ticks() >= deadline {
            return Err("the machine did not switch into ACPI mode");
        }
        ports.pause();
    }
    Ok(())
}

/// Switches the machine off: sleep state S5, with `types` from
/// `sleep_type(dsdt, 5)`. Returns only if it did not work.
pub fn switch_off(ports: &mut impl Ports, controls: &Controls, types: (u8, u8)) -> &'static str {
    if let Err(why) = enable_acpi_mode(ports, controls) {
        return why;
    }
    // SLP_TYP first, with every other bit as it was; then SLP_EN with
    // it, PM1a then PM1b - the second write is the one that acts.
    let registers = [(controls.pm1a_control, types.0), (controls.pm1b_control, types.1)];
    let mut values = [0u16; 2];
    for (value, &(port, kind)) in values.iter_mut().zip(&registers).filter(|(_, &(port, _))| port != 0) {
        let old = ports.read16(port) & !(pm1::SLEEP_TYPE_MASK | pm1::SLEEP_ENABLE);
        *value = old | u16::from(kind & 0x7) << pm1::SLEEP_TYPE_SHIFT;
        ports.write16(port, *value);
    }
    for (value, &(port, _)) in values.iter().zip(&registers).filter(|(_, &(port, _))| port != 0) {
        ports.write16(port, value | pm1::SLEEP_ENABLE);
    }
    wait_out(ports);
    "the machine is still on after SLP_EN"
}

/// Resets the machine. Returns only if it did not work.
pub fn reset(ports: &mut impl Ports, controls: &Controls) -> &'static str {
    ports.write8(controls.reset_port, controls.reset_value);
    wait_out(ports);
    "the machine is still running after the reset write"
}

fn wait_out(ports: &mut impl Ports) {
    let deadline = ports.ticks() + STOP_WAIT;
    while ports.ticks() < deadline {
        ports.pause();
    }
}
