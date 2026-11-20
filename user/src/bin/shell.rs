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
//! a file: `ls`, `cat`, `write`, `append`, `rm`, `mv` and `cp` are
//! programs on the disk, like any other. The shell itself never changes a file - it
//! only reads the ones it runs.
//!
//! The shell is also where the current directory lives: `cd` changes
//! it, `pwd` shows it, the prompt shows it. It is a path the shell
//! remembers and nothing else - the file service is never told. A
//! program that is given the files is told the directory it was started
//! in, and resolves the names it is given against that (taaj::files).
//!
//! Output goes to the screen and to the kernel's debug log (the serial
//! line), which is what the boot tests read.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;
use core::fmt::Write;

use abi::ProcessEntry;
use taaj::disk::Disk;
use taaj::files::{path, FileError, Files, Kind};
use taaj::{println, Buffer, Error, Handle};

taaj::program!(main);

const LINE_MAX: usize = 256;
const CTRL_C: &[u8] = b"\x03";
/// Keys kept while a program runs; more than this are dropped.
const TYPED_AHEAD_MAX: usize = 256;

/// The programs that are given the file service when run by name:
/// the ones whose whole job is files - and each only as much of it as
/// that job takes. These read; they get the channel that cannot change
/// anything.
const LOOK_PROGRAMS: &[&str] = &["ls", "cat"];
/// And these change files.
const CHANGE_PROGRAMS: &[&str] = &["write", "append", "rm", "mkdir", "mv", "cp"];

/// What a program is to be given of the files. Everything not on the
/// lists above gets the screen and its arguments; `with files COMMAND`
/// and `with readonly COMMAND` add the files, by the user's own
/// decision. The shell holds the authority, so the shell decides who
/// shares it; a list in a file would let any program that can write
/// files grant itself more.
#[derive(Clone, Copy, PartialEq)]
enum Grant {
    /// What the lists say.
    Usual,
    /// The files, to read and to change.
    Files,
    /// The files, to read.
    ReadOnly,
}

struct Shell {
    keys: Handle,
    /// Keys that arrived while a program was running, oldest first.
    typed_ahead: RefCell<VecDeque<Vec<u8>>>,
    screen: Option<Handle>,
    init: Option<Handle>,
    system: Option<Handle>,
    files: Option<Files>,
    /// The file service again, by its read-only channel: what the shell
    /// itself uses - it only ever reads the programs it runs.
    look: Option<Files>,
    disk: Option<Disk>,
    /// The current directory: a resolved path from the root, "" for the
    /// root. Only a name - whether it is still there is found out when
    /// it is used. (A RefCell because `run` takes `&self`: the borrow
    /// is checked when the program runs instead of when it is built.)
    directory: RefCell<String>,
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

    /// The prompt says where the shell is: "taaj:/docs> ". On the
    /// screen only - the log has the line that follows it.
    fn prompt(&self) {
        let mut text = Buffer::<{ LINE_MAX + 16 }>::new();
        let _ = write!(text, "taaj:{}> ", path::display(&self.directory.borrow()));
        self.screen_only(text.as_bytes());
    }

