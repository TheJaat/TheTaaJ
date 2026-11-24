//! usb - the USB driver: an xHCI host controller, and the keyboards and
//! mice plugged into it.
//!
//! What the driver does with the controller is in the `usb` crate,
//! tested there against a simulated controller and devices; this program
//! gives it the controller's real registers, its real DMA memory and a
//! way to wait, and passes on what the devices say:
//!
//!   - a keyboard's reports become key presses and releases
//!     (`input::usb::BootKeyboard`), then the same set-1 scancodes the
//!     PS/2 keyboard sends, through the same decoder - so the layout,
//!     Shift and Ctrl are the PS/2 keyboard's - and the terminal bytes go
//!     to the keys' channel, as kbd's and hid's do. A held key repeats
//!     here (a USB keyboard does not repeat by itself);
//!   - a mouse's reports go to the display as pointer reports;
//!   - a USB stick (the first one) is a disk: the disk protocol's
//!     requests (`taaj::block_service`, as blk, ahci and nvme serve
//!     them) become SCSI READs and WRITEs (`usb::storage`). When init
//!     has no other disk, it gives this program the disk's channels, and
//!     fs reads TaajFS from the stick. With no stick in, every request
//!     is answered "nothing in the drive".
//!
//! Devices may come and go: a port change is an event, and a device
//! plugged in later is enumerated then; one pulled out loses its slot.
//!
//! Startup handles, from init:
//!   0  the controller's registers: the memory of its BAR 0, uncached
//!   1  its DMA memory: physically contiguous, its address known
//!   2  the device channel: the PciDeviceInfo comes first
//!   3  the keys' channel (send only)
//!   4  the display's channel: pointer reports (send only)
//!   5  pings from init (abi::PING)
//!   6  the controller's interrupt (MSI or MSI-X), if it has one - else
//!      polled
//!
//! The description on 2 comes with two handles when the stick is the
//! system's disk: the disk's request channels, the one that may write
//! and the read-only one (receive only).

#![no_std]
#![no_main]

use abi::display::Pointer;
use abi::PciDeviceInfo;
use input::pointer::Report;
use input::usb::{boot_mouse, BootKeyboard, Repeat};
use input::{evdev, terminal_bytes, Decoder};
use taaj::block_service::{self, Served};
use taaj::{println, Error, Handle};
use usb::descriptor;
use usb::device::{self, Attached, Kind};
use usb::storage::Storage;
use usb::xhci::{self, layout, Bus, Controller, Dma, Event};

taaj::program!(main);

/// The controller's registers, mapped here, and its interrupt.
struct Registers {
    base: *mut u8,
    size: usize,
    interrupt: Option<Handle>,
}

impl Bus for Registers {
    fn read(&mut self, register: usize) -> u32 {
        assert!(register + 4 <= self.size, "register {register:#x} is past the controller's");
        // SAFETY: inside the mapping (checked above), a 32-bit register
        // at a multiple of four (every offset in the crate is).
        // Volatile: each read is a question to the controller.
        unsafe { self.base.add(register).cast::<u32>().read_volatile() }
    }

    fn write(&mut self, register: usize, value: u32) {
        assert!(register + 4 <= self.size, "register {register:#x} is past the controller's");
        // SAFETY: as for `read`.
        unsafe { self.base.add(register).cast::<u32>().write_volatile(value) }
    }

    /// With an interrupt: until it fires, or the next tick. The kernel
    /// counts interrupts, so one that fired before the wait is not lost.
    /// Polled: give the CPU away.
    fn pause(&mut self) {
        match self.interrupt {
            Some(line) => {
                if let Ok(Some(_)) = taaj::object_wait_until(&[line], taaj::ticks() + 1) {
                    let _ = taaj::interrupt_wait(line); // pending: returns at once
                }
            }
            None => taaj::yield_now(),
        }
    }

    fn ticks(&mut self) -> u64 {
        taaj::ticks()
    }

    fn size(&self) -> usize {
        self.size
    }
}

