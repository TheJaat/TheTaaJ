//! init - the first process, and the supervisor of all the others.
//!
//! The kernel starts only this, with six startup handles on a PC:
//!   1  the launcher: the right to start programs
//!   2  the keyboard's interrupt line
//!   3  I/O port 0x60
//!   4  I/O port 0x64
//!   5  a channel with a message per device the kernel found - the
//!      framebuffer (its description and memory object), and a virtio
//!      disk (its description, ports, DMA memory and interrupt)
//!   6  the system handle: the right to look at memory and processes
//! (all with every right but the launcher's, so init can share them out.)
//!
//! and five on an AArch64 machine:
//!   1  the launcher
//!   2  the serial port's interrupt line
//!   3  the memory its registers are in
//!   4  the channel of devices - a screen if QEMU was given one (ramfb,
//!      described as on the PC), a disk, network cards and input
//!      devices (each: its description, registers, DMA memory and
//!      interrupt)
//!   5  the system handle
//!
//! The services: registry (where the others are found), display (the
//! window server), fs (files - on the disk, if there is one), blk,
//! ahci or nvme (the disk driver: for a virtio disk, a SATA one or an
//! NVMe one), nic or e1000 (the network card's driver: a virtio card or
//! an Intel one) and net (the network, for programs - both only if
//! there is a card), shell, kbd (the PC's
//! PS/2 keyboard and mouse), hid (virtio keyboards, mice and
//! tablets, if there are any) and usb (an xHCI controller, and the USB
//! keyboards and mice on it, if there is one).
//!
//! The terminal is a window on the screen when the machine has a screen
//! and a keyboard. A PC always does; an AArch64 machine does when QEMU
//! was given ramfb and a virtio input device (init cannot tell a
//! keyboard from a mouse: only hid asks the device). Then the drivers'
//! keys and pointer reports go to display, which passes the keys on to
//! the window with the focus - to the shell, on a channel of its own,
//! while that is the terminal - and programs open windows on its
//! "windows" channel. Otherwise the terminal is the serial port, and
//! one driver, serial, is both keyboard and screen: it is given the
//! keys' channel to send on and the display's to read, there is no
//! window server, and nothing else in the system knows the difference.
//!
//! What a service works *on* - an interrupt line, ports, the
//! framebuffer's memory, the channel its requests arrive on - init
//! hands it at its start, in a fixed order: those are its own, and
//! nobody else's. What a service *uses of another service* it asks the
//! registry for, by name. init tells the registry what answers to each
//! name, and gives each client a channel to the registry on which only
//! the names meant for that client exist (the `registry` crate): the
//! shell can ask for the screen, the files, the keyboard; fs can ask
//! for the disk, and for nothing else.
//!
//! init also runs programs for the shell. The shell has no launcher; it
//! sends a request (abi::init) on a channel to init, with the handles
//! the program is to have - the shell's own, handed down. init starts
//! the program with exactly those, and replies "ok" with a handle to
//! the new process, or an error.
//!
//! init builds the system from these: it wires the services together
//! with channels and starts each one with only the handles it needs, and
//! fewer rights than init holds - kbd gets the interrupt as wait-only,
//! the ports without the right to pass them on.
//!
//! Then it watches. When a service ends - crashes, most likely - init
//! starts it again with fresh copies of the same handles. It keeps the
//! originals of everything, including both ends of the keyboard
//! channel, so a dying kbd does not close the shell's channel: the shell
//! never notices the driver was replaced. This is the point of drivers
//! in user mode: a driver bug costs a restart, not the system.
//!
//! A service can also fail without ending: stuck in a loop, or waiting
//! for something that never comes. Its process is alive, so nothing
//! above would notice. For the services that answer requests - fs and
//! blk - init keeps a watchdog: once a second it sends each a small
//! request of the kind any client could send, and expects an answer
//! within the service's patience. No answer, and init ends the process
//! (kill) - after which it is a service that ended, restarted like any
//! other, taking over the same queue with every request still in it.
//! init never *waits* for those answers: they are one more thing in the
//! set it watches, with a time limit. It must not hang because a
//! service did.
//!
//! A service that keeps crashing is not restarted forever. More than
//! MAX_RESTARTS within RESTART_WINDOW ticks and init gives up on it -
//! restarting in a tight loop would only hide the bug and burn the CPU.

#![no_std]
#![no_main]

extern crate alloc;

use abi::{FramebufferInfo, MmioDeviceInfo, PciDeviceInfo, PowerInfo};
use registry::protocol::Admin;
use taaj::rights::{DUPLICATE, READ, RECEIVE, SEND, TRANSFER, WRITE};
use core::fmt::Write;

use taaj::{println, Buffer, Error, Handle};

taaj::program!(main);

/// What the shell may ask init to run: programs, not services.
const RUNNABLE: &[&str] = &["hello", "peek"];

/// Whether this machine's keyboard, if it has a screen, is the PS/2
/// one the kernel hands init the ports of (the PC's), or a virtio one
/// that arrives as a device (QEMU's AArch64 machine).
const PS2_KEYBOARD: bool = cfg!(target_arch = "x86_64");

/// The most input devices passed on to `hid`.
const INPUTS_MAX: usize = 4;

/// A device the kernel found (a disk, a network card), as it described
/// it: on a PCI bus, or at a place in memory. Each kind's driver is the
/// same program for both.
#[derive(Clone, Copy)]
enum Device {
    Pci(PciDeviceInfo),
    Mmio(MmioDeviceInfo),
}

/// On a grant: the service may copy the handle and pass copies on - to
/// the programs it has init start. Without these two rights a handle
/// stays where init put it.
const DELEGABLE: u32 = DUPLICATE | TRANSFER;

const MAX_RESTARTS: usize = 5;
const RESTART_WINDOW: u64 = 30 * 100; // 30 seconds of 100 Hz ticks
/// How often a watched service is asked whether it is there, in ticks.
const PING_EVERY: u64 = 100;
/// And how soon a ping that could not be sent is tried again.
const RETRY_EVERY: u64 = 10;

/// The watchdog's view of one service.
struct Watch {
    /// Where its requests go - init's own end of that channel.
    requests: Handle,
    /// A request that changes nothing and is answered at once.
    ping: [u8; 16],
    ping_length: usize,
    /// One channel for all its answers, kept for good: each ping carries
    /// a copy of the sending end. (A fresh channel per ping, as other
    /// clients make, would do - but the number of channels in the system
    /// would flicker, and the tests count them.)
    answers_tx: Handle,
    answers_rx: Handle,
    /// Ticks an answer may take. Longer for a service that itself waits
    /// on another: fs is silent while blk is being replaced.
    patience: u64,
    /// When to ask next.
    next_ping: u64,
    /// While a ping is unanswered: when to stop believing.
    deadline: Option<u64>,
    /// Whether that ping could be sent, or is still to be tried.
    sent: bool,
}

impl Watch {
    fn new(requests: Handle, ping: &[u8], patience: u64) -> Option<Watch> {
        let (answers_tx, answers_rx) = taaj::channel_create().ok()?;
        let mut bytes = [0u8; 16];
        bytes[..ping.len()].copy_from_slice(ping);
        Some(Watch { requests, ping: bytes, ping_length: ping.len(), answers_tx, answers_rx, patience, next_ping: 0, deadline: None, sent: false })
    }

    /// Sends a ping, if one is due. The time limit starts with the first
    /// attempt, sent or not: a ping that cannot be sent - the queue is
    /// full - is tried again each time round, and a service that leaves
    /// its queue full for all of its patience is not answering either.
    fn ask(&mut self, now: u64) {
        if now < self.next_ping || (self.deadline.is_some() && self.sent) {
            return;
        }
        self.sent = match taaj::duplicate(self.answers_tx, SEND | TRANSFER) {
            Ok(copy) => taaj::send(self.requests, &self.ping[..self.ping_length], &[copy])
                .inspect_err(|_| {
                    let _ = taaj::close(copy); // not sent: still ours
                })
                .is_ok(),
            Err(_) => false,
        };
        if self.deadline.is_none() {
            self.deadline = Some(now + self.patience);
        }
    }

