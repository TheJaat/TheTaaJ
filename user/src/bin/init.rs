//! init - the first process, and the supervisor of all the others.
//!
//! The kernel starts only this, with five startup handles:
//!   1  the launcher: the right to start programs
//!   2  the keyboard's interrupt line
//!   3  I/O port 0x60
//!   4  I/O port 0x64
//!   5  a channel with a message per device the kernel found - for now
//!      the framebuffer: its description and its memory object
//! (all with every right but the launcher's, so init can share them out.)
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

use abi::FramebufferInfo;
use taaj::rights::{DUPLICATE, READ, RECEIVE, SEND, TRANSFER, WRITE};
use taaj::{print, println, Error, Handle};

taaj::program!(main);

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
fn start(launcher: Handle, name: &str, grants: &[&Grant]) -> Result<Handle, Error> {
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
    taaj::spawn(launcher, name, copies).inspect_err(|_| {
        // Not started: the copies are still ours.
        for &(copy, _) in copies {
            let _ = taaj::close(copy);
        }
    })
}

fn main() -> i64 {
    let (Some(launcher), Some(interrupt), Some(data_port), Some(status_port), Some(devices)) = (
        taaj::startup_handle(0),
        taaj::startup_handle(1),
        taaj::startup_handle(2),
        taaj::startup_handle(3),
        taaj::startup_handle(4),
    ) else {
        println!("[init] started without the kernel's handles");
        return 1;
    };
    let Ok((keys_out, keys_in)) = taaj::channel_create() else { return 2 };

    // The screen, if the kernel found one: its description and memory.
    let mut message = [0u8; 64];
    let mut fb_memory = [0u64; 1];
    let framebuffer = match taaj::receive(devices, &mut message, &mut fb_memory) {
        Ok((n, 1)) if message[..n].starts_with(b"framebuffer") => {
            FramebufferInfo::from_bytes(&message[b"framebuffer".len()..n])
        }
        _ => None,
    };
    let Ok((display_tx, display_rx)) = taaj::channel_create() else { return 2 };

    // Everything a service may be given, and the rights it gets.
    let grants = [
        Grant { handle: interrupt, rights: RECEIVE },    // 0
        Grant { handle: data_port, rights: 0 },          // 1
        Grant { handle: status_port, rights: 0 },        // 2
        Grant { handle: keys_out, rights: SEND },        // 3
        Grant { handle: keys_in, rights: RECEIVE },      // 4
        Grant { handle: fb_memory[0], rights: READ | WRITE }, // 5
        Grant { handle: display_rx, rights: RECEIVE },   // 6
    ];
    debug_assert!(grants.iter().all(|g| g.rights & DUPLICATE == 0));

    let mut services = [
        Service { name: "shell", grants: &[4], process: 0, restarts: [0; MAX_RESTARTS], restart_count: 0, given_up: false },
        Service { name: "kbd", grants: &[0, 1, 2, 3], process: 0, restarts: [0; MAX_RESTARTS], restart_count: 0, given_up: false },
        Service {
            name: "display",
            grants: &[5, 6],
            process: 0,
            restarts: [0; MAX_RESTARTS],
            restart_count: 0,
            given_up: framebuffer.is_none(),
        },
    ];
    let launch = |service: &Service| {
        let mut list = [&grants[0]; 8];
        for (n, &g) in service.grants.iter().enumerate() {
            list[n] = &grants[g];
        }
        let process = start(launcher, service.name, &list[..service.grants.len()])?;
        // Each new display needs to be told what it is drawing on.
        if let (Some(fb), "display") = (framebuffer, service.name) {
            taaj::send(display_tx, &fb.to_bytes(), &[])?;
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
    print!("[init] started:");
    for service in services.iter().filter(|s| !s.given_up) {
        print!(" {}", service.name);
    }
    println!("; supervising");

    loop {
        // Watch every service still running.
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
        if count == 0 {
            println!("[init] nothing left to supervise");
            // init must not exit; there is nothing better to do than wait.
            loop {
                taaj::sleep(100 * 60);
            }
        }
        let (index, code) = match taaj::wait_any(&watched[..count]) {
            Ok(result) => result,
            Err(_) => return 3,
        };
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