/// A device in use, and what its keyboard (if it has one) is doing.
struct InUse {
    attached: Attached,
    keyboard: BootKeyboard,
    decoder: Decoder,
    repeat: Repeat,
    /// Failed transfers in a row: a device that only fails is let go.
    errors: u32,
}

struct Driver {
    controller: Controller<Registers>,
    devices: [Option<InUse>; xhci::SLOTS as usize],
    keys_out: Handle,
    pointer_out: Handle,
    /// The slot of the stick that is the disk.
    disk: Option<u8>,
    /// Ports whose device was looked at and is not driven (a hub, a
    /// pointer without the boot protocol, ...), one bit each: left
    /// alone until it is unplugged. Enumerating a device resets its
    /// port, and the reset raises another port event; without this,
    /// each such device was enumerated again on every event (seen
    /// in VMware: three times each).
    not_driven: [u64; 4],
}

impl Driver {
    fn is_not_driven(&self, port: u8) -> bool {
        self.not_driven[usize::from(port / 64)] & (1 << (port % 64)) != 0
    }

    fn set_not_driven(&mut self, port: u8, not_driven: bool) {
        let (word, bit) = (usize::from(port / 64), 1u64 << (port % 64));
        if not_driven {
            self.not_driven[word] |= bit;
        } else {
            self.not_driven[word] &= !bit;
        }
    }
}

/// A name as INQUIRY gives it: ASCII, padded with spaces.
fn trimmed(name: &[u8]) -> &str {
    let name = core::str::from_utf8(name).unwrap_or("?");
    name.trim_end_matches([' ', '\0'])
}

/// Classes worth a word in the log, for devices not driven.
fn class_name(class: u8) -> &'static str {
    match class {
        descriptor::CLASS_HID => "an input device without the boot protocol",
        descriptor::CLASS_STORAGE => "mass storage",
        descriptor::CLASS_HUB => "a hub",
        0x01 => "audio",
        0x02 | 0x0A => "a modem or network adapter",
        0x0E => "a camera",
        0xE0 => "a wireless adapter",
        _ => "a device of a class not driven here",
    }
}

impl Driver {
    /// Enumerates the device on `port`, and says what it is.
    fn attach(&mut self, port: u8) {
        if self.is_not_driven(port) || self.devices.iter().flatten().any(|d| d.attached.port == port) {
            return;
        }
        let attached = match device::attach(&mut self.controller, port, self.disk.is_none()) {
            Ok(attached) => attached,
            Err(why) => {
                println!("[usb] port {port}: {why}");
                return;
            }
        };
        let (vendor, product, speed) = (attached.vendor, attached.product, attached.speed.name());
        if let Some(storage) = attached.storage {
            println!(
                "[usb] port {port}: disk \"{} {}\" ({vendor:04x}:{product:04x}, {speed}), {} sectors ({} MiB){} ready",
                trimmed(&storage.vendor),
                trimmed(&storage.product),
                storage.sectors,
                storage.sectors / 2048,
                if storage.read_only { ", read-only" } else { "" }
            );
            self.disk = Some(attached.slot);
        } else if attached.functions.iter().all(Option::is_none) {
            match attached.storage_problem {
                Some(why) => println!("[usb] port {port}: {vendor:04x}:{product:04x}, {speed}, mass storage - not driven: {why}"),
                None => println!(
                    "[usb] port {port}: {vendor:04x}:{product:04x}, {speed}, {} (class {:#04x}) - not driven",
                    class_name(attached.class),
                    attached.class
                ),
            }
            self.controller.disable_slot(attached.slot);
            self.set_not_driven(port, true);
            return;
        }
        for function in attached.functions.iter().flatten() {
            let what = match function.kind {
                Kind::Keyboard => "keyboard",
                Kind::Mouse => "mouse",
            };
            println!("[usb] port {port}: {what} ({vendor:04x}:{product:04x}, {speed}) ready");
        }
        let slot = usize::from(attached.slot) - 1;
        self.devices[slot] =
            Some(InUse { attached, keyboard: BootKeyboard::new(), decoder: Decoder::new(), repeat: Repeat::new(), errors: 0 });
    }

