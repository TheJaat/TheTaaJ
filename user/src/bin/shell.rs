//! The shell: reads a line from the keyboard, runs it as a command.
//!
//! Startup handles, from init:
//!   1  the channel from the keyboard driver (receive only)
//!   2  the channel to the display (send only) - may be absent
//!   3  the request channel to init: "run NAME"
//!   4  the system handle: looking at memory and processes, not touching
//!
//! It has no launcher and no hardware. To run a program it asks init,
//! which decides what may run, and replies with a handle to the new
//! process - all the shell can do with it is wait for it to end.
//!
//! Output goes to the screen and to the kernel's debug log (the serial
//! line), which is what the boot tests read.

#![no_std]
#![no_main]

use core::fmt::Write;

use abi::ProcessEntry;
use taaj::{println, Buffer, Error, Handle};

taaj::program!(main);

const LINE_MAX: usize = 256;
const PROMPT: &[u8] = b"taaj> ";

struct Shell {
    screen: Option<Handle>,
    init: Option<Handle>,
    system: Option<Handle>,
}

/// Formats into a buffer, then writes it at once to screen and log.
macro_rules! say {
    ($shell:expr, $($arg:tt)*) => {{
        let mut text = Buffer::<512>::new();
        let _ = write!(text, $($arg)*);
        $shell.write(text.as_bytes());
    }};
}

impl Shell {
    fn write(&self, bytes: &[u8]) {
        let _ = taaj::write(bytes);
        self.screen_only(bytes);
    }

    fn screen_only(&self, bytes: &[u8]) {
        if let Some(screen) = self.screen {
            taaj::write_to_display(screen, bytes);
        }
    }

    fn run(&self, line: &str) {
        let line = line.trim();
        let (command, rest) = line.split_once(' ').unwrap_or((line, ""));
        let rest = rest.trim();
        match command {
            "" => {}
            "help" => self.write(
                b"commands:\n\
                  \x20 help         this list\n\
                  \x20 echo TEXT    print TEXT\n\
                  \x20 clear        clear the screen\n\
                  \x20 uptime       time since boot\n\
                  \x20 mem          memory in use\n\
                  \x20 ps           processes\n\
                  \x20 run NAME     run a program (hello, peek)\n",
            ),
            "echo" => say!(self, "{rest}\n"),
            "clear" => self.screen_only(b"\x0c"),
            "uptime" => self.uptime(),
            "mem" => self.mem(),
            "ps" => self.ps(),
            "run" if !rest.is_empty() => self.run_program(rest),
            "run" => self.write(b"usage: run NAME\n"),
            _ => say!(self, "unknown command: {command} (try help)\n"),
        }
    }

    fn system_info(&self) -> Option<abi::SystemInfo> {
        match self.system.map(taaj::system_info) {
            Some(Ok(info)) => Some(info),
            Some(Err(error)) => {
                say!(self, "cannot read the system's state: {error:?}\n");
                None
            }
            None => {
                self.write(b"no system handle: this shell may not look\n");
                None
            }
        }
    }

    fn uptime(&self) {
        let Some(info) = self.system_info() else { return };
        let hundredths = info.ticks * 100 / info.ticks_per_second.max(1);
        say!(self, "up {}.{:02} s ({} ticks)\n", hundredths / 100, hundredths % 100, info.ticks);
    }

    fn mem(&self) {
        let Some(info) = self.system_info() else { return };
        let mib = |frames: u64| frames * 4096 / (1024 * 1024);
        let used = info.frames_total - info.frames_free;
        say!(
            self,
            "memory: {} MiB used, {} MiB free, {} MiB in all ({}% used)\n\
             kernel heap: {} KiB used of {} KiB\n",
            mib(used),
            mib(info.frames_free),
            mib(info.frames_total),
            used * 100 / info.frames_total.max(1),
            info.heap_used / 1024,
            info.heap_total / 1024,
        );
    }

