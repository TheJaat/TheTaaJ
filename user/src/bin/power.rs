//! power - switches the machine off, and resets it, when init asks.
//!
//! The kernel finds how this machine is switched off and hands exactly
//! that to init (kernel64/src/power.rs); init gives it to this program,
//! and keeps the only channel that reaches it. So nothing but init can
//! switch the machine off - and init does only after it has stopped the
//! other services, so that the disk is left whole.
//!
//! On a PC: ACPI. The DSDT, read-only, from which `\_S5` (the sleep
//! type that means "off") is read at start - so that a DSDT this program
//! cannot read is reported at boot, not when someone wants to switch
//! off - and the I/O ports the FADT names. What to write to them is in
//! the `acpi` crate, tested on the host against a simulated chipset.
//!
//! On AArch64: a power handle, and the POWER system call - the kernel
//! makes PSCI's SYSTEM_OFF or SYSTEM_RESET call, which only it can.
//!
//! Startup handles, from init:
//!   1  the request channel: one byte, abi::power::OFF or RESET. An
//!      answer comes back only if it did not work: why, as text.
//!   2  the description channel: init sends the PowerInfo (abi) with
//!      its handles, a fresh copy for each start of this program

#![no_std]
#![no_main]

use abi::{power, PowerInfo};
use taaj::{println, Handle};

taaj::program!(main);

/// What this machine is switched off with.
enum Way {
    /// ACPI: the ports, and the sleep type for S5 - or why there is
    /// none (resetting still works).
    Acpi(acpi::Controls, Result<(u8, u8), &'static str>),
    Psci(Handle),
}

/// The ports, through the instructions that reach them (`in`, `out`):
/// the kernel lets this process use exactly the ones it was given.
#[cfg(target_arch = "x86_64")]
struct Ports;

#[cfg(target_arch = "x86_64")]
impl acpi::Ports for Ports {
    fn read16(&mut self, port: u16) -> u16 {
        // SAFETY: one of the ports enabled at start (an access to any
        // other ends this process); reading PM1 control changes nothing.
        unsafe { taaj::port::in16(port) }
    }

    fn write16(&mut self, port: u16, value: u16) {
        // SAFETY: as for read16; the value is the acpi crate's.
        unsafe { taaj::port::out16(port, value) }
    }

    fn write8(&mut self, port: u16, value: u8) {
        // SAFETY: as for write16.
        unsafe { taaj::port::out8(port, value) }
    }

    fn pause(&mut self) {
        taaj::sleep(1);
    }

    fn ticks(&mut self) -> u64 {
        taaj::ticks()
    }
}

/// Reads the description and its handles, and gets ready.
fn start(described: Handle) -> Result<Way, &'static str> {
    let mut message = [0u8; 64];
    let mut handles = [0u64; abi::MESSAGE_HANDLES_MAX as usize];
    let (n, count) = taaj::receive(described, &mut message, &mut handles).map_err(|_| "no description from init")?;
    let info = PowerInfo::from_bytes(&message[..n]).ok_or("a description that is not a PowerInfo")?;
    if count != info.handles() {
        return Err("a description without its handles");
    }
    match info.kind {
        power::PSCI => Ok(Way::Psci(handles[0])),
        power::ACPI => acpi_start(&info, &handles[..count]),
        _ => Err("a kind of power control this program does not know"),
    }
}