    /// `cd DIR`: makes DIR the current directory, if it is one. `cd`
    /// alone goes to the root. The new directory is worked out from the
    /// text ("..", ".", a leading '/'), and then the file service is
    /// asked one thing: is that a directory?
    fn cd(&self, argument: &str) {
        let wanted = path::resolve(&self.directory.borrow(), argument);
        let Some(files) = self.look.as_ref().or(self.files.as_ref()) else {
            return self.write(b"cd: no file service\n");
        };
        match files.stat(&wanted) {
            Ok((Kind::Directory, _)) => *self.directory.borrow_mut() = wanted,
            Ok((Kind::File, _)) => say!(self, "cd: {argument}: not a directory\n"),
            Err(error) => say!(self, "cd: {argument}: {}\n", error.describe()),
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
                  \x20 cd [DIR], pwd    go to DIR (to the root); say where this is\n\
                  \x20 time COMMAND     run COMMAND, and say how long it took\n\
                  \x20 run NAME ARGS    run a program: a file, or hello, peek\n\
                  \x20              (Ctrl+C stops it)\n\
                  \x20 NAME ARGS    the same, for a file in the root (./NAME, or a\n\
                  \x20              path with a /, for one elsewhere). On the disk:\n\
                  \x20   ls [DIR...]        list a directory (this one)\n\
                  \x20   cat FILE...        show files\n\
                  \x20   write FILE TEXT, append FILE TEXT   set, or add to, FILE\n\
                  \x20   mv FROM TO, cp FROM TO   move or rename; copy a file\n\
                  \x20   mkdir DIR...       make directories\n\
                  \x20   rm NAME...         remove files, empty directories\n\
                  \x20   (a name is a path from here, ../plan - or /from/the/root)\n\
                  \x20   given              say what it was given\n\
                  \x20 with files COMMAND      run COMMAND with the files too\n\
                  \x20 with readonly COMMAND   the same, but only to read them\n\
                  \x20              (of the programs above, all but given\n\
                  \x20              get the files anyway)\n\
                  \x20 fs stall SECONDS   a test: the file service goes silent\n\
                  \x20 disk stall SECONDS, display stall SECONDS   the same,\n\
                  \x20              for the disk driver and the display\n\
                  \x20 (F11 hangs the keyboard driver, F12 crashes it)\n\
                  \x20 disk         the disk's size\n\
                  \x20 disk read N, disk write N TEXT   show sector N, put TEXT in it\n\
                  \x20 disk check        test the disk driver\n",
            ),
            "echo" => say!(self, "{rest}\n"),
            "cd" if rest.contains(' ') => self.write(b"usage: cd [DIR]\n"),
            "cd" => self.cd(if rest.is_empty() { "/" } else { rest }),
            "pwd" => say!(self, "{}\n", path::display(&self.directory.borrow())),
            "clear" => self.screen_only(b"\x0c"),
            "uptime" => self.uptime(),
            "mem" => self.mem(),
            "ps" => self.ps(),
            "time" if !rest.is_empty() => {
                // Ticks are 10 ms apart: good for "is it slow", no finer.
                let start = taaj::ticks();
                self.run(rest);
                let hundredths = taaj::ticks() - start;
                say!(self, "took {}.{:02} s\n", hundredths / 100, hundredths % 100);
            }
            "time" => self.write(b"usage: time COMMAND\n"),
            "run" if !rest.is_empty() => {
                let (name, arguments) = rest.split_once(' ').unwrap_or((rest, ""));
                self.run_program(name, arguments.trim(), true, Grant::Usual);
            }
            "run" => self.write(b"usage: run NAME ARGS\n"),
            "with" => {
                let (what, command) = rest.split_once(' ').unwrap_or((rest, ""));
                let grant = match what {
                    "files" => Some(Grant::Files),
                    "readonly" => Some(Grant::ReadOnly),
                    _ => None,
                };
                match (grant, command.trim()) {
                    (Some(grant), command) if !command.is_empty() => {
                        let (name, arguments) = command.split_once(' ').unwrap_or((command, ""));
                        self.run_program(name, arguments.trim(), false, grant);
                    }
                    _ => self.write(b"usage: with files COMMAND, with readonly COMMAND\n"),
                }
            }
            "fs" => self.fs(rest),
            "display" => match (rest.strip_prefix("stall ").and_then(|n| n.trim().parse::<u64>().ok()), self.screen) {
                (Some(seconds @ 1..=30), Some(screen)) => {
                    say!(self, "display: the screen will show nothing for {seconds} s\n");
                    let mut message = [0u8; 9];
                    message[0] = abi::display::STALL;
                    message[1..].copy_from_slice(&(seconds * 100).to_le_bytes());
                    let _ = taaj::send(screen, &message, &[]);
                }
                (Some(1..=30), None) => self.write(b"no display\n"),
                _ => self.write(b"usage: display stall SECONDS (1 to 30)\n"),
            },
            "disk" => self.disk(rest),
            // Not the shell's own: a program in a file, if there is one.
            _ => self.run_program(command, rest, false, Grant::Usual),
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

    /// `fs stall SECONDS`: makes the file service answer nothing for a
    /// while - to see what its clients do then, this shell among them.
    fn fs(&self, arguments: &str) {
        let seconds = arguments.strip_prefix("stall ").and_then(|n| n.trim().parse::<u64>().ok());
        match (seconds, &self.files) {
            (Some(seconds @ 1..=30), Some(files)) => match files.stall(seconds * 100) {
                Ok(()) => say!(self, "fs: the file service will answer nothing for {seconds} s\n"),
                Err(error) => say!(self, "fs: {}\n", error.describe()),
            },
            (Some(1..=30), None) => self.write(b"no file service\n"),
            _ => self.write(b"usage: fs stall SECONDS (1 to 30)\n"),
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
            ("stall", Some(Ok(seconds @ 1..=30)), None) => {
                let mut request = [0u8; abi::block::REQUEST_SIZE];
                request[0] = abi::block::STALL;
                request[1..].copy_from_slice(&(seconds * 100).to_le_bytes());
                match disk.raw(&request, None) {
                    Ok(()) => say!(self, "disk: the driver will answer nothing for {seconds} s\n"),
                    Err(error) => say!(self, "disk: {}\n", error.describe()),
                }
            }
            ("check", None, None) => match self.disk_check(disk) {
                Ok(checks) => say!(self, "disk check: all {checks} checks passed\n"),
                Err(failed) => say!(self, "disk check: FAILED - {failed}\n"),
            },
            _ => self.write(b"usage: disk, disk read N, disk write N TEXT, disk check, disk stall SECONDS\n"),
        }
    }

    /// Tests the driver's many-sector requests through this shell's own
    /// (read-only) channel: that they return what single reads do, and
    /// that every wrong request is refused - by an answer, with the
    /// driver still running afterwards. Returns how many checks passed,
    /// or which one failed.
    fn disk_check(&self, disk: &Disk) -> Result<u32, &'static str> {
        use abi::block::{self, BAD_REQUEST, OUT_OF_RANGE, READ_MANY, READ_ONLY, SECTOR, SECTORS_MAX, WRITE_MANY};
        use taaj::disk::{DiskError, Window};
        use taaj::rights::{READ, TRANSFER, WRITE};

        let request = |op: u8, sector: u64, count: u32| {
            let mut bytes = [0u8; block::MANY_REQUEST_SIZE];
            bytes[0] = op;
            bytes[1..9].copy_from_slice(&sector.to_le_bytes());
            bytes[9..].copy_from_slice(&count.to_le_bytes());
            bytes
        };
        // A memory object of `bytes` bytes, as a handle to lend.
        let memory = |bytes: u64, rights: u32| {
            let object = taaj::memory_create(bytes).ok()?;
            let lent = taaj::duplicate(object, rights | TRANSFER).ok();
            let _ = taaj::close(object);
            lent
        };
        let refused = |result: Result<(), DiskError>, status: u8| result == Err(DiskError::Refused(status));
        let mut passed = 0;
        let mut check = |ok: bool, what: &'static str| {
            passed += 1;
            if ok { Ok(()) } else { Err(what) }
        };
        let (sectors, _) = disk.info().map_err(|_| "the disk does not answer")?;
        let full = u64::from(SECTORS_MAX) * SECTOR as u64;