    /// Takes every answer that has come. Any answer means "alive" - also
    /// a late one to an earlier ping.
    fn answered(&mut self, now: u64) {
        // Room for the longest message there is: an answer that did not
        // fit could not be taken out of the queue, would be "ready" for
        // ever, and init would look at nothing else again.
        let mut reply = [0u8; abi::MESSAGE_MAX as usize];
        let mut any = false;
        while let Ok(Some(_)) = taaj::poll(&[self.answers_rx]) {
            if taaj::receive(self.answers_rx, &mut reply, &mut []).is_err() {
                break;
            }
            any = true;
        }
        if any {
            self.deadline = None;
            self.next_ping = now + PING_EVERY;
        }
    }

    /// A new process serves this now: give it time to start, and forget
    /// what the old one owed.
    fn restarted(&mut self, now: u64) {
        self.answered(now);
        self.deadline = None;
        self.next_ping = now + PING_EVERY;
    }

    /// The next tick at which there is something to do.
    fn wake_at(&self, now: u64) -> u64 {
        match self.deadline {
            Some(deadline) if self.sent => deadline,
            Some(deadline) => deadline.min(now + RETRY_EVERY),
            None => self.next_ping,
        }
    }
}

/// A startup handle a service gets: one of init's own, copied, and the
/// rights the service's copy carries.
struct Grant {
    handle: Handle,
    rights: u32,
}

struct Service {
    name: &'static str,
    grants: &'static [usize], // indexes into the handle list below
    process: Handle,
    /// Ticks of recent restarts, oldest first.
    restarts: [u64; MAX_RESTARTS],
    restart_count: usize,
    given_up: bool,
    /// For a service that answers requests: is it still answering?
    watch: Option<Watch>,
}

/// Starts `name` with a fresh duplicate of each grant. A duplicate is
/// made with TRANSFER (needed to hand it over) and then handed over with
/// only the rights in the grant - so the service cannot pass it on.
/// The services on the way from a key to the screen: the keyboard
/// drivers (PS/2, the serial line, USB and its HID devices), the display
/// and the shell. They run before busy programs, so that typing stays
/// quick however many are running (MaK-WSL/sched's K20, journal entry
/// 124). It is init's decision, not the kernel's: the kernel only keeps
/// the priorities it is given. Everything else, and everything the shell
/// and posix_spawn start, runs at the default.
const INTERACTIVE: &[&str] = &["kbd", "serial", "usb", "hid", "display", "shell"];
const INTERACTIVE_PRIORITY: i64 = -5;

/// Gives a service its priority, if it is one of INTERACTIVE. init may:
/// its ceiling is the highest, and it started the service (KILL).
fn prioritize(name: &str, process: Handle) {
    if !INTERACTIVE.contains(&name) {
        return;
    }
    let level = abi::sched::level(INTERACTIVE_PRIORITY);
    if let Err(error) = taaj::sched_set(abi::sched::PROCESS, process, abi::sched::PRIORITY, level) {
        println!("[init] could not give {name} priority {INTERACTIVE_PRIORITY}: {error:?}");
    }
}

fn start(launcher: Handle, name: &str, grants: &[&Grant], argument: u64) -> Result<Handle, Error> {
    let mut copies = [(0u64, 0u32); 8];
    for (n, grant) in grants.iter().enumerate() {
        match taaj::duplicate(grant.handle, grant.rights | TRANSFER) {
            Ok(copy) => copies[n] = (copy, grant.rights),
            Err(error) => {
                for &(copy, _) in &copies[..n] {
                    let _ = taaj::close(copy);
                }
                return Err(error);
            }
        }
    }
    let copies = &copies[..grants.len()];
    taaj::spawn(launcher, name, copies, argument).inspect_err(|_| {
        // Not started: the copies are still ours.
        for &(copy, _) in copies {
            let _ = taaj::close(copy);
        }
    })
}