    /// The device on `port` is gone.
    fn detach(&mut self, port: u8) {
        let Some(slot) = self.devices.iter().flatten().find(|d| d.attached.port == port).map(|d| d.attached.slot) else {
            return;
        };
        self.devices[usize::from(slot) - 1] = None;
        self.controller.disable_slot(slot);
        if self.disk == Some(slot) {
            self.disk = None;
            println!("[usb] port {port}: unplugged - the disk is gone");
        } else {
            println!("[usb] port {port}: unplugged");
        }
    }

    fn event(&mut self, event: Event) {
        match event {
            Event::Port { port } => {
                let status = self.controller.port_status(port);
                self.controller.port_seen(port);
                if !status.connected {
                    self.set_not_driven(port, false);
                }
                match (status.connected, self.devices.iter().flatten().any(|d| d.attached.port == port)) {
                    (true, false) => self.attach(port),
                    (false, true) => self.detach(port),
                    _ => {}
                }
            }
            Event::Transfer { slot, endpoint, code, remaining, .. } => self.report(slot, endpoint, code, remaining),
            Event::Command { .. } | Event::Other => {}
        }
    }

    /// A report from a device's interrupt endpoint: passed on, and the
    /// next one asked for.
    fn report(&mut self, slot: u8, dci: u8, code: u8, remaining: u32) {
        let Some(in_use) = self.devices.get_mut(usize::from(slot).wrapping_sub(1)).and_then(Option::as_mut) else { return };
        let Some(function) = in_use.attached.function(dci) else { return };
        let asked = usize::from(function.packet).min(device::REPORT_MAX);
        if code == xhci::code::SUCCESS || code == xhci::code::SHORT_PACKET {
            in_use.errors = 0;
            let length = asked.saturating_sub(remaining as usize);
            let data = self.controller.endpoint_data(slot, dci, length);
            match function.kind {
                Kind::Keyboard => {
                    let mut report = [0u8; 8];
                    let n = data.len().min(8);
                    report[..n].copy_from_slice(&data[..n]);
                    let (decoder, keys_out) = (&mut in_use.decoder, self.keys_out);
                    in_use.keyboard.feed(&report[..n], |code, pressed| press(decoder, keys_out, code, pressed));
                    in_use.repeat.held(in_use.keyboard.held(), taaj::ticks());
                }
                Kind::Mouse => {
                    if let Some(Report::Relative { dx, dy, buttons }) = boot_mouse(data) {
                        let message = Pointer { absolute: false, x: dx, y: dy, buttons };
                        let _ = taaj::send(self.pointer_out, &message.to_message(), &[]);
                    }
                }
            }
        } else {
            in_use.errors += 1;
            if in_use.errors > 10 {
                let port = in_use.attached.port;
                println!("[usb] port {port}: endpoint {dci} fails every time (code {code}) - let go");
                self.devices[usize::from(slot) - 1] = None;
                self.controller.disable_slot(slot);
                return;
            }
            self.controller.recover_endpoint(slot, dci);
        }
        if self.controller.queue_in(slot, dci, asked).is_err() {
            println!("[usb] slot {slot}: endpoint {dci} cannot be asked again");
        }
    }

    /// Held keys whose time to repeat has come; and when to look next.
    fn repeat(&mut self) -> Option<u64> {
        let now = taaj::ticks();
        let mut next: Option<u64> = None;
        for in_use in self.devices.iter_mut().flatten() {
            if let Some(code) = in_use.repeat.due(now) {
                press(&mut in_use.decoder, self.keys_out, code, true);
            }
            if let Some(at) = in_use.repeat.next() {
                next = Some(next.map_or(at, |n| n.min(at)));
            }
        }
        next
    }
}

/// The stick, as the block service sees a disk.
impl block_service::Device for Driver {
    fn sectors(&self) -> u64 {
        self.storage().map_or(0, |s| s.sectors)
    }

    fn read_only(&self) -> bool {
        self.storage().is_none_or(|s| s.read_only)
    }

    fn present(&self) -> bool {
        self.storage().is_some()
    }

