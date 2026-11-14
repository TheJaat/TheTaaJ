//! init - the first process, and the supervisor of all the others.
//!
//! The kernel starts only this, with six startup handles:
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
//! The services: display (the screen), fs (files - on the disk, if
//! there is one), blk (the disk driver), shell, and kbd (the keyboard
//! driver).
//!
//! init also runs programs for the shell. The shell has no launcher; it
//! sends "run NAME" on a channel to init, which starts the program - if
//! it is one a user may run - with the screen as its stdout, and replies
//! "ok" with a handle to the new process, or an error.
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
//! A service that keeps crashing is not restarted forever. More than
//! MAX_RESTARTS within RESTART_WINDOW ticks and init gives up on it -
//! restarting in a tight loop would only hide the bug and burn the CPU.

#![no_std]
#![no_main]

use abi::{FramebufferInfo, PciDeviceInfo};
use taaj::rights::{DUPLICATE, READ, RECEIVE, SEND, TRANSFER, WRITE};
use core::fmt::Write;

use taaj::{println, Buffer, Error, Handle};

taaj::program!(main);

/// What the shell may ask init to run: programs, not services.
const RUNNABLE: &[&str] = &["hello", "peek"];

const MAX_RESTARTS: usize = 5;
const RESTART_WINDOW: u64 = 30 * 100; // 30 seconds of 100 Hz ticks

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
}

/// Starts `name` with a fresh duplicate of each grant. A duplicate is
/// made with TRANSFER (needed to hand it over) and then handed over with
/// only the rights in the grant - so the service cannot pass it on.
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
    let (Some(launcher), Some(interrupt), Some(data_port), Some(status_port), Some(devices), Some(system)) = (
        taaj::startup_handle(0),
        taaj::startup_handle(1),
        taaj::startup_handle(2),
        taaj::startup_handle(3),
        taaj::startup_handle(4),
        taaj::startup_handle(5),
    ) else {
        println!("[init] started without the kernel's handles");
        return 1;
    };
    let Ok((keys_out, keys_in)) = taaj::channel_create() else { return 2 };

    // What the kernel found: a message per device, then the channel
    // closes. The screen - its description and memory - and a disk: its
    // description and its ports, DMA memory and interrupt.
    let mut framebuffer = None;
    let mut fb_memory = 0;
    let mut disk: Option<(PciDeviceInfo, [Handle; 3], usize)> = None;
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
            if info.device_id == virtio::PCI_DEVICE_BLOCK && disk.is_none() && (2..=3).contains(&count) {
                disk = Some((info, [handles[0], handles[1], handles[2]], count));
                used = true;
            } else {
                println!("[init] no driver for PCI device {:04x}:{:04x}", info.vendor_id, info.device_id);
            }
        }
        if !used {
            for &handle in &handles[..count] {
                let _ = taaj::close(handle);
            }
        }
    }
    let _ = taaj::close(devices);
    let Ok((display_tx, display_rx)) = taaj::channel_create() else { return 2 };
    let Ok((requests, shell_requests)) = taaj::channel_create() else { return 2 };
    // The file service's requests. init keeps both ends, as it does the
    // keyboard's: a restarted fs takes over the same queue, and clients
    // keep the channel they have.
    let Ok((files_in, files_out)) = taaj::channel_create() else { return 2 };
    // The disk's: its description for each new driver, and requests.
    let Ok((disk_info_tx, disk_info_rx)) = taaj::channel_create() else { return 2 };
    // Two request channels: reads and writes for fs, which owns what is
    // on the disk; reads only for the shell, which may look.
    let Ok((disk_in, disk_out)) = taaj::channel_create() else { return 2 };
    let Ok((disk_look_in, disk_look_out)) = taaj::channel_create() else { return 2 };
    if disk.is_none() {
        // No disk: no one will ever answer, so say so at once - a
        // client's send fails with PeerClosed instead of waiting forever.
        let _ = (taaj::close(disk_in), taaj::close(disk_look_in));
    }
    let [disk_ports, disk_dma, disk_interrupt] = disk.map_or([0; 3], |(_, handles, _)| handles);

    // Everything a service may be given, and the rights it gets.
    let grants = [
        Grant { handle: interrupt, rights: RECEIVE },    // 0
        Grant { handle: data_port, rights: 0 },          // 1
        Grant { handle: status_port, rights: 0 },        // 2
        Grant { handle: keys_out, rights: SEND },        // 3
        Grant { handle: keys_in, rights: RECEIVE },      // 4
        Grant { handle: fb_memory, rights: READ | WRITE },    // 5
        Grant { handle: display_rx, rights: RECEIVE },   // 6
        Grant { handle: display_tx, rights: SEND },      // 7: text to the screen
        Grant { handle: shell_requests, rights: SEND | RECEIVE }, // 8: requests to init
        Grant { handle: system, rights: 0 },             // 9: looking, not touching
        Grant { handle: files_in, rights: RECEIVE },     // 10: requests, for fs
        Grant { handle: files_out, rights: SEND },       // 11: requests, to fs
        Grant { handle: disk_ports, rights: 0 },          // 12: the disk's ports
        Grant { handle: disk_dma, rights: READ | WRITE }, // 13: its DMA memory
        Grant { handle: disk_info_rx, rights: RECEIVE },  // 14: its description
        Grant { handle: disk_in, rights: RECEIVE },       // 15: requests, for blk
        Grant { handle: disk_out, rights: SEND },         // 16: requests, to blk
        Grant { handle: disk_interrupt, rights: RECEIVE }, // 17: its interrupt
        Grant { handle: disk_look_in, rights: RECEIVE },  // 18: read-only requests, for blk
        Grant { handle: disk_look_out, rights: SEND },    // 19: read-only requests, to blk
    ];
    debug_assert!(grants.iter().all(|g| g.rights & DUPLICATE == 0));

    // The display first: it must have its info before the shell's text
    // arrives on the same channel.
    let mut services = [
        Service {
            name: "display",
            grants: &[5, 6],
            process: 0,
            restarts: [0; MAX_RESTARTS],
            restart_count: 0,
            given_up: framebuffer.is_none(),
        },
        Service { name: "fs", grants: &[10, 16], process: 0, restarts: [0; MAX_RESTARTS], restart_count: 0, given_up: false },
        Service {
            name: "blk",
            grants: match disk {
                Some((_, _, 3)) => &[12, 13, 14, 15, 18, 17],
                _ => &[12, 13, 14, 15, 18], // no MSI-X: the driver polls
            },
            process: 0,
            restarts: [0; MAX_RESTARTS],
            restart_count: 0,
            given_up: disk.is_none(),
        },
        Service { name: "shell", grants: &[4, 7, 8, 9, 11, 19], process: 0, restarts: [0; MAX_RESTARTS], restart_count: 0, given_up: false },
        Service { name: "kbd", grants: &[0, 1, 2, 3], process: 0, restarts: [0; MAX_RESTARTS], restart_count: 0, given_up: false },
    ];
    let launch = |service: &Service| {
        let mut list = [&grants[0]; 8];
        for (n, &g) in service.grants.iter().enumerate() {
            list[n] = &grants[g];
        }
        let process = start(launcher, service.name, &list[..service.grants.len()], 0)?;
        // Each new display needs to be told what it is drawing on.
        if let (Some(fb), "display") = (framebuffer, service.name) {
            let mut info = [0u8; 1 + FramebufferInfo::SIZE];
            info[0] = abi::display::INFO;
            info[1..].copy_from_slice(&fb.to_bytes());
            taaj::send(display_tx, &info, &[])?;
        }
        // And each new disk driver, what its device is.
        if let (Some((info, _, _)), "blk") = (disk, service.name) {
            taaj::send(disk_info_tx, &info.to_bytes(), &[])?;
        }
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
        let mut watched = [0u64; 8];
        let mut which = [0usize; 8];
        let mut count = 0;
        for (n, service) in services.iter().enumerate() {
            if !service.given_up {
                watched[count] = service.process;
                which[count] = n;
                count += 1;
            }
        }
        watched[count] = requests;
        let index = match taaj::object_wait(&watched[..=count]) {
            Ok(index) => index,
            Err(_) => return 3,
        };
        if index == count {
            serve_request(launcher, requests, framebuffer.map(|_| display_tx));
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
    }
}