fn acpi_start(info: &PowerInfo, handles: &[Handle]) -> Result<Way, &'static str> {
    // The ports: every handle after the DSDT's.
    for &ports in &handles[1..] {
        taaj::ioports_enable(ports).map_err(|_| "the kernel would not enable the ports")?;
    }
    let controls = acpi::Controls {
        pm1a_control: info.pm1a_control,
        pm1b_control: info.pm1b_control,
        smi_command: info.smi_command,
        acpi_enable: info.acpi_enable,
        reset_port: info.reset_port,
        reset_value: info.reset_value,
    };
    // The DSDT, mapped read-only: the handle allows nothing else.
    let offset = (info.dsdt % 4096) as usize;
    let length = info.dsdt_length as usize;
    let fits = taaj::memory_size(handles[0]).is_ok_and(|size| (offset + length) as u64 <= size);
    let types = match (fits, taaj::memory_map(handles[0], false)) {
        (true, Ok(page)) => {
            // SAFETY: inside the mapping (checked above), which nothing
            // writes: the firmware's table, read-only.
            let table = unsafe { core::slice::from_raw_parts(page.add(offset), length) };
            acpi::sleep_type(table, 5)
        }
        _ => Err("the DSDT could not be read"),
    };
    Ok(Way::Acpi(controls, types))
}

/// What a program prints is queued, and written to the serial line
/// when a CPU is idle (the kernel's console.rs). A machine switched off
/// right after a `println!` would take the line with it: wait this long
/// first (0.2 s), so the log says what happened last.
const SAID: u64 = 20;

/// Does what was asked; returns only if it did not work - why.
fn act(way: &Way, request: u8) -> &'static str {
    match (way, request) {
        #[cfg(target_arch = "x86_64")]
        (Way::Acpi(controls, Ok(types)), power::OFF) => {
            println!("[power] switching off: sleep type {}/{} into PM1 control (ACPI S5)", types.0, types.1);
            taaj::sleep(SAID);
            acpi::switch_off(&mut Ports, controls, *types)
        }
        (Way::Acpi(_, Err(why)), power::OFF) => why,
        #[cfg(target_arch = "x86_64")]
        (Way::Acpi(controls, _), power::RESET) => {
            println!("[power] resetting: {:#x} into port {:#x}", controls.reset_value, controls.reset_port);
            taaj::sleep(SAID);
            acpi::reset(&mut Ports, controls)
        }
        (Way::Psci(handle), power::OFF | power::RESET) => {
            println!("[power] {} (PSCI)", if request == power::OFF { "switching off" } else { "resetting" });
            taaj::sleep(SAID);
            match taaj::power(*handle, request) {
                Ok(()) | Err(_) => "the firmware refused",
            }
        }
        _ => "an unknown request",
    }
}

fn main() -> i64 {
    let (Some(requests), Some(described)) = (taaj::startup_handle(0), taaj::startup_handle(1)) else {
        println!("[power] started without its handles");
        return 1;
    };
    let way = match start(described) {
        Ok(way) => way,
        Err(why) => {
            println!("[power] {why} - cannot switch off or reset");
            return 2;
        }
    };
    match &way {
        Way::Acpi(controls, types) => {
            let off = match types {
                Ok((a, b)) => {
                    let mut text = taaj::Buffer::<48>::new();
                    let _ = core::fmt::Write::write_fmt(&mut text, format_args!("sleep type {a}/{b} (\\_S5)"));
                    text
                }
                Err(why) => {
                    let mut text = taaj::Buffer::<48>::new();
                    let _ = core::fmt::Write::write_str(&mut text, "cannot: ");
                    let _ = core::fmt::Write::write_str(&mut text, why);
                    text
                }
            };
            println!(
                "[power] ACPI: off is {}; reset is {:#x} into port {:#x}; ready",
                core::str::from_utf8(off.as_bytes()).unwrap_or("?"),
                controls.reset_value,
                controls.reset_port
            );
        }
        Way::Psci(_) => println!("[power] PSCI: SYSTEM_OFF and SYSTEM_RESET; ready"),
    }
    let mut message = [0u8; 8];
    loop {
        let Ok((n, _)) = taaj::receive(requests, &mut message, &mut []) else {
            return 0; // init has gone: nobody can ask any more
        };
        let why = act(&way, if n == 1 { message[0] } else { 0 });
        println!("[power] could not: {why}");
        let _ = taaj::send(requests, why.as_bytes(), &[]);
    }
}