fn main() -> i64 {
    // The input device's two resources after its interrupt: a PS/2
    // keyboard's two ports, or a serial port's registers (and nothing).
    #[cfg(target_arch = "x86_64")]
    let given = (
        taaj::startup_handle(0),
        taaj::startup_handle(1),
        taaj::startup_handle(2),
        taaj::startup_handle(3),
        taaj::startup_handle(4),
        taaj::startup_handle(5),
    );
    #[cfg(not(target_arch = "x86_64"))]
    let given = (
        taaj::startup_handle(0),
        taaj::startup_handle(1),
        taaj::startup_handle(2),
        Some(0),
        taaj::startup_handle(3),
        taaj::startup_handle(4),
    );
    let (Some(launcher), Some(interrupt), Some(input_first), Some(input_second), Some(devices), Some(system)) = given else {
        println!("[init] started without the kernel's handles");
        return 1;
    };
    let Ok((keys_out, keys_in)) = taaj::channel_create() else { return 2 };

    // What the kernel found: a message per device, then the channel
    // closes. The screen - its description and memory - and a disk: its
    // description and its ports, DMA memory and interrupt.
    let mut framebuffer = None;
    let mut fb_memory = 0;
    let mut disk: Option<(Device, [Handle; 3], usize)> = None;
    let mut card: Option<(Device, [Handle; 3], usize)> = None;
    // A USB (xHCI) controller: its description, registers, DMA memory
    // and interrupt - the first one only.
    let mut usb_controller: Option<(PciDeviceInfo, [Handle; 3], usize)> = None;
    // Keyboards, mice, tablets - virtio ones: each with its handles,
    // kept here and copied to each `hid` that is started.
    let mut inputs: [Option<(MmioDeviceInfo, [Handle; 3], usize)>; INPUTS_MAX] = [None; INPUTS_MAX];
    // How the machine is switched off: its description and handles
    // (abi::power), for `power`.
    let mut power: Option<(PowerInfo, [Handle; 5], usize)> = None;
    loop {
        let mut message = [0u8; 64];
        let mut handles = [0u64; abi::MESSAGE_HANDLES_MAX as usize];
        let Ok((n, count)) = taaj::receive(devices, &mut message, &mut handles) else { break };
        let message = &message[..n];
        let mut used = false;
        if let (Some(rest), 1) = (message.strip_prefix(b"framebuffer"), count) {
            framebuffer = FramebufferInfo::from_bytes(rest);
            fb_memory = handles[0];
            used = true;
        } else if let Some(info) = message.strip_prefix(b"pci").and_then(PciDeviceInfo::from_bytes) {
            let is_ahci = (info.class, info.subclass, info.interface) == (ahci::CLASS, ahci::SUBCLASS, ahci::INTERFACE);
            let is_nvme = (info.class, info.subclass, info.interface) == (nvme::CLASS, nvme::SUBCLASS, nvme::INTERFACE);
            let is_virtio = info.vendor_id == virtio::PCI_VENDOR && !info.registers_in_memory();
            let is_xhci = (info.class, info.subclass, info.interface) == (usb::xhci::CLASS, usb::xhci::SUBCLASS, usb::xhci::INTERFACE);
            if is_virtio && info.device_id == virtio::PCI_DEVICE_BLOCK && disk.is_none() && (2..=3).contains(&count) {
                disk = Some((Device::Pci(info), [handles[0], handles[1], handles[2]], count));
                used = true;
            } else if (is_ahci || is_nvme) && info.registers_in_memory() && disk.is_none() && (2..=3).contains(&count) {
                // The first disk wins: a virtio one, a SATA controller
                // or an NVMe one, whichever the bus has first.
                disk = Some((Device::Pci(info), [handles[0], handles[1], handles[2]], count));
                used = true;
            } else if ((is_virtio && info.device_id == virtio::PCI_DEVICE_NETWORK)
                || (e1000::model(info.vendor_id, info.device_id).is_some() && info.registers_in_memory()))
                && card.is_none()
                && (2..=3).contains(&count)
            {
                // The first card wins: a virtio one or an Intel one.
                card = Some((Device::Pci(info), [handles[0], handles[1], handles[2]], count));
                used = true;
            } else if is_xhci && info.registers_in_memory() && usb_controller.is_none() && (2..=3).contains(&count) {
                usb_controller = Some((info, [handles[0], handles[1], handles[2]], count));
                used = true;
            } else {
                println!("[init] no driver for PCI device {:04x}:{:04x}", info.vendor_id, info.device_id);
            }
        } else if let Some(info) = message.strip_prefix(b"power").and_then(PowerInfo::from_bytes) {
            if power.is_none() && count == info.handles() && count <= 5 {
                let mut kept = [0; 5];
                kept[..count].copy_from_slice(&handles[..count]);
                power = Some((info, kept, count));
                used = true;
            }
        } else if let Some(info) = message.strip_prefix(b"mmio").and_then(MmioDeviceInfo::from_bytes) {
            if info.device == virtio::mmio::DEVICE_BLOCK && info.version == 1 && disk.is_none() && (2..=3).contains(&count) {
                disk = Some((Device::Mmio(info), [handles[0], handles[1], handles[2]], count));
                used = true;
            } else if info.device == virtio::mmio::DEVICE_NETWORK && info.version == 1 && card.is_none() && (2..=3).contains(&count) {
                card = Some((Device::Mmio(info), [handles[0], handles[1], handles[2]], count));
                used = true;
            } else if let (virtio::mmio::DEVICE_INPUT, 1, 2..=3, Some(slot)) =
                (info.device, info.version, count, inputs.iter_mut().find(|slot| slot.is_none()))
            {
                *slot = Some((info, [handles[0], handles[1], handles[2]], count));
                used = true;
            } else {
                println!("[init] no driver for the device at {:#x} (virtio kind {})", info.registers, info.device);
            }
        }
        if !used {
            for &handle in &handles[..count] {
                let _ = taaj::close(handle);
            }
        }
    }
    let _ = taaj::close(devices);
    // The terminal: a serial port - keyboard and screen in one, driven
    // by `serial` - unless there is a screen and a keyboard for it: the
    // PC's PS/2 keyboard, or virtio input devices.
    let has_inputs = inputs.iter().any(Option::is_some);
    let serial_terminal = framebuffer.is_none() || !(PS2_KEYBOARD || has_inputs);
    // Ports are enabled by whoever holds them, whatever the rights; a
    // memory object must be mapped, to read and to write.
    let input_rights = if PS2_KEYBOARD { 0 } else { READ | WRITE };
    // The input devices' driver: their descriptions and handles, a
    // message each, before it starts; and its pings.
    let Ok((hid_devices_tx, hid_devices_rx)) = taaj::channel_create() else { return 2 };
    let Ok((hid_ping_tx, hid_ping_rx)) = taaj::channel_create() else { return 2 };
    let Ok((display_tx, display_rx)) = taaj::channel_create() else { return 2 };
    // With a screen, the keyboard drivers' keys go to the window server,
    // which passes them on to the window with the focus - to the shell
    // on this channel while that is the terminal. And programs open
    // windows on another ("windows"); with no window server, its far end
    // is closed, so they are told at once instead of waiting.
    let Ok((shell_keys_out, shell_keys_in)) = taaj::channel_create() else { return 2 };
    let Ok((windows_out, windows_in)) = taaj::channel_create() else { return 2 };
    if serial_terminal {
        let _ = taaj::close(windows_in);
    }
    // The display and the keyboard driver answer no requests, so each
    // gets a channel only for the watchdog's pings (abi::PING).
    let Ok((display_ping_tx, display_ping_rx)) = taaj::channel_create() else { return 2 };
    let Ok((kbd_ping_tx, kbd_ping_rx)) = taaj::channel_create() else { return 2 };
    // Memory for the text on the screen. It is init's, lent to each
    // display in turn - so the text outlives a display that is
    // replaced. Without it (no memory to spare) a display keeps its
    // text itself, and a new one starts blank.
    let screen_text = taaj::memory_create(abi::display::KEPT_SIZE).unwrap_or(0);
    let Ok((requests, shell_requests)) = taaj::channel_create() else { return 2 };
    // The file service's requests. init keeps both ends, as it does the
    // keyboard's: a restarted fs takes over the same queue, and clients
    // keep the channel they have.
    let Ok((files_in, files_out)) = taaj::channel_create() else { return 2 };
    // And a second: requests on it may read files, not change them.
    let Ok((files_look_in, files_look_out)) = taaj::channel_create() else { return 2 };
    // /tmp's (J64): a second file service, with no disk - its files in
    // memory, gone when it ends - and the same two channels. Every
    // program's namespace mounts it at /tmp (taaj::file_service).
    let Ok((tmp_in, tmp_out)) = taaj::channel_create() else { return 2 };
    let Ok((tmp_look_in, tmp_look_out)) = taaj::channel_create() else { return 2 };
    // /dev's (J65): the devices - null, zero, full, random, urandom.
    // One channel: nothing there can be changed, so there is no
    // read-only one to give instead.
    let Ok((dev_in, dev_out)) = taaj::channel_create() else { return 2 };
    // The disk's: its description for each new driver, and requests.
    let Ok((disk_info_tx, disk_info_rx)) = taaj::channel_create() else { return 2 };
    // Two request channels: reads and writes for fs, which owns what is
    // on the disk; reads only for the shell, which may look.
    let Ok((disk_in, disk_out)) = taaj::channel_create() else { return 2 };
    let Ok((disk_look_in, disk_look_out)) = taaj::channel_create() else { return 2 };
    // No other disk, and a USB controller: usb serves the disk's
    // channels - from the first stick plugged in, if there is one.
    let usb_disk = disk.is_none() && usb_controller.is_some();
    if disk.is_none() && !usb_disk {
        // No disk: no one will ever answer, so say so at once - a
        // client's send fails with PeerClosed instead of waiting forever.
        let _ = (taaj::close(disk_in), taaj::close(disk_look_in));
    }
    let [disk_ports, disk_dma, disk_interrupt] = disk.map_or([0; 3], |(_, handles, _)| handles);
    // The network card's, likewise: its description for each new
    // driver, and one channel of requests - whole frames, for whoever
    // is given it.
    let Ok((card_info_tx, card_info_rx)) = taaj::channel_create() else { return 2 };
    let Ok((card_in, card_out)) = taaj::channel_create() else { return 2 };
    if card.is_none() {
        let _ = taaj::close(card_in);
    }
    let [card_registers, card_dma, card_interrupt] = card.map_or([0; 3], |(_, handles, _)| handles);
    // The network service: its requests, and its channel to the
    // registry (it asks for the card's driver by name, as fs asks for
    // the disk's). No card, no network: its clients are told at once.
    let Ok((net_in, net_out)) = taaj::channel_create() else { return 2 };
    let Ok((net_names_tx, net_names_rx)) = taaj::channel_create() else { return 2 };
    // power's: the only channel that can ask it anything - init's end
    // stays here, and nobody is given a copy - and its description.
    let Ok((power_asked, power_ask)) = taaj::channel_create() else { return 2 };
    let Ok((power_info_tx, power_info_rx)) = taaj::channel_create() else { return 2 };
    // usb's: the controller's description for each new driver, and the
    // watchdog's pings (it answers no requests).
    let Ok((usb_info_tx, usb_info_rx)) = taaj::channel_create() else { return 2 };
    let Ok((usb_ping_tx, usb_ping_rx)) = taaj::channel_create() else { return 2 };
    let [usb_registers, usb_dma, usb_interrupt] = usb_controller.map_or([0; 3], |(_, handles, _)| handles);
    if card.is_none() {
        let _ = taaj::close(net_in);
    }
    // The registry's: one channel for what init tells it, and one per
    // client for what that client asks. init keeps both ends of each,
    // as it does of every channel: a registry that is replaced takes
    // over the same queues, and clients keep the channel they have.
    let Ok((admin_tx, admin_rx)) = taaj::channel_create() else { return 2 };
    let Ok((shell_names_tx, shell_names_rx)) = taaj::channel_create() else { return 2 };
    let Ok((fs_names_tx, fs_names_rx)) = taaj::channel_create() else { return 2 };

    // Whether a device's registers are memory, to be mapped (a
    // virtio-mmio device's, an AHCI or NVMe controller's, an Intel
    // card's), or I/O ports.
    let disk_registers_in_memory = match disk {
        Some((Device::Mmio(_), ..)) => true,
        Some((Device::Pci(info), ..)) => info.registers_in_memory(),
        None => false,
    };
    let card_registers_in_memory = match card {
        Some((Device::Mmio(_), ..)) => true,
        Some((Device::Pci(info), ..)) => info.registers_in_memory(),
        None => false,
    };
    // Everything a service may be given, and the rights it gets.
    let grants = [
        Grant { handle: interrupt, rights: RECEIVE },    // 0
        Grant { handle: input_first, rights: input_rights },  // 1
        Grant { handle: input_second, rights: input_rights }, // 2
        Grant { handle: keys_out, rights: SEND },        // 3
        Grant { handle: keys_in, rights: RECEIVE },      // 4
        Grant { handle: fb_memory, rights: READ | WRITE },    // 5
        Grant { handle: display_rx, rights: RECEIVE },   // 6
        Grant { handle: display_tx, rights: SEND | DELEGABLE }, // 7: text to the screen
        Grant { handle: shell_requests, rights: SEND | RECEIVE }, // 8: requests to init
        Grant { handle: system, rights: 0 },             // 9: looking, not touching
        Grant { handle: files_in, rights: RECEIVE },     // 10: requests, for fs
        Grant { handle: files_out, rights: SEND | DELEGABLE }, // 11: requests, to fs
        // 12: the disk's registers - ports, or memory to be mapped (a
        // virtio-mmio disk's, or an AHCI controller's)
        Grant { handle: disk_ports, rights: if disk_registers_in_memory { READ | WRITE } else { 0 } },
        Grant { handle: disk_dma, rights: READ | WRITE }, // 13: its DMA memory
        Grant { handle: disk_info_rx, rights: RECEIVE },  // 14: its description
        Grant { handle: disk_in, rights: RECEIVE },       // 15: requests, for blk
        Grant { handle: disk_out, rights: SEND },         // 16: requests, to blk
        Grant { handle: disk_interrupt, rights: RECEIVE }, // 17: its interrupt
        Grant { handle: disk_look_in, rights: RECEIVE },  // 18: read-only requests, for blk
        Grant { handle: disk_look_out, rights: SEND },    // 19: read-only requests, to blk
        Grant { handle: files_look_in, rights: RECEIVE }, // 20: read-only requests, for fs
        Grant { handle: files_look_out, rights: SEND | DELEGABLE }, // 21: read-only requests, to fs
        Grant { handle: display_ping_rx, rights: RECEIVE }, // 22: pings, for display
        Grant { handle: kbd_ping_rx, rights: RECEIVE },     // 23: pings, for kbd
        Grant { handle: screen_text, rights: READ | WRITE }, // 24: the screen's text, kept
        Grant { handle: admin_rx, rights: RECEIVE },          // 25: registrations, for registry
        Grant { handle: shell_names_tx, rights: SEND },       // 26: lookups, from the shell
        Grant { handle: fs_names_tx, rights: SEND },          // 27: lookups, from fs
        // 28: the network card's registers - ports, or memory to be mapped
        // (a virtio-mmio card's, or an Intel card's BAR 0)
        Grant { handle: card_registers, rights: if card_registers_in_memory { READ | WRITE } else { 0 } },
        Grant { handle: card_dma, rights: READ | WRITE },     // 29: its DMA memory
        Grant { handle: card_info_rx, rights: RECEIVE },      // 30: its description
        Grant { handle: card_in, rights: RECEIVE },           // 31: requests, for nic or e1000
        Grant { handle: card_out, rights: SEND },             // 32: requests, to nic or e1000
        Grant { handle: card_interrupt, rights: RECEIVE },    // 33: its interrupt
        Grant { handle: net_in, rights: RECEIVE },            // 34: requests, for net
        Grant { handle: net_out, rights: SEND },              // 35: requests, to net
        Grant { handle: net_names_tx, rights: SEND },         // 36: lookups, from net
        Grant { handle: display_tx, rights: SEND },           // 37: pointer reports, to the display
        Grant { handle: hid_devices_rx, rights: RECEIVE },    // 38: input devices, for hid
        Grant { handle: hid_ping_rx, rights: RECEIVE },       // 39: pings, for hid
        Grant { handle: shell_keys_in, rights: RECEIVE },     // 40: keys, for the shell (from display)
        Grant { handle: shell_keys_out, rights: SEND },       // 41: keys, from display to the shell
        Grant { handle: windows_in, rights: RECEIVE },        // 42: windows to open, for display
        Grant { handle: windows_out, rights: SEND },          // 43: windows to open, from programs
        Grant { handle: power_asked, rights: RECEIVE | SEND }, // 44: requests, for power (and its answers)
        Grant { handle: power_info_rx, rights: RECEIVE },     // 45: how to switch off, for power
        Grant { handle: usb_registers, rights: READ | WRITE }, // 46: the USB controller's registers (BAR 0)
        Grant { handle: usb_dma, rights: READ | WRITE },      // 47: its DMA memory
        Grant { handle: usb_info_rx, rights: RECEIVE },       // 48: its description, for usb
        Grant { handle: usb_ping_rx, rights: RECEIVE },       // 49: pings, for usb
        Grant { handle: usb_interrupt, rights: RECEIVE },     // 50: its interrupt
        Grant { handle: tmp_in, rights: RECEIVE },            // 51: requests, for tmp
        Grant { handle: tmp_out, rights: SEND | DELEGABLE },  // 52: requests, to tmp
        Grant { handle: tmp_look_in, rights: RECEIVE },       // 53: read-only requests, for tmp
        Grant { handle: tmp_look_out, rights: SEND | DELEGABLE }, // 54: read-only requests, to tmp
        Grant { handle: dev_in, rights: RECEIVE },            // 55: requests, for dev
        Grant { handle: dev_out, rights: SEND | DELEGABLE },  // 56: requests, to dev
    ];
    // Only the shell's screen and the files (the disk's, /tmp's and
    // /dev's) can be copied and passed on.
    debug_assert!(grants.iter().enumerate().all(|(n, g)| g.rights & DELEGABLE == 0 || matches!(n, 7 | 11 | 21 | 52 | 54 | 56)));

    // The watchdog's pings: for fs, "what does this channel allow?" on
    // the read-only channel; for blk, "what disk is this?". fs gets the
    // longer patience - it waits on blk, and must outlast blk's
    // replacement.
    let fs_ping = [fileserver::protocol::ACCESS, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    let fs_watch = Watch::new(files_look_out, &fs_ping, 400);
    let blk_watch = (disk.is_some() || usb_disk).then(|| Watch::new(disk_look_out, &[abi::block::INFO], 200)).flatten();
    // Which program drives the disk: the virtio one, the SATA one or
    // the NVMe one. All speak abi::block, on the same channels; nobody
    // else knows.
    let disk_driver = match disk {
        None if usb_disk => "usb",
        Some((Device::Pci(info), _, _)) if (info.class, info.subclass) == (nvme::CLASS, nvme::SUBCLASS) => "nvme",
        Some((Device::Pci(info), _, _)) if info.registers_in_memory() => "ahci",
        _ => "blk",
    };
    // Which program drives the network card: the virtio one or the
    // Intel one. Both speak abi::nic, on the same channel.
    let card_driver = match card {
        Some((Device::Pci(info), _, _)) if e1000::model(info.vendor_id, info.device_id).is_some() => "e1000",
        _ => "nic",
    };
    // The network card's driver: "what card is this?".
    let nic_watch = card.and_then(|_| Watch::new(card_out, &[abi::nic::INFO], 200));
    // The network service: "who is this machine?". The longer patience,
    // as for fs: it waits on nic while that is being replaced.
    let net_watch = card.and_then(|_| Watch::new(net_out, &[abi::net::INFO], 400));
    // The display and the keyboard driver: a ping on a channel of its own.
    let display_watch = framebuffer.filter(|_| !serial_terminal).and_then(|_| Watch::new(display_ping_tx, &[abi::PING], 200));
    let kbd_watch = Watch::new(kbd_ping_tx, &[abi::PING], 200);
    let hid_watch = (!serial_terminal && has_inputs).then(|| Watch::new(hid_ping_tx, &[abi::PING], 200)).flatten();
    // usb is watched as a disk driver when it is one (blk_watch, on the
    // service entry below), else with pings.
    let usb_watch = usb_controller.filter(|_| !usb_disk).and_then(|_| Watch::new(usb_ping_tx, &[abi::PING], 200));
    let (blk_watch, usb_watch) = if usb_disk { (None, blk_watch) } else { (blk_watch, usb_watch) };

    // What the registry is told, each time one is started. The
    // services: a name, and which grant it means - the handle, and the
    // rights a client's copy gets. (The shell's old startup handles,
    // now asked for by name; and the disk, for fs.)
    let registered: [(&str, usize); 14] = [
        // The keys come from the drivers straight to the shell where the
        // terminal is a serial port; from the window server where it is
        // a window.
        ("keys", if serial_terminal { 4 } else { 40 }),
        ("display", 7),
        ("init", 8),
        ("system", 9),
        ("files", 11),
        ("files.look", 21),
        ("tmp", 52),
        ("tmp.look", 54),
        ("dev", 56),
        ("disk", 16),
        ("disk.look", 19),
        ("nic", 32),
        ("net", 35),
        ("windows", 43),
    ];
    // And the namespaces: whose, the receiving end of its channel, and
    // the names that exist on it. This table is the system's policy,
    // in one place: who may reach what.
    let namespaces: [(&str, Handle, &[&str]); 3] = [
        // (The shell has the network card itself only for the program
        // that tests its driver. Everything else uses "net".)
        ("shell", shell_names_rx, &["keys", "display", "init", "system", "files", "files.look", "tmp", "tmp.look", "dev", "disk.look", "nic", "net", "windows"]),
        ("fs", fs_names_rx, &["disk"]),
        ("net", net_names_rx, &["nic"]),
    ];
    // Sends one message to the registry, with a copy of `handle` that
    // carries `rights` and can be copied and passed on from there. The
    // queue may be full of what a registry that crashed never read;
    // the new one empties it as soon as it runs.
    let tell = |message: &[u8], handle: Handle, rights: u32| -> Result<(), Error> {
        let copy = taaj::duplicate(handle, rights | DELEGABLE)?;
        let started = taaj::ticks();
        loop {
            match taaj::send(admin_tx, message, &[copy]) {
                Err(Error::QueueFull) if taaj::ticks() - started < 200 => taaj::yield_now(),
                Ok(()) => return Ok(()),
                Err(error) => {
                    let _ = taaj::close(copy); // not sent: still ours
                    return Err(error);
                }
            }
        }
    };

    // The registry first: everything after it may ask it for something.
    // Then the display: it must have its info before the shell's text
    // arrives on the same channel.
    let mut services = [
        Service { name: "registry", grants: &[25], process: 0, restarts: [0; MAX_RESTARTS], restart_count: 0, given_up: false, watch: None },
        Service {
            name: "display",
            // The screen, its messages, pings, the drivers' keys, the
            // shell's keys, the windows to open - and the terminal's
            // text, kept, if there was memory for it.
            grants: if screen_text != 0 { &[5, 6, 22, 4, 41, 42, 24] } else { &[5, 6, 22, 4, 41, 42] },
            process: 0,
            restarts: [0; MAX_RESTARTS],
            restart_count: 0,
            // Where the terminal is the serial port, its driver reads
            // the display's messages: a window server would take them.
            given_up: framebuffer.is_none() || serial_terminal,
            watch: display_watch,
        },
        Service { name: "fs", grants: &[10, 20, 27], process: 0, restarts: [0; MAX_RESTARTS], restart_count: 0, given_up: false, watch: fs_watch },
        // /tmp: the fs program, given no registry and so no disk - its
        // files in memory (fs.rs). Restarted empty, as /tmp is after a
        // reboot.
        Service { name: "tmp", grants: &[51, 53], process: 0, restarts: [0; MAX_RESTARTS], restart_count: 0, given_up: false, watch: None },
        // /dev: its devices (dev.rs).
        Service { name: "dev", grants: &[55], process: 0, restarts: [0; MAX_RESTARTS], restart_count: 0, given_up: false, watch: None },
        Service {
            name: disk_driver,
            grants: match disk {
                Some((_, _, 3)) => &[12, 13, 14, 15, 18, 17],
                _ => &[12, 13, 14, 15, 18], // no MSI-X: the driver polls
            },
            process: 0,
            restarts: [0; MAX_RESTARTS],
            restart_count: 0,
            given_up: disk.is_none(),
            watch: blk_watch,
        },
        Service {
            name: card_driver,
            grants: match card {
                Some((_, _, 3)) => &[28, 29, 30, 31, 33],
                _ => &[28, 29, 30, 31], // no interrupt: the driver polls
            },
            process: 0,
            restarts: [0; MAX_RESTARTS],
            restart_count: 0,
            given_up: card.is_none(),
            watch: nic_watch,
        },
        Service {
            name: "net",
            grants: &[34, 36],
            process: 0,
            restarts: [0; MAX_RESTARTS],
            restart_count: 0,
            given_up: card.is_none(),
            watch: net_watch,
        },
        Service { name: "shell", grants: &[26], process: 0, restarts: [0; MAX_RESTARTS], restart_count: 0, given_up: false, watch: None },
        // The keys: the PS/2 keyboard's driver (which is the mouse's
        // too, and so may send to the display: 37), or the serial
        // port's - which is also the screen, and so reads the display's
        // channel (6). Neither, where the keys are virtio devices'.
        Service {
            name: if serial_terminal { "serial" } else { "kbd" },
            grants: if serial_terminal { &[0, 1, 3, 6, 23] } else { &[0, 1, 2, 3, 23, 37] },
            process: 0,
            restarts: [0; MAX_RESTARTS],
            restart_count: 0,
            given_up: !serial_terminal && !PS2_KEYBOARD,
            watch: kbd_watch,
        },
        // Virtio keyboards and pointers: keys to the shell (3),
        // movements to the display (37).
        Service {
            name: "hid",
            grants: &[38, 3, 37, 39],
            process: 0,
            restarts: [0; MAX_RESTARTS],
            restart_count: 0,
            given_up: serial_terminal || !has_inputs,
            watch: hid_watch,
        },
        // The USB controller, and its keyboards and mice: keys to the
        // shell (3), movements to the display (37), as hid's. (The
        // disk's requests, when a stick is the disk, come with the
        // controller's description: a process starts with 8 handles at
        // most.)
        Service {
            name: "usb",
            grants: match usb_controller {
                Some((_, _, 3)) => &[46, 47, 48, 3, 37, 49, 50],
                _ => &[46, 47, 48, 3, 37, 49], // no MSI: the driver polls
            },
            process: 0,
            restarts: [0; MAX_RESTARTS],
            restart_count: 0,
            given_up: usb_controller.is_none(),
            watch: usb_watch,
        },
        // Switching off and resetting, when init asks: nobody else can.
        Service {
            name: "power",
            grants: &[44, 45],
            process: 0,
            restarts: [0; MAX_RESTARTS],
            restart_count: 0,
            given_up: power.is_none(),
            watch: None,
        },
    ];
    let launch = |service: &Service| {
        // The input devices, before their driver starts: it takes what
        // is waiting and no more. Whatever an earlier one left unread
        // is thrown away first - it would be the same devices twice.
        if service.name == "hid" {
            let mut stale = [0u8; abi::MESSAGE_MAX as usize];
            let mut handles = [0u64; abi::MESSAGE_HANDLES_MAX as usize];
            while let Ok(Some(_)) = taaj::poll(&[hid_devices_rx]) {
                let Ok((_, count)) = taaj::receive(hid_devices_rx, &mut stale, &mut handles) else { break };
                for &handle in &handles[..count] {
                    let _ = taaj::close(handle);
                }
            }
            for (info, handles, count) in inputs.iter().flatten() {
                // Copies that can be handed over, with the rights the
                // driver needs: registers and DMA memory mapped to read
                // and write, the interrupt to wait on.
                let mut copies = [0u64; 3];
                let mut made = 0;
                for (n, &handle) in handles[..*count].iter().enumerate() {
                    let rights = if n == 2 { RECEIVE } else { READ | WRITE };
                    match taaj::duplicate(handle, rights | TRANSFER) {
                        Ok(copy) => {
                            copies[n] = copy;
                            made += 1;
                        }
                        Err(_) => break,
                    }
                }
                if made < *count || taaj::send(hid_devices_tx, &info.to_bytes(), &copies[..made]).is_err() {
                    for &copy in &copies[..made] {
                        let _ = taaj::close(copy);
                    }
                }
            }
        }
        let mut list = [&grants[0]; 8];
        for (n, &g) in service.grants.iter().enumerate() {
            list[n] = &grants[g];
        }
        // tmp is the fs program, under another name.
        let program = if service.name == "tmp" { "fs" } else { service.name };
        let process = start(launcher, program, &list[..service.grants.len()], 0)?;
        // Each new display needs to be told what it is drawing on.
        if let (Some(fb), "display") = (framebuffer, service.name) {
            let mut info = [0u8; 1 + FramebufferInfo::SIZE];
            info[0] = abi::display::INFO;
            info[1..].copy_from_slice(&fb.to_bytes());
            // The queue may be full of text nobody showed - that is how
            // a display that stopped looks from here. The new one
            // throws text away until its info comes, so there is room
            // as soon as it has run.
            let started = taaj::ticks();
            loop {
                match taaj::send(display_tx, &info, &[]) {
                    Err(Error::QueueFull) if taaj::ticks() - started < 200 => taaj::yield_now(),
                    sent => break sent?,
                }
            }
        }
        // Each new registry, everything: the services first, then the
        // namespaces - so that when it learns of a client it already
        // knows all that client may find.
        if service.name == "registry" {
            let mut message = [0u8; 256];
            for &(name, grant) in &registered {
                let rights = grants[grant].rights;
                let length = Admin::Register { name, rights }.encode(&mut message).ok_or(Error::InvalidArgument)?;
                tell(&message[..length], grants[grant].handle, rights)?;
            }
            for &(label, channel, names) in &namespaces {
                let length = Admin::Namespace { label, names: names.to_vec() }.encode(&mut message).ok_or(Error::InvalidArgument)?;
                tell(&message[..length], channel, RECEIVE)?;
            }
        }
        // And each new disk driver, what its device is.
        if let (Some((info, _, _)), true) = (disk, service.name == disk_driver) {
            match info {
                Device::Pci(info) => taaj::send(disk_info_tx, &info.to_bytes(), &[])?,
                Device::Mmio(info) => taaj::send(disk_info_tx, &info.to_bytes(), &[])?,
            }
        }
        // And each new power: its description, with fresh copies of the
        // handles - the ports, the DSDT to read (never write), or the
        // power handle - that it keeps for as long as it runs.
        if let (Some((info, handles, count)), "power") = (power, service.name) {
            let mut copies = [0u64; 5];
            for (n, &handle) in handles[..count].iter().enumerate() {
                let rights = if info.kind == abi::power::ACPI && n == 0 { READ } else { 0 };
                copies[n] = taaj::duplicate(handle, rights | TRANSFER)?;
            }
            if let Err(error) = taaj::send(power_info_tx, &info.to_bytes(), &copies[..count]) {
                for &copy in &copies[..count] {
                    let _ = taaj::close(copy);
                }
                return Err(error);
            }
        }
        // And each new USB driver, what its controller is - and, when a
        // stick is the disk, fresh copies of the disk's two request
        // channels' receiving ends (which may write, which only read).
        if let (Some((info, _, _)), "usb") = (usb_controller, service.name) {
            let mut copies = [0u64; 2];
            let count = if usb_disk { 2 } else { 0 };
            for (n, &channel) in [disk_in, disk_look_in].iter().take(count).enumerate() {
                copies[n] = taaj::duplicate(channel, RECEIVE | TRANSFER)?;
            }
            if let Err(error) = taaj::send(usb_info_tx, &info.to_bytes(), &copies[..count]) {
                for &copy in &copies[..count] {
                    let _ = taaj::close(copy);
                }
                return Err(error);
            }
        }
        // And each new network card's driver.
        if let (Some((info, _, _)), true) = (card, service.name == card_driver) {
            match info {
                Device::Pci(info) => taaj::send(card_info_tx, &info.to_bytes(), &[])?,
                Device::Mmio(info) => taaj::send(card_info_tx, &info.to_bytes(), &[])?,
            }
        }
        prioritize(service.name, process);
        Ok::<Handle, Error>(process)
    };

    for service in services.iter_mut().filter(|s| !s.given_up) {
        match launch(service) {
            Ok(process) => service.process = process,
            Err(error) => {
                println!("[init] could not start {}: {error:?}", service.name);
                service.given_up = true;
            }
        }
    }
    // One line, written at once: other services are already printing.
    let mut line = Buffer::<128>::new();
    let _ = write!(line, "[init] started:");
    for service in services.iter().filter(|s| !s.given_up) {
        let _ = write!(line, " {}", service.name);
    }
    println!("{}; supervising", core::str::from_utf8(line.as_bytes()).unwrap_or("?"));

    loop {
        // Watch every service still running, and the request channel.
        let mut watched = [0u64; 24];
        let mut which = [0usize; 24];
        let mut count = 0;
        for (n, service) in services.iter().enumerate() {
            if !service.given_up {
                watched[count] = service.process;
                which[count] = n;
                count += 1;
            }
        }
        watched[count] = requests;
        let processes = count;

        // The watchdog. Ask whoever is due; end whoever has been silent
        // too long - its process then ends, and is restarted below like
        // any service that ended.
        let now = taaj::ticks();
        let mut wake = u64::MAX;
        for (n, service) in services.iter_mut().enumerate() {
            let (process, name) = (service.process, service.name);
            let Some(watch) = service.watch.as_mut().filter(|_| !service.given_up) else { continue };
            watch.ask(now);
            if watch.deadline.is_some_and(|deadline| now >= deadline) {
                println!("[init] {name} has not answered for {} s - stopping it", watch.patience / 100);
                let _ = taaj::kill(process);
                // Until it has ended and been replaced: nothing to ask.
                watch.deadline = None;
                watch.next_ping = u64::MAX;
            }
            wake = wake.min(watch.wake_at(now));
            count += 1;
            watched[count] = watch.answers_rx;
            which[count] = n;
        }

        let index = match taaj::object_wait_until(&watched[..=count], wake) {
            Ok(Some(index)) => index,
            Ok(None) => continue, // time for the watchdog
            Err(error) => {
                // Without init nothing is supervised: say why, at least.
                println!("[init] cannot watch the services: {error:?} - giving up");
                return 3;
            }
        };
        if index == processes {
            let power_ready = services.iter().any(|s| s.name == "power" && !s.given_up);
            if let Some(action) = serve_request(launcher, requests, power_ready) {
                shut_down(&mut services, action, power_ask, files_look_out, [disk_driver, card_driver]);
            }
            continue;
        }
        if index > processes {
            if let Some(watch) = services[which[index]].watch.as_mut() {
                watch.answered(taaj::ticks());
            }
            continue;
        }
        // A service ended; it is ready, so this returns at once.
        let Ok((_, code)) = taaj::wait_any(&watched[index..=index]) else { return 3 };
        let service = &mut services[which[index]];
        let _ = taaj::close(service.process);

        // Too many restarts lately?
        let now = taaj::ticks();
        let recent = service.restarts[..service.restart_count].iter().filter(|&&t| now - t < RESTART_WINDOW).count();
        if recent >= MAX_RESTARTS {
            println!("[init] {} ended (code {code}) {recent} times in 30 s - giving up on it", service.name);
            service.given_up = true;
            // No disk driver will come: say so to whoever waits on one.
            // The requests' channels lose their reading end, every
            // request in them is dropped with its reply channel, and fs
            // - waiting for an answer - hears "peer closed" and goes on
            // with files in memory, as it does with no disk at all. (A
            // controller with no disk on it, say: an AHCI controller
            // whose only drive is the CD.)
            if service.name == disk_driver {
                let _ = (taaj::close(disk_in), taaj::close(disk_look_in));
            }
            continue;
        }
        if service.restart_count == MAX_RESTARTS {
            service.restarts.rotate_left(1);
            service.restart_count -= 1;
        }
        service.restarts[service.restart_count] = now;
        service.restart_count += 1;

        println!("[init] {} ended with code {code} - restarting it", service.name);
        match launch(service) {
            Ok(process) => service.process = process,
            Err(error) => {
                println!("[init] could not restart {}: {error:?}", service.name);
                service.given_up = true;
            }
        }
        if let Some(watch) = service.watch.as_mut() {
            watch.restarted(taaj::ticks());
        }
    }
}

/// Answers one request from the shell: start a program (abi::init).
///
/// A BUILT_IN program is one in the kernel image that a user may run.
/// An IMAGE is a program the shell read from a file; its first handle
/// is the memory object holding it, and the name only what to call it.
///
/// What the program can do is decided by the handles that came with the
/// request, and by nothing else. They are the shell's own - a copy of
/// its channel to the screen, of its channel to the file service - and
/// init passes them on, each cut down to "send". init gives nothing of
/// its own: it holds the launcher, the disk's ports, every channel in
/// the system, and a request that could make init reach for those on
/// someone's behalf would make the shell as powerful as init. This way
/// the shell can hand down what it has, and no more. That is also why
/// it is safe to run whatever is in a file: a process can only use what
/// it holds a handle to.
///
/// The arguments are the one thing init makes: a channel with a single
/// message in it, whose receiving end the program gets. The directory
/// the program starts in travels the same way: it is text the shell
/// chose, like the arguments, and init does not look at it.
///
/// The shell reads the file, not init. init restarts the file service
/// when it crashes; if init itself waited on the file service, a hung
/// one would stop the only process able to fix it.
fn serve_request(launcher: Handle, requests: Handle, power_ready: bool) -> Option<u8> {
    use abi::init::{Run, BUILT_IN, IMAGE};
    use abi::startup::{ARGUMENTS, DESCRIPTORS, DIRECTORY, HANDLES, KEYBOARD, SCREEN, SERVICES, STDERR, STDIN, STDOUT};

    // Room for any message and any number of handles: a request that
    // did not fit would stay at the head of the queue forever.
    let mut message = [0u8; abi::MESSAGE_MAX as usize];
    let mut handles = [0u64; abi::MESSAGE_HANDLES_MAX as usize];
    let Ok((n, count)) = taaj::receive(requests, &mut message, &mut handles) else { return None };
    let close = |handles: &[Handle]| {
        for &handle in handles {
            let _ = taaj::close(handle);
        }
    };
    // Switching off, or resetting: init's to do (shut_down), once the
    // services are stopped - if this machine can be switched off at all.
    let asked = &message[..n];
    if asked == abi::power::ASK_OFF || asked == abi::power::ASK_RESET {
        close(&handles[..count]);
        if !power_ready {
            let _ = taaj::send(requests, b"error: this machine cannot be switched off or reset from here", &[]);
            return None;
        }
        return Some(if asked == abi::power::ASK_OFF { abi::power::OFF } else { abi::power::RESET });
    }
    // One handle for the image, one for each flag, one for the
    // descriptors and one for the reply if they come - no more, no
    // fewer.
    let run = Run::decode(&message[..n]).filter(|run| {
        let flags = u64::from(run.flags);
        flags & !(STDOUT | SERVICES | STDIN | STDERR | KEYBOARD | SCREEN) == 0
            && count
                == usize::from(run.what() == IMAGE)
                    + (flags & HANDLES).count_ones() as usize
                    + usize::from(run.has_descriptors())
                    + usize::from(run.replies_apart())
    });
    let Some(run) = run else {
        close(&handles[..count]);
        let _ = taaj::send(requests, b"error: unknown request", &[]);
        return None;
    };
    // Where the reply goes: back on the shared channel, or on the
    // asker's own (abi::init::REPLY), which init lets go of when done.
    struct Apart(Option<Handle>);
    impl Drop for Apart {
        fn drop(&mut self) {
            if let Some(channel) = self.0 {
                let _ = taaj::close(channel);
            }
        }
    }
    let apart = Apart(run.replies_apart().then(|| handles[count - 1]));
    let count = count - usize::from(apart.0.is_some());
    let to = apart.0.unwrap_or(requests);
    let reply = |text: &[u8], handles: &[Handle]| {
        let _ = taaj::send(to, text, handles);
    };
    let (image, passed) = match run.what() {
        IMAGE => (Some(handles[0]), &handles[1..count]),
        _ => (None, &handles[..count]),
    };
    if run.what() == BUILT_IN && !RUNNABLE.contains(&run.name) {
        close(passed);
        reply(b"error: no such program, on the disk or built in", &[]);
        return None;
    }

    // The arguments, waiting in a channel of their own - and the
    // directory the program starts in, in another. The root, which
    // needs no saying, means no channel for the directory.
    let waiting = |text: &[u8]| -> Result<Option<Handle>, Error> {
        if text.is_empty() {
            return Ok(None);
        }
        let (tx, rx) = taaj::channel_create()?;
        let sent = taaj::send(tx, text, &[]);
        let _ = taaj::close(tx);
        sent.map(|()| Some(rx)).inspect_err(|_| close(&[rx]))
    };
    // The command line is always sent, even with no arguments: after
    // them (an empty message, if none) comes the program's name and its
    // file (abi::startup::ARGUMENTS), which a C program needs as argv[0],
    // and clang to find its headers.
    let command_line = || -> Result<Option<Handle>, Error> {
        let mut program = [0u8; 2 * 256 + 2];
        let length = abi::arguments::encode([run.name, run.file].into_iter(), &mut program).ok_or(Error::InvalidArgument)?;
        let (tx, rx) = taaj::channel_create()?;
        // Then, if the asker gave one, the environment (abi::init::
        // ENVIRONMENT): a third message.
        let (arguments, environment) = run.parts();
        let sent = taaj::send(tx, arguments, &[])
            .and_then(|()| taaj::send(tx, &program[..length], &[]))
            .and_then(|()| if environment.is_empty() { Ok(()) } else { taaj::send(tx, environment, &[]) });
        let _ = taaj::close(tx);
        sent.map(|()| Some(rx)).inspect_err(|_| close(&[rx]))
    };
    let arguments = command_line();
    let directory = waiting(run.directory.as_bytes());
    let (arguments, directory) = match (arguments, directory) {
        (Ok(arguments), Ok(directory)) => (arguments, directory),
        (arguments, directory) => {
            // One could not be made: the other, if it was, is not needed.
            for made in [&arguments, &directory] {
                if let Ok(Some(rx)) = made {
                    close(&[*rx]);
                }
            }
            close(&handles[..count]);
            let error = arguments.err().or(directory.err()).unwrap_or(Error::InvalidArgument);
            let mut text = Buffer::<64>::new();
            let _ = write!(text, "error: {error:?}");
            reply(text.as_bytes(), &[]);
            return None;
        }
    };

    // The startup handles, in the order of the flags' bits: stdout,
    // arguments, services, directory, stdin, stderr, descriptors. What
    // the shell passed is cut down to the one thing it is for: an output
    // to send on, an input to receive from.
    let mut given = [(0u64, 0u32); 7];
    let mut length = 0;
    let mut flags = u64::from(run.flags);
    if run.has_descriptors() {
        flags |= DESCRIPTORS;
    }
    let mut passed = passed.iter().copied();
    for flag in [STDOUT, ARGUMENTS, SERVICES, DIRECTORY, STDIN, STDERR, DESCRIPTORS] {
        let entry = match flag {
            // The descriptors' channel: only to read them from. (What
            // comes on it - pipe ends, streams - carries rights of its
            // own.)
            DESCRIPTORS if flags & flag != 0 => passed.next().map(|handle| (handle, RECEIVE)),
            ARGUMENTS => arguments.map(|rx| (rx, RECEIVE)),
            DIRECTORY => directory.map(|rx| (rx, RECEIVE)),
            // An input that is the keyboard is also sent on: the
            // program asks for each line (abi::stream::WANT).
            STDIN if flags & flag != 0 && flags & KEYBOARD != 0 => passed.next().map(|handle| (handle, RECEIVE | SEND)),
            // Any other input may be handed on, as the outputs are: a
            // program reading a pipe or a file can give it to one it
            // starts (spawn.rs in libc).
            STDIN if flags & flag != 0 => passed.next().map(|handle| (handle, RECEIVE | DELEGABLE)),
            // Outputs and the services: to send on, and to hand on to a
            // program this one starts (spawn.rs in libc) - a copy can do
            // no more than the handle it was made from.
            _ if flags & flag != 0 => passed.next().map(|handle| (handle, SEND | DELEGABLE)),
            _ => None,
        };
        if let Some(entry) = entry {
            given[length] = entry;
            length += 1;
            flags |= flag;
        }
    }
    let given = &given[..length];

    let result = match image {
        Some(image) => taaj::spawn_image(launcher, image, run.name, given, flags),
        None => taaj::spawn(launcher, run.name, given, flags),
    };
    // The image: the kernel has its own copy by now.
    if let Some(image) = image {
        close(&[image]);
    }
    match result {
        Ok(process) => {
            // For the shell, or for a program it let start programs
            // (abi::init::REPLY).
            let asker = if run.replies_apart() { "a program" } else { "the shell" };
            println!("[init] started {} for {asker}{}", run.name, if image.is_some() { ", from a file" } else { "" });
            reply(b"ok", &[process]);
        }
        Err(error) => {
            // Not started: its handles are still here.
            for &(handle, _) in given {
                close(&[handle]);
            }
            let mut text = Buffer::<64>::new();
            let _ = match error {
                Error::NotExecutable => write!(text, "error: {} is not a program", run.name),
                error => write!(text, "error: {error:?}"),
            };
            reply(text.as_bytes(), &[]);
        }
    }
    None
}

/// Switches the machine off, or resets it (abi::power::OFF, RESET):
/// first the services are stopped, in an order that leaves the disk
/// whole, then `power` is asked. Returns only if that did not work -
/// and then with the services stopped, which is what was wanted anyway:
/// it says it is safe to switch off by hand, and does nothing more.
///
/// The order: first whoever could ask for more work - the shell (and,
/// with it, nobody types any more), the input drivers, the network.
/// Then the file service, but only once it has answered a request: it
/// serves one at a time, and answering means it is between two, with
/// every change it has answered for on the disk (fs.rs: each request
/// is one copy-on-write commit). Even if a program it was not told to
/// stop slips a request in after that, TaajFS's commits are whole or
/// not at all. Then the disk driver, with nothing left to do; then the
/// rest. `power` last: it is the one that does the switching.
fn shut_down(services: &mut [Service], action: u8, power: Handle, files_look: Handle, [disk_driver, card_driver]: [&str; 2]) {
    let what = if action == abi::power::OFF { "switching off" } else { "resetting" };
    println!("[init] {what}: stopping the services");
    // usb stops with the keyboards - unless it is the disk's driver,
    // which waits for fs.
    let usb = if disk_driver == "usb" { "" } else { "usb" };
    let order = ["shell", "hid", usb, "kbd", "serial", "net", card_driver, "dev", "tmp", "fs", disk_driver, "display", "registry"];
    for name in order {
        let Some(service) = services.iter_mut().find(|s| s.name == name && !s.given_up) else { continue };
        let between = name != "fs" || between_requests(files_look);
        let _ = taaj::kill(service.process);
        let ended = matches!(taaj::object_wait_until(&[service.process], taaj::ticks() + 200), Ok(Some(_)));
        let _ = taaj::close(service.process);
        service.given_up = true; // never restarted: nothing watches any more
        println!(
            "[init] stopped {name}{}{}",
            if between { "" } else { " (it did not answer first)" },
            if ended { "" } else { " (it did not end within 2 s)" }
        );
    }
    let mut answer = [0u8; 128];
    let mut said = 0;
    if taaj::send(power, &[action], &[]).is_ok() {
        // power answers only if it could not: after its own waits
        // (ACPI mode, then the machine's stopping) - 5 s at most.
        if let Ok(Some(_)) = taaj::object_wait_until(&[power], taaj::ticks() + 800) {
            said = taaj::receive(power, &mut answer, &mut []).map_or(0, |(n, _)| n);
        }
    }
    let why = core::str::from_utf8(&answer[..said]).ok().filter(|why| !why.is_empty()).unwrap_or("no answer from power");
    println!("[init] could not finish {what}: {why}. The services are stopped: it is safe to switch the machine off by hand");
    loop {
        taaj::sleep(60 * 100);
    }
}

/// Asks the file service something any client may, and waits up to
/// 4 s for its answer: true once it has answered - it is then between
/// two requests.
fn between_requests(files_look: Handle) -> bool {
    let ping = [fileserver::protocol::ACCESS, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    let Ok((answers_tx, answers_rx)) = taaj::channel_create() else { return false };
    let asked = taaj::send(files_look, &ping, &[answers_tx]).is_ok();
    if !asked {
        let _ = taaj::close(answers_tx);
    }
    let answered = asked && matches!(taaj::object_wait_until(&[answers_rx], taaj::ticks() + 400), Ok(Some(_)));
    let _ = taaj::close(answers_rx);
    answered
}
