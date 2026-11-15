//! The shell: reads a line from the keyboard, runs it as a command.
//!
//! Startup handles, from init:
//!   1  the channel from the keyboard driver (receive only)
//!   2  the channel to the display (send only) - may be absent
//!   3  the request channel to init: "start this program" (abi::init)
//!   4  the system handle: looking at memory and processes, not touching
//!   5  the request channel to the file service (send only)
//!   6  the request channel to the disk driver: reads only - files on
//!      the disk are the file service's to change
//!
//! It has no launcher and no hardware. To run a program it asks init,
//! and sends along what the program is to have: a copy of its channel to
//! the screen, a copy of its channel to the file service. init replies
//! with a handle to the new process, which lets the shell wait for it
//! to end - and end it: Ctrl+C stops the program in the foreground.
//! Keys typed meanwhile are kept, and read as the next line.
//!
//! A word that is not one of the shell's own commands is looked for as
//! a file: `ls`, `cat`, `write`, `append` and `rm` are programs on the
//! disk, like any other. The shell itself never changes a file - it
//! only reads the ones it runs.
//!
//! Output goes to the screen and to the kernel's debug log (the serial
//! line), which is what the boot tests read.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::cell::RefCell;
use core::fmt::Write;

use abi::ProcessEntry;
use taaj::disk::Disk;
use taaj::files::Files;
use taaj::{println, Buffer, Error, Handle};

taaj::program!(main);

const LINE_MAX: usize = 256;
const PROMPT: &[u8] = b"taaj> ";
const CTRL_C: &[u8] = b"\x03";
/// Keys kept while a program runs; more than this are dropped.
const TYPED_AHEAD_MAX: usize = 256;