    fn ps(&self) {
        let Some(system) = self.system else {
            self.write(b"no system handle: this shell may not look\n");
            return;
        };
        let mut entries = [ProcessEntry::default(); 32];
        let total = match taaj::process_list(system, &mut entries) {
            Ok(total) => total,
            Err(error) => return say!(self, "cannot list processes: {error:?}\n"),
        };
        self.write(b"  PID  NAME        STATE\n");
        for entry in &entries[..total.min(entries.len())] {
            match entry.exit_code {
                None => say!(self, "{:>5}  {:<10}  running\n", entry.pid, entry.name()),
                Some(code) => say!(self, "{:>5}  {:<10}  ended ({code})\n", entry.pid, entry.name()),
            }
        }
        if let Some(info) = self.system_info() {
            say!(self, "{} processes, {} threads, {} channels\n", info.processes, info.threads, info.channels);
        }
    }

    /// Asks init to start `name`, then waits for it - a foreground job.
    fn run_program(&self, name: &str) {
        let Some(init) = self.init else {
            self.write(b"no channel to init: cannot run programs\n");
            return;
        };
        let mut request = Buffer::<64>::new();
        if write!(request, "run {name}").is_err() || taaj::send(init, request.as_bytes(), &[]).is_err() {
            self.write(b"cannot reach init\n");
            return;
        }
        let mut reply = [0u8; 64];
        let mut handle = [0u64; 1];
        let process = match taaj::receive(init, &mut reply, &mut handle) {
            Ok((2, 1)) if &reply[..2] == b"ok" => handle[0],
            Ok((n, _)) => {
                self.write(&reply[..n]);
                self.write(b"\n");
                return;
            }
            Err(error) => return say!(self, "no answer from init: {error:?}\n"),
        };
        match taaj::wait_any(&[process]) {
            Ok((_, abi::KILLED)) => say!(self, "{name} was killed by the kernel (it broke a rule)\n"),
            Ok((_, code)) => say!(self, "{name} exited with code {code}\n"),
            Err(error) => say!(self, "lost track of {name}: {error:?}\n"),
        }
        let _ = taaj::close(process);
    }
}

fn main() -> i64 {
    let keys = taaj::bootstrap();
    let shell =
        Shell { screen: taaj::startup_handle(1), init: taaj::startup_handle(2), system: taaj::startup_handle(3) };
    println!("[shell] ready - type a command (try help)");
    shell.screen_only(b"Welcome to TheTaaJ. Type help for the commands.\n\n");
    shell.screen_only(PROMPT);

    let mut line = [0u8; LINE_MAX];
    let mut length = 0;
    let mut input = [0u8; 16];
    loop {
        let n = match taaj::receive(keys, &mut input, &mut []) {
            Ok((n, _)) => n,
            Err(Error::PeerClosed) => {
                println!("[shell] the keyboard driver is gone");
                return 0;
            }
            Err(_) => return 1,
        };
        // The driver sends one key per message: a character (as UTF-8),
        // a control code, or an escape sequence.
        match &input[..n] {
            b"\n" => {
                shell.write(b"\n");
                let text = core::str::from_utf8(&line[..length]).unwrap_or("(not UTF-8)");
                println!("[shell] line: {text:?} (process {})", taaj::process_id());
                shell.run(text);
                println!("[shell] ran {text:?}");
                shell.screen_only(PROMPT);
                length = 0;
            }
            b"\x08" => {
                if length > 0 {
                    // Drop the whole last character, however many bytes.
                    length -= 1;
                    while length > 0 && line[length] & 0xC0 == 0x80 {
                        length -= 1;
                    }
                    shell.write(b"\x08 \x08"); // back, blank, back
                }
            }
            [first, ..] if *first < 0x20 => {} // other control keys, escapes: not yet
            text if length + text.len() <= LINE_MAX => {
                line[length..length + text.len()].copy_from_slice(text);
                length += text.len();
                shell.write(text);
            }
            _ => {} // line full
        }
    }
}
