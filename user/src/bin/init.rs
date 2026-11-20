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
//! The services: registry (where the others are found), display (the
//! screen), fs (files - on the disk, if there is one), blk (the disk
//! driver), shell, and kbd (the keyboard driver).
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

use abi::{FramebufferInfo, PciDeviceInfo};
use registry::protocol::Admin;
use taaj::rights::{DUPLICATE, READ, RECEIVE, SEND, TRANSFER, WRITE};
use core::fmt::Write;

use taaj::{println, Buffer, Error, Handle};

taaj::program!(main);

/// What the shell may ask init to run: programs, not services.
const RUNNABLE: &[&str] = &["hello", "peek"];

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
        let mut reply = [0u8; 16];
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
    // The registry's: one channel for what init tells it, and one per
    // client for what that client asks. init keeps both ends of each,
    // as it does of every channel: a registry that is replaced takes
    // over the same queues, and clients keep the channel they have.
    let Ok((admin_tx, admin_rx)) = taaj::channel_create() else { return 2 };
    let Ok((shell_names_tx, shell_names_rx)) = taaj::channel_create() else { return 2 };
    let Ok((fs_names_tx, fs_names_rx)) = taaj::channel_create() else { return 2 };

    // Everything a service may be given, and the rights it gets.
    let grants = [
        Grant { handle: interrupt, rights: RECEIVE },    // 0
        Grant { handle: data_port, rights: 0 },          // 1
        Grant { handle: status_port, rights: 0 },        // 2
        Grant { handle: keys_out, rights: SEND },        // 3
        Grant { handle: keys_in, rights: RECEIVE },      // 4
        Grant { handle: fb_memory, rights: READ | WRITE },    // 5
        Grant { handle: display_rx, rights: RECEIVE },   // 6
        Grant { handle: display_tx, rights: SEND | DELEGABLE }, // 7: text to the screen
        Grant { handle: shell_requests, rights: SEND | RECEIVE }, // 8: requests to init
        Grant { handle: system, rights: 0 },             // 9: looking, not touching
        Grant { handle: files_in, rights: RECEIVE },     // 10: requests, for fs
        Grant { handle: files_out, rights: SEND | DELEGABLE }, // 11: requests, to fs
        Grant { handle: disk_ports, rights: 0 },          // 12: the disk's ports
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
    ];
    // Only the shell's screen and files can be copied and passed on.
    debug_assert!(grants.iter().enumerate().all(|(n, g)| g.rights & DELEGABLE == 0 || matches!(n, 7 | 11 | 21)));

    // The watchdog's pings: for fs, "what does this channel allow?" on
    // the read-only channel; for blk, "what disk is this?". fs gets the
    // longer patience - it waits on blk, and must outlast blk's
    // replacement.
    let fs_ping = [fileserver::protocol::ACCESS, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    let fs_watch = Watch::new(files_look_out, &fs_ping, 400);
    let blk_watch = disk.and_then(|_| Watch::new(disk_look_out, &[abi::block::INFO], 200));
    // The display and the keyboard driver: a ping on a channel of its own.
    let display_watch = framebuffer.and_then(|_| Watch::new(display_ping_tx, &[abi::PING], 200));
    let kbd_watch = Watch::new(kbd_ping_tx, &[abi::PING], 200);

    // What the registry is told, each time one is started. The
    // services: a name, and which grant it means - the handle, and the
    // rights a client's copy gets. (The shell's old startup handles,
    // now asked for by name; and the disk, for fs.)
    let registered: [(&str, usize); 8] = [
        ("keys", 4),
        ("display", 7),
        ("init", 8),
        ("system", 9),
        ("files", 11),
        ("files.look", 21),
        ("disk", 16),
        ("disk.look", 19),
    ];
    // And the namespaces: whose, the receiving end of its channel, and
    // the names that exist on it. This table is the system's policy,
    // in one place: who may reach what.
    let namespaces: [(&str, Handle, &[&str]); 2] = [
        ("shell", shell_names_rx, &["keys", "display", "init", "system", "files", "files.look", "disk.look"]),
        ("fs", fs_names_rx, &["disk"]),
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
            grants: if screen_text != 0 { &[5, 6, 22, 24] } else { &[5, 6, 22] },
            process: 0,
            restarts: [0; MAX_RESTARTS],
            restart_count: 0,
            given_up: framebuffer.is_none(),
            watch: display_watch,
        },
        Service { name: "fs", grants: &[10, 20, 27], process: 0, restarts: [0; MAX_RESTARTS], restart_count: 0, given_up: false, watch: fs_watch },
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
            watch: blk_watch,
        },
        Service { name: "shell", grants: &[26], process: 0, restarts: [0; MAX_RESTARTS], restart_count: 0, given_up: false, watch: None },
        Service { name: "kbd", grants: &[0, 1, 2, 3, 23], process: 0, restarts: [0; MAX_RESTARTS], restart_count: 0, given_up: false, watch: kbd_watch },
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
        let mut watched = [0u64; 16];
        let mut which = [0usize; 16];
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
            serve_request(launcher, requests);
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
fn serve_request(launcher: Handle, requests: Handle) {
    use abi::init::{Run, BUILT_IN, IMAGE};
    use abi::startup::{ARGUMENTS, DIRECTORY, HANDLES, SCREEN, SERVICES, STDERR, STDIN, STDOUT};

    // Room for any message and any number of handles: a request that
    // did not fit would stay at the head of the queue forever.
    let mut message = [0u8; abi::MESSAGE_MAX as usize];
    let mut handles = [0u64; abi::MESSAGE_HANDLES_MAX as usize];
    let Ok((n, count)) = taaj::receive(requests, &mut message, &mut handles) else { return };
    let close = |handles: &[Handle]| {
        for &handle in handles {
            let _ = taaj::close(handle);
        }
    };
    let reply = |text: &[u8], handles: &[Handle]| {
        let _ = taaj::send(requests, text, handles);
    };

    // One handle for the image, one for each flag - no more, no fewer.
    let run = Run::decode(&message[..n]).filter(|run| {
        let flags = u64::from(run.flags);
        flags & !(STDOUT | SERVICES | STDIN | STDERR | SCREEN) == 0
            && count == usize::from(run.kind == IMAGE) + (flags & HANDLES).count_ones() as usize
    });
    let Some(run) = run else {
        close(&handles[..count]);
        return reply(b"error: unknown request", &[]);
    };
    let (image, passed) = match run.kind {
        IMAGE => (Some(handles[0]), &handles[1..count]),
        _ => (None, &handles[..count]),
    };
    if run.kind == BUILT_IN && !RUNNABLE.contains(&run.name) {
        close(passed);
        return reply(b"error: no such program, on the disk or built in", &[]);
    }

    // The arguments, waiting in a channel of their own - and the
    // directory the program starts in, in another. Nothing to say
    // (no arguments; the root) means no channel.
    let waiting = |text: &[u8]| -> Result<Option<Handle>, Error> {
        if text.is_empty() {
            return Ok(None);
        }
        let (tx, rx) = taaj::channel_create()?;
        let sent = taaj::send(tx, text, &[]);
        let _ = taaj::close(tx);
        sent.map(|()| Some(rx)).inspect_err(|_| close(&[rx]))
    };
    let arguments = waiting(run.arguments);
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
            return reply(text.as_bytes(), &[]);
        }
    };

    // The startup handles, in the order of the flags' bits: stdout,
    // arguments, services, directory, stdin, stderr. What the shell passed is cut
    // down to the one thing it is for: an output to send on, an input
    // to receive from.
    let mut given = [(0u64, 0u32); 6];
    let mut length = 0;
    let mut flags = u64::from(run.flags);
    let mut passed = passed.iter().copied();
    for flag in [STDOUT, ARGUMENTS, SERVICES, DIRECTORY, STDIN, STDERR] {
        let entry = match flag {
            ARGUMENTS => arguments.map(|rx| (rx, RECEIVE)),
            DIRECTORY => directory.map(|rx| (rx, RECEIVE)),
            STDIN if flags & flag != 0 => passed.next().map(|handle| (handle, RECEIVE)),
            _ if flags & flag != 0 => passed.next().map(|handle| (handle, SEND)),
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
            println!("[init] started {} for the shell{}", run.name, if image.is_some() { ", from a file" } else { "" });
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
}
