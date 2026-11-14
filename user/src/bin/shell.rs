//! The shell: reads a line from the keyboard, runs it as a command.
//!
//! Startup handles, from init:
//!   1  the channel from the keyboard driver (receive only)
//!   2  the channel to the display (send only) - may be absent
//!   3  the request channel to init: "run NAME"
//!   4  the system handle: looking at memory and processes, not touching
//!   5  the request channel to the file service (send only)
//!   6  the request channel to the disk driver (send only)
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
use taaj::disk::Disk;
use taaj::files::Files;
use taaj::{println, Buffer, Error, Handle};

taaj::program!(main);

const LINE_MAX: usize = 256;
const PROMPT: &[u8] = b"taaj> ";

struct Shell {
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
                  \x20 run NAME     run a program (hello, peek)\n\
                  \x20 ls           list the files\n\
                  \x20 cat FILE     show a file\n\
                  \x20 write FILE TEXT    make FILE hold TEXT\n\
                  \x20 append FILE TEXT   add TEXT to the end of FILE\n\
                  \x20 rm FILE      remove a file\n\
                  \x20 disk         the disk's size\n\
                  \x20 disk read N        show sector N\n\
                  \x20 disk write N TEXT  put TEXT in sector N\n",
            ),
            "echo" => say!(self, "{rest}\n"),
            "clear" => self.screen_only(b"\x0c"),
            "uptime" => self.uptime(),
            "mem" => self.mem(),
            "ps" => self.ps(),
            "run" if !rest.is_empty() => self.run_program(rest),
            "run" => self.write(b"usage: run NAME\n"),
            "ls" => self.ls(),
            "cat" if !rest.is_empty() => self.cat(rest),
            "write" | "append" => match rest.split_once(' ') {
                Some((file, text)) => self.write_file(file, text, command == "append"),
                None => say!(self, "usage: {command} FILE TEXT\n"),
            },
            "rm" if !rest.is_empty() => self.rm(rest),
            "cat" | "rm" => say!(self, "usage: {command} FILE\n"),
            "disk" => self.disk(rest),
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

    fn files(&self) -> Option<&Files> {
        if self.files.is_none() {
            self.write(b"no file service: this shell has no files\n");
        }
        self.files.as_ref()
    }

    fn ls(&self) {
        let Some(files) = self.files() else { return };
        let mut bytes = 0;
        let listed = files.list(|name, size| {
            say!(self, "{size:>8}  {name}\n");
            bytes += size;
        });
        match listed {
            Ok(0) => self.write(b"no files\n"),
            Ok(count) => say!(self, "{count} file{}, {bytes} bytes\n", if count == 1 { "" } else { "s" }),
            Err(error) => say!(self, "ls: {}\n", error.describe()),
        }
    }

    fn cat(&self, name: &str) {
        let Some(files) = self.files() else { return };
        let mut last = b'\n';
        match files.read(name, |piece| {
            self.write(piece);
            last = piece[piece.len() - 1];
        }) {
            // Keep the prompt on a line of its own.
            Ok(size) if size > 0 && last != b'\n' => self.write(b"\n"),
            Ok(_) => {}
            Err(error) => say!(self, "cat: {name}: {}\n", error.describe()),
        }
    }

    /// `write` and `append`: the text, as a line.
    fn write_file(&self, name: &str, text: &str, append: bool) {
        let Some(files) = self.files() else { return };
        let mut line = Buffer::<{ LINE_MAX + 1 }>::new();
        let _ = writeln!(line, "{}", text.trim());
        let result = if append { files.append(name, line.as_bytes()) } else { files.write(name, line.as_bytes()) };
        match result {
            Ok(size) => say!(self, "{name}: {size} bytes\n"),
            Err(error) => say!(self, "{}: {name}: {}\n", if append { "append" } else { "write" }, error.describe()),
        }
    }

    fn rm(&self, name: &str) {
        let Some(files) = self.files() else { return };
        if let Err(error) = files.remove(name) {
            say!(self, "rm: {name}: {}\n", error.describe());
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
    let shell = Shell {
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