        // 1. Many sectors at once are the same bytes as one at a time.
        let mut window = Window::new(SECTORS_MAX).map_err(|_| "no memory for a window")?;
        let count = SECTORS_MAX.min(sectors as u32);
        let many = disk.read_many(&mut window, 0, count).is_ok();
        let mut same = many;
        let mut single = [0u8; SECTOR];
        for n in 0..count as usize {
            same = same && disk.read(n as u64, &mut single).is_ok() && window.bytes()[n * SECTOR..][..SECTOR] == single;
        }
        check(same, "sectors read together differ from sectors read one by one")?;

        // 2-5. Requests the driver must refuse without touching memory.
        let small = memory(4096, READ | WRITE).ok_or("no memory")?;
        check(refused(disk.raw(&request(READ_MANY, 0, 16), Some(small)), BAD_REQUEST), "a memory object too small was accepted")?;
        let (ours, theirs) = taaj::channel_create().map_err(|_| "no channel")?;
        check(refused(disk.raw(&request(READ_MANY, 0, 8), Some(theirs)), BAD_REQUEST), "a channel was accepted as memory")?;
        let _ = taaj::close(ours);
        let read_only = memory(full, READ).ok_or("no memory")?;
        check(refused(disk.raw(&request(READ_MANY, 0, 8), Some(read_only)), BAD_REQUEST), "memory it may not write was accepted for a read")?;
        let none = memory(full, READ | WRITE).ok_or("no memory")?;
        check(refused(disk.raw(&request(READ_MANY, 0, 0), Some(none)), BAD_REQUEST), "a request for no sectors was accepted")?;
        let many = memory(2 * full, READ | WRITE).ok_or("no memory")?;
        check(refused(disk.raw(&request(READ_MANY, 0, SECTORS_MAX + 1), Some(many)), BAD_REQUEST), "more sectors than the limit were accepted")?;