struct Shell {
    keys: Handle,
    /// Keys that arrived while a program was running, oldest first.
    typed_ahead: RefCell<VecDeque<Vec<u8>>>,
    screen: Option<Handle>,
    init: Option<Handle>,
    system: Option<Handle>,
    files: Option<Files>,
    disk: Option<Disk>,
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
                  \x20 run NAME ARGS    run a program: a file, or hello, peek\n\
                  \x20              (Ctrl+C stops it)\n\
                  \x20 NAME ARGS    the same, for a file. On the disk:\n\
                  \x20   ls                 list the files\n\
                  \x20   cat FILE...        show files\n\
                  \x20   write FILE TEXT    make FILE hold TEXT\n\
                  \x20   append FILE TEXT   add TEXT to the end of FILE\n\
                  \x20   rm FILE...         remove files\n\
                  \x20 disk         the disk's size\n\
                  \x20 disk read N        show sector N\n\
                  \x20 disk write N TEXT  put TEXT in sector N\n",
            ),
            "echo" => say!(self, "{rest}\n"),
            "clear" => self.screen_only(b"\x0c"),
            "uptime" => self.uptime(),
            "mem" => self.mem(),
            "ps" => self.ps(),
            "run" if !rest.is_empty() => {
                let (name, arguments) = rest.split_once(' ').unwrap_or((rest, ""));
                self.run_program(name, arguments.trim(), true);
            }
            "run" => self.write(b"usage: run NAME ARGS\n"),
            "disk" => self.disk(rest),
            // Not the shell's own: a program in a file, if there is one.
            _ => self.run_program(command, rest, false),
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

    fn disk(&self, arguments: &str) {
        let Some(disk) = &self.disk else {
            return self.write(b"no disk service\n");
        };
        let mut words = arguments.splitn(3, ' ');
        let (action, sector, text) = (words.next().unwrap_or(""), words.next().map(str::parse::<u64>), words.next());
        match (action, sector, text) {
            ("", None, None) => match disk.info() {
                Ok((sectors, read_only)) => say!(
                    self,
                    "disk: {sectors} sectors of 512 bytes ({} KiB){}\n",
                    sectors / 2,
                    if read_only { ", read-only" } else { "" }
                ),
                Err(error) => say!(self, "disk: {}\n", error.describe()),
            },
            ("read", Some(Ok(sector)), None) => {
                let mut data = [0u8; abi::block::SECTOR];
                match disk.read(sector, &mut data) {
                    Ok(()) => self.dump(&data),
                    Err(error) => say!(self, "disk: sector {sector}: {}\n", error.describe()),
                }
            }
            ("write", Some(Ok(sector)), Some(text)) => {
                let mut data = [0u8; abi::block::SECTOR];
                let n = text.len().min(data.len());
                data[..n].copy_from_slice(&text.as_bytes()[..n]);
                match disk.write(sector, &data) {
                    Ok(()) => say!(self, "disk: wrote sector {sector}\n"),
                    Err(error) => say!(self, "disk: sector {sector}: {}\n", error.describe()),
                }
            }
            _ => self.write(b"usage: disk, disk read N, disk write N TEXT\n"),
        }
    }

    /// A sector as hex and text, 16 bytes a line, up to the last line
    /// that is not all zeros.
    fn dump(&self, data: &[u8]) {
        let lines = data.chunks(16).rposition(|line| line.iter().any(|&b| b != 0)).map_or(1, |n| n + 1);
        for (n, line) in data.chunks(16).take(lines).enumerate() {
            let mut text = Buffer::<96>::new();
            let _ = write!(text, "{:04x} ", n * 16);
            for byte in line {
                let _ = write!(text, " {byte:02x}");
            }
            let _ = write!(text, "  ");
            for &byte in line {
                let _ = text.write_char(if (0x20..0x7F).contains(&byte) { byte as char } else { '.' });
            }
            let _ = writeln!(text);
            self.write(text.as_bytes());
        }
        if lines < data.len() / 16 {
            self.write(b"(the rest is zeros)\n");
        }
    }

    /// The program file `name` from the disk, in a new memory object -
    /// how a program gets from the file service to the kernel, which
    /// knows nothing of files. Ok(None) if there is no such file.
    fn program_image(&self, name: &str) -> Result<Option<Handle>, &'static str> {
        let Some(files) = &self.files else { return Ok(None) };
        let size = match files.size(name) {
            Ok(size) => size,
            Err(_) => return Ok(None), // no such file (or no file service): maybe built in
        };
        if size == 0 || size > abi::PROGRAM_IMAGE_MAX {
            return Err("is empty, or too big to be a program");
        }
        let memory = taaj::memory_create(size).map_err(|_| "does not fit in memory")?;
        let filled = taaj::memory_map(memory, true).map_err(|_| "does not fit in memory").and_then(|base| {
            let mut at = 0usize;
            let read = files.read(name, |piece| {
                // Never past the object, even if the file grew meanwhile.
                let n = piece.len().min(size as usize - at);
                // SAFETY: inside the mapping, `size` bytes long.
                unsafe { base.add(at).copy_from_nonoverlapping(piece.as_ptr(), n) };
                at += n;
            });
            let _ = taaj::memory_unmap(base);
            read.map(|_| ()).map_err(|error| error.describe())
        });
        match filled {
            Ok(()) => Ok(Some(memory)),
            Err(why) => {
                let _ = taaj::close(memory);
                Err(why)
            }
        }
    }

    /// A copy of one of the shell's own handles, to give to a program:
    /// good for sending on, and for handing over, nothing else.
    fn delegate(handle: Option<Handle>) -> Option<Handle> {
        taaj::duplicate(handle?, taaj::rights::SEND | taaj::rights::TRANSFER).ok()
    }

    /// Asks init to start `name`, then waits for it - a foreground job.
    /// A file of that name is read and sent along; otherwise, for `run`
    /// (`explicit`), init looks among the programs built into the kernel
    /// image. The program gets the screen and the files, as the shell
    /// has them, and `arguments`.
    ///
    /// `run NAME` says how the program ended; a program run by its name
    /// alone is a command, and says nothing unless it failed.
    fn run_program(&self, name: &str, arguments: &str, explicit: bool) {
        let image = match self.program_image(name) {
            Ok(None) if !explicit => return say!(self, "unknown command: {name} (try help)\n"),
            Ok(image) => image,
            Err(why) => return say!(self, "run: {name} {why}\n"),
        };
        let mut handles = [0u64; 3];
        let mut count = 0;
        let mut flags = 0;
        let offered = [
            (0, image),
            (abi::startup::STDOUT, Self::delegate(self.screen)),
            (abi::startup::FILES, Self::delegate(self.files.as_ref().map(Files::channel))),
        ];
        for (flag, handle) in offered {
            if let Some(handle) = handle {
                handles[count] = handle;
                count += 1;
                flags |= flag as u8;
            }
        }
        let handles = &handles[..count];
        let request = abi::init::Run {
            kind: if image.is_some() { abi::init::IMAGE } else { abi::init::BUILT_IN },
            flags,
            name,
            arguments: arguments.as_bytes(),
        };
        let mut message = [0u8; 3 + 2 * LINE_MAX];
        let sent = match (self.init, request.encode(&mut message)) {
            (Some(init), Some(length)) => taaj::send(init, &message[..length], handles).map(|()| init),
            _ => Err(Error::InvalidArgument),
        };
        let Ok(init) = sent else {
            for &handle in handles {
                let _ = taaj::close(handle); // not sent: still ours
            }
            return self.write(b"cannot reach init: cannot run programs\n");
        };
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
        match self.wait_for(process) {
            Ok((_, abi::KILLED)) => say!(self, "{name} was killed by the kernel (it broke a rule)\n"),
            Ok((_, abi::STOPPED)) => say!(self, "{name} was stopped\n"),
            Ok((_, 0)) if !explicit => {}
            Ok((_, code)) => say!(self, "{name} exited with code {code}\n"),
            Err(error) => say!(self, "lost track of {name}: {error:?}\n"),
        }
        let _ = taaj::close(process);
    }
}