/// Answers one request from the shell. Requests are text: "run NAME".
fn serve_request(launcher: Handle, requests: Handle, screen: Option<Handle>) {
    let mut message = [0u8; 64];
    let Ok((n, _)) = taaj::receive(requests, &mut message, &mut []) else { return };
    let reply = |text: &[u8], handles: &[Handle]| {
        let _ = taaj::send(requests, text, handles);
    };
    let Some(name) = message[..n].strip_prefix(b"run ").and_then(|n| core::str::from_utf8(n).ok()) else {
        return reply(b"error: unknown request", &[]);
    };
    if !RUNNABLE.contains(&name) {
        return reply(b"error: not a program you can run", &[]);
    }
    // Its stdout: a send-only copy of the display channel.
    let result = match screen {
        Some(screen) => match taaj::duplicate(screen, SEND | TRANSFER) {
            Ok(copy) => taaj::spawn(launcher, name, &[(copy, SEND)], abi::startup::STDOUT).inspect_err(|_| {
                let _ = taaj::close(copy);
            }),
            Err(error) => Err(error),
        },
        None => taaj::spawn(launcher, name, &[], 0),
    };
    match result {
        Ok(process) => {
            println!("[init] started {name} for the shell");
            reply(b"ok", &[process]);
        }
        Err(error) => {
            let mut text = Buffer::<64>::new();
            let _ = write!(text, "error: {error:?}");
            reply(text.as_bytes(), &[]);
        }
    }
}