        // 6. Starting on the disk and running off its end.
        let past = memory(full, READ | WRITE).ok_or("no memory")?;
        check(refused(disk.raw(&request(READ_MANY, sectors.saturating_sub(4), 8), Some(past)), OUT_OF_RANGE), "a read past the end was accepted")?;
        let wrapped = memory(full, READ | WRITE).ok_or("no memory")?;
        check(refused(disk.raw(&request(READ_MANY, u64::MAX, 8), Some(wrapped)), OUT_OF_RANGE), "a sector number that wraps was accepted")?;

        // 7. This channel may look, not change.
        let data = memory(full, READ).ok_or("no memory")?;
        check(refused(disk.raw(&request(WRITE_MANY, 0, 8), Some(data)), READ_ONLY), "a write was accepted on the read-only channel")?;

        // And the driver is still there.
        disk.info().map_err(|_| "the driver did not survive the checks")?;
        Ok(passed)
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
    ///
    /// Where the file is looked for depends on how it was named. A bare
    /// name - `ls` - is a command, and commands are the files in the
    /// root, wherever the shell happens to be: `ls` must not stop
    /// working after `cd docs`, and must not become another program
    /// because the directory has a file called `ls` in it. A name with
    /// a '/' in it - `tools/given`, `./given`, `/tools/given` - is a
    /// path, and is resolved like any other path, from here.
    fn program_image(&self, name: &str) -> Result<Option<Handle>, &'static str> {
        let resolved;
        let name = if name.contains('/') {
            resolved = path::resolve(&self.directory.borrow(), name);
            resolved.as_str()
        } else {
            name
        };
        let Some(files) = self.look.as_ref().or(self.files.as_ref()) else { return Ok(None) };
        let size = match files.size(name) {
            Ok(size) => size,
            // Not answering is not "no such file": say what is wrong.
            Err(FileError::Unreachable(Error::TimedOut)) => return Err("cannot be looked up: the file service is not answering"),
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
    /// image. The program gets `arguments` and the screen, as the shell
    /// has it - and the files only if it is one of FILE_PROGRAMS, or
    /// the user said so (`grant`): a program gets what it needs,
    /// not what the shell happens to hold. A program that gets the
    /// files is also told the current directory; one that does not has
    /// no use for it.
    ///
    /// `run NAME` says how the program ended; a program run by its name
    /// alone is a command, and says nothing unless it failed.
    fn run_program(&self, name: &str, arguments: &str, explicit: bool, grant: Grant) {
        let image = match self.program_image(name) {
            Ok(None) if !explicit => return say!(self, "unknown command: {name} (try help)\n"),
            Ok(image) => image,
            Err(why) => return say!(self, "run: {name} {why}\n"),
        };
        // The files go only to a program from a file: "ls" built into
        // the kernel image would be another program of the same name.
        let on_disk = |list: &[&str]| image.is_some() && list.contains(&name);
        let files = match grant {
            Grant::Files => self.files.as_ref(),
            Grant::ReadOnly => self.look.as_ref(),
            Grant::Usual if on_disk(CHANGE_PROGRAMS) => self.files.as_ref(),
            Grant::Usual if on_disk(LOOK_PROGRAMS) => self.look.as_ref(),
            Grant::Usual => None,
        };
        let mut handles = [0u64; 3];
        let mut count = 0;
        let mut flags = 0;
        let offered = [
            (0, image),
            (abi::startup::STDOUT, Self::delegate(self.screen)),
            (abi::startup::FILES, Self::delegate(files.map(Files::channel))),
        ];
        for (flag, handle) in offered {
            if let Some(handle) = handle {
                handles[count] = handle;
                count += 1;
                flags |= flag as u8;
            }
        }
        let handles = &handles[..count];
        // A copy: the borrow of the directory must not last while the
        // program runs.
        let directory = if files.is_some() { self.directory.borrow().clone() } else { String::new() };
        let request = abi::init::Run {
            kind: if image.is_some() { abi::init::IMAGE } else { abi::init::BUILT_IN },
            flags,
            name,
            directory: &directory,
            arguments: arguments.as_bytes(),
        };
        let mut message = [0u8; abi::MESSAGE_MAX as usize];
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
        look: taaj::startup_handle(6).map(Files::new),
        directory: RefCell::new(String::new()),
    };
    println!("[shell] ready - type a command (try help)");
    shell.screen_only(b"Welcome to TheTaaJ. Type help for the commands.\n\n");
    shell.prompt();

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
                shell.prompt();
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
                shell.prompt();
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