impl Shell {
    /// Waits for the foreground program to end, watching the keyboard
    /// meanwhile: Ctrl+C asks the kernel to end the program - and the
    /// wait goes on, until it has. Any other key is kept for later.
    fn wait_for(&self, process: Handle) -> Result<(usize, i64), Error> {
        let mut input = [0u8; 16];
        loop {
            // Whichever comes first: the program's end, or a key.
            if taaj::object_wait(&[process, self.keys])? == 0 {
                return taaj::wait_any(&[process]);
            }
            match taaj::receive(self.keys, &mut input, &mut []) {
                Ok((n, _)) if &input[..n] == CTRL_C => {
                    self.write(b"^C\n");
                    if let Err(error) = taaj::kill(process) {
                        say!(self, "cannot stop it: {error:?}\n");
                    }
                }
                Ok((n, _)) => {
                    let mut typed = self.typed_ahead.borrow_mut();
                    if typed.len() < TYPED_AHEAD_MAX {
                        typed.push_back(input[..n].to_vec());
                    }
                }
                // No keyboard: nothing to watch but the program.
                Err(_) => return taaj::wait_any(&[process]),
            }
        }
    }

    /// The next key: one typed while a program ran, if any are left,
    /// else one from the keyboard driver - waiting for it.
    fn next_key(&self, input: &mut [u8; 16]) -> Result<usize, Error> {
        if let Some(key) = self.typed_ahead.borrow_mut().pop_front() {
            input[..key.len()].copy_from_slice(&key);
            return Ok(key.len());
        }
        taaj::receive(self.keys, input, &mut []).map(|(n, _)| n)
    }
}

fn main() -> i64 {
    let shell = Shell {
        keys: taaj::bootstrap(),
        typed_ahead: RefCell::new(VecDeque::new()),
        screen: taaj::startup_handle(1),
        init: taaj::startup_handle(2),
        system: taaj::startup_handle(3),
        files: taaj::startup_handle(4).map(Files::new),
        disk: taaj::startup_handle(5).map(Disk::new),
    };
    println!("[shell] ready - type a command (try help)");
    shell.screen_only(b"Welcome to TheTaaJ. Type help for the commands.\n\n");
    shell.screen_only(PROMPT);

    let mut line = [0u8; LINE_MAX];
    let mut length = 0;
    let mut input = [0u8; 16];
    loop {
        let n = match shell.next_key(&mut input) {
            Ok(n) => n,
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
            CTRL_C => {
                // Abandon the line.
                shell.write(b"^C\n");
                shell.screen_only(PROMPT);
                length = 0;
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