    fn transfer(&mut self, write: bool, sector: u64, count: u32) -> u8 {
        let Some(slot) = self.disk else { return abi::block::NO_MEDIUM };
        let Some(storage) = self.devices[usize::from(slot) - 1].as_mut().and_then(|d| d.attached.storage.as_mut()) else {
            return abi::block::NO_MEDIUM;
        };
        match storage.transfer(&mut self.controller, write, sector, count) {
            Ok(()) => abi::block::OK,
            Err(error) => {
                println!("[usb] the disk: {} sectors at {sector}: {}", if write { "writing" } else { "reading" }, error.describe());
                match error {
                    usb::storage::Error::Failed { key: 0x7, .. } => abi::block::READ_ONLY,
                    _ => abi::block::IO_ERROR,
                }
            }
        }
    }

    fn data(&mut self, length: usize) -> &mut [u8] {
        self.controller.storage_data_mut(length)
    }
}

impl Driver {
    fn storage(&self) -> Option<&Storage> {
        let slot = self.disk?;
        self.devices[usize::from(slot) - 1].as_ref()?.attached.storage.as_ref()
    }
}

/// One key going down or up: through the PS/2 decoder, out as terminal
/// bytes.
fn press(decoder: &mut Decoder, keys_out: Handle, code: u16, pressed: bool) {
    let Some((bytes, length)) = evdev::scancode(code, pressed) else { return };
    for &byte in &bytes[..length] {
        let Some(event) = decoder.feed(byte) else { continue };
        if let Some(text) = terminal_bytes(event, decoder.control_held()) {
            // A reader that lags loses keys, not the driver.
            let _ = taaj::send(keys_out, text.as_bytes(), &[]);
        }
    }
}

fn map(memory: Handle, what: &str) -> Result<*mut u8, i64> {
    taaj::memory_map(memory, true).map_err(|error| {
        println!("[usb] cannot map {what}: {error:?}");
        4
    })
}

fn start(info: &PciDeviceInfo, registers: Handle, dma: Handle, interrupt: Option<Handle>) -> Result<Controller<Registers>, i64> {
    let is_xhci = (info.class, info.subclass, info.interface) == (xhci::CLASS, xhci::SUBCLASS, xhci::INTERFACE);
    // Where the registers begin in the mapping, and whether they all fit
    // in it: asked of the kernel, not taken from the description.
    let offset = (info.registers % 4096) as usize;
    let size = info.registers_size as usize;
    let fits = taaj::memory_size(registers).is_ok_and(|mapped| (offset + size) as u64 <= mapped);
    let needed = layout::needed(xhci::SLOTS, 0);
    if !is_xhci || !info.registers_in_memory() || !fits || (info.dma_size as usize) < needed {
        println!("[usb] no usable device description");
        return Err(2);
    }
    let page = map(registers, "the controller's registers")?;
    let memory = map(dma, "the DMA memory")?;
    // SAFETY: inside the mapping (checked above).
    let base = unsafe { page.add(offset) };
    // SAFETY: the DMA memory, all of it, mapped here and used for
    // nothing else; `dma_physical` is where the kernel says it is.
    let dma = unsafe { Dma::new(memory, info.dma_physical, info.dma_size as usize) };
    let controller = Controller::start(Registers { base, size, interrupt }, dma, interrupt.is_some()).map_err(|why| {
        println!("[usb] the controller would not start: {why}");
        5
    })?;
    println!(
        "[usb] xHCI {}.{:x} controller at {:02x}:{:02x}.{}: {} ports, {} scratchpad pages, {}",
        controller.version >> 8,
        controller.version & 0xFF,
        info.bus,
        info.device,
        info.function,
        controller.ports,
        controller.scratchpads,
        match (interrupt, info.msix) {
            (None, _) => "polled",
            (Some(_), true) => "MSI-X interrupts",
            (Some(_), false) => "MSI interrupts",
        }
    );
    Ok(controller)
}

fn main() -> i64 {
    let (Some(registers), Some(dma), Some(described), Some(keys_out), Some(pointer_out)) = (
        taaj::startup_handle(0),
        taaj::startup_handle(1),
        taaj::startup_handle(2),
        taaj::startup_handle(3),
        taaj::startup_handle(4),
    ) else {
        println!("[usb] started without its controller");
        return 1;
    };
    let mut pings = taaj::startup_handle(5);
    let interrupt = taaj::startup_handle(6);

    let mut message = [0u8; abi::MESSAGE_MAX as usize];
    let mut handles = [0u64; abi::MESSAGE_HANDLES_MAX as usize];
    let (description, given) = match taaj::receive(described, &mut message, &mut handles) {
        Ok((n, count)) => (&message[..n], count),
        Err(_) => (&[][..], 0),
    };
    // The disk's two channels, while they have clients.
    let mut disk_channels = match given {
        2 => [Some(handles[0]), Some(handles[1])],
        _ => {
            for &handle in &handles[..given] {
                let _ = taaj::close(handle);
            }
            [None, None]
        }
    };
    let Some(info) = PciDeviceInfo::from_bytes(description) else {
        println!("[usb] no usable device description");
        return 2;
    };
    let controller = match start(&info, registers, dma, interrupt) {
        Ok(controller) => controller,
        Err(code) => return code,
    };
    let mut driver = Driver { controller, devices: [const { None }; xhci::SLOTS as usize], keys_out, pointer_out, disk: None, not_driven: [0; 4] };

    // What is plugged in already. A controller may say so with a port
    // event for each once it runs, or may not (QEMU's drops the events
    // of a reset, which happen while it is stopped): so the ports are
    // looked at once, here. A device found both ways is attached once.
    for port in 1..=driver.controller.ports {
        if driver.controller.port_status(port).connected {
            driver.controller.port_seen(port);
            driver.attach(port);
        }
    }
    loop {
        driver.controller.interrupt_seen();
        while let Some(event) = driver.controller.next_event() {
            driver.event(event);
        }
        let repeat_at = driver.repeat();

        // The controller, init's pings, or the disk's requests; a
        // polled controller every tick, a held key when it repeats.
        let mut waiting = [0u64; 4];
        let mut length = 0;
        if let Some(line) = interrupt {
            waiting[length] = line;
            length += 1;
        }
        let ping_at = length;
        if let Some(channel) = pings {
            waiting[length] = channel;
            length += 1;
        }
        let disk_at = length;
        let mut disk_which = [0usize; 2];
        for (n, channel) in disk_channels.iter().enumerate() {
            if let Some(channel) = *channel {
                disk_which[length - disk_at] = n;
                waiting[length] = channel;
                length += 1;
            }
        }
        let until = match (interrupt, repeat_at) {
            (None, _) => Some(taaj::ticks() + 1),
            (Some(_), at) => at,
        };
        let ready = match (length, until) {
            (0, at) => {
                taaj::sleep(at.map_or(1, |at| at.saturating_sub(taaj::ticks()).max(1)));
                continue;
            }
            (_, Some(at)) => taaj::object_wait_until(&waiting[..length], at),
            (_, None) => taaj::object_wait(&waiting[..length]).map(Some),
        };
        match ready {
            Ok(Some(n)) if n >= disk_at => {
                let which = disk_which[n - disk_at];
                let Some(channel) = disk_channels[which] else { continue };
                // 6 may write; 7 only reads.
                match block_service::serve_one(&mut driver, "usb", channel, which == 0) {
                    Served::Answered => {}
                    Served::Closed => {
                        let _ = taaj::close(channel);
                        disk_channels[which] = None;
                    }
                    Served::Failed => return 6,
                }
            }
            Ok(Some(n)) if n == ping_at && pings.is_some() => {
                if let Some(channel) = pings {
                    if !taaj::answer_pings(channel) {
                        pings = None;
                    }
                }
            }
            Ok(Some(_)) => {
                // The interrupt: taken, so the next can come. The
                // controller is told at the top.
                if let Some(line) = interrupt {
                    if taaj::interrupt_wait(line).is_err() {
                        return 3;
                    }
                }
            }
            Ok(None) => {}
            Err(Error::PeerClosed) => return 0,
            Err(_) => return 3,
        }
    }
}
