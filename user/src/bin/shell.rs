//! The shell: reads a line from the keyboard, runs it as a command.
//!
//! One startup handle, from init: the channel to the registry. What
//! the shell works with it asks for there, by name (taaj::services):
//!   keys        the channel from the keyboard driver (receive only)
//!   display     the channel to the display (send only)
//!   init        the request channel to init: "start this program"
//!   system      the system handle: looking at memory and processes
//!   files       the request channel to the file service
//!   files.look  the same, for requests that only read
//!   disk.look   the request channel to the disk driver: reads only -
//!               files on the disk are the file service's to change
//! Those are the names that exist for the shell; `services` lists
//! them, and `services NAME` asks for one. The disk driver's other
//! channel, "disk", exists too - for the file service, not here.
//!
//! It has no launcher and no hardware. To run a program it asks init,
//! and sends along what the program is to have: a copy of its channel to
//! the screen, a copy of its channel to the file service. init replies
//! with a handle to the new process, which lets the shell wait for it
//! to end - and end it: Ctrl+C stops the program in the foreground.
//!
//! Keys typed while a program runs belong to whoever reads next. A
//! program in the foreground that was given no other input has the
//! keyboard as its standard input - but only when it asks: it says it
//! is about to wait for input (abi::stream::WANT), and the shell then
//! lets a line be typed for it, shown and corrected as the shell's own
//! lines are, and sends it at Enter. Ctrl+D ends that input. Keys a
//! program never asks for are kept, and read as the shell's next line.
//!
//! A word that is not one of the shell's own commands is looked for as
//! a file: `ls`, `cat`, `write`, `append`, `rm`, `mv` and `cp` are
//! programs on the disk, like any other.
//!
//! A command's output can be sent to a file, and its input taken from
//! one: `ls > list`, `echo more >> list`, `wc < list`. The program is
//! not given the file, or the file service. It is given a channel -
//! its standard output, or input - and the shell stands at the other
//! end, writing what arrives into the file, or sending the file's
//! contents a piece at a time. So redirection is the one way the shell
//! changes files itself, on the user's word; and a program that was
//! granted no files still has none.
//!
//! Commands can be joined: in `cat notes | wc`, what `cat` prints is
//! what `wc` reads. For that the shell makes a channel, gives one end
//! to each program, and keeps neither: the bytes go from one program to
//! the other, and the shell only waits for both to end. One of the
//! shell's own commands may begin a pipeline (`ps | wc`); it runs
//! first, to its end, and what it printed is then sent to the next.
//!
//! A line that ends in `&` is run in the background: the shell starts
//! it, says its number, and takes the next line at once. `jobs` lists
//! what is running so; `kill N` ends job N; `wait` waits until all of
//! them are over. The shell can end exactly
//! the programs it started, because those are the ones it holds a
//! handle to - there is no "kill any process by its number".
//!
//! Everything the shell has running is a job, in one list: the
//! background ones, and - while the shell waits for it - the one in
//! the foreground. One loop (`attend`) watches them all, with the
//! keyboard; "waiting for a command" is that loop until the foreground
//! job is over, and "waiting for a key" is the same loop until a key
//! comes. So a background job whose output goes to a file is served
//! whatever else the shell is doing.
//!
//! The shell is also where the current directory lives: `cd` changes
//! it, `pwd` shows it, the prompt shows it. It is a path the shell
//! remembers and nothing else - the file service is never told. A
//! program that is given the files is told the directory it was started
//! in, and resolves the names it is given against that (taaj::files).
//!
//! `source FILE` runs the lines of a file as if they were typed, one
//! after another, and stops at the first that fails: a script, such as
//! the one that builds the C library on TheTaaJ (libc/build.taaj).
//!
//! Output goes to the screen and to the kernel's debug log (the serial
//! line), which is what the boot tests read.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::{Cell, RefCell};
use core::fmt::Write;

use abi::ProcessEntry;
use taaj::disk::Disk;
use input::line::Command;
use taaj::files::{path, FileError, Files, Kind};
use taaj::services::{ServiceError, Services};
use taaj::{println, Buffer, Error, Handle};

taaj::program!(main);

const LINE_MAX: usize = 256;
const CTRL_C: &[u8] = b"\x03";
/// "No more input": the end of what is being typed to a program.
const CTRL_D: &[u8] = b"\x04";
/// Keys kept while a program runs; more than this are dropped.
const TYPED_AHEAD_MAX: usize = 256;

/// The programs that are given the file service when run by name:
/// the ones whose whole job is files - and each only as much of it as
/// that job takes. These read; they get the channel that cannot change
/// anything.
const LOOK_PROGRAMS: &[&str] = &["ls", "cat", "wc", "log"];
/// And these change files - the C compiler too, which reads sources,
/// headers and libraries and writes what it makes of them.
const CHANGE_PROGRAMS: &[&str] = &["write", "append", "rm", "mkdir", "mv", "cp", "tcc"];
/// The one program given the network card: the check of its driver.
const CARD_PROGRAMS: &[&str] = &["niccheck"];
/// And these are given the network.
const NET_PROGRAMS: &[&str] = &["ping", "netinfo", "udpecho", "udpsend", "tcpecho", "tcpsend", "cnet"];
/// And these are given both: the network, and the files to read only -
/// a web server, which gives files out and must never change one.
const WEB_PROGRAMS: &[&str] = &["httpd"];
/// And these open windows: they are given the window server.
const WINDOW_PROGRAMS: &[&str] = &["counter"];
/// And these open windows on files they change: the editor.
const EDIT_PROGRAMS: &[&str] = &["edit"];
/// The shell's own commands - the words `run_words` knows, and must be
/// kept the same as. They read no input, so in a pipeline one may only
/// stand first.
const OWN_COMMANDS: &[&str] =
    &["help", "echo", "cd", "pwd", "clear", "uptime", "mem", "ps", "time", "run", "with", "fs", "display", "disk", "jobs", "kill", "wait", "services", "registry", "source", "poweroff", "reboot"];
/// The largest script `source` reads.
const SCRIPT_MAX: u64 = 64 * 1024;
/// How deep scripts may run scripts: deep enough for any real use, and
/// a script that sources itself stops instead of running for ever.
const SOURCE_DEPTH_MAX: u32 = 4;
/// "one CPU" or "4 CPUs": how many, in words a sentence can use.
fn cpus_text(cpus: u64) -> String {
    if cpus == 1 {
        String::from("one CPU")
    } else {
        alloc::format!("{cpus} CPUs")
    }
}

/// Most commands in one pipeline. The wait watches them all at once,
/// with the keyboard and an output, and the kernel waits on 32 things
/// at most - but nobody types eight.
const PIPELINE_MAX: usize = 8;
/// Most things the shell may be watching when a background job is
/// started: the kernel waits on WAIT_HANDLES_MAX at once, and there
/// must always be room left for a whole pipeline in the foreground -
/// its programs, and its output.
const WATCHED_MAX: usize = abi::WAIT_HANDLES_MAX as usize - (PIPELINE_MAX + 1);
/// Most output kept from one of the shell's own commands for the next
/// command of a pipeline to read.
const KEPT_MAX: usize = 1 << 20;

/// What a program is to be given of the files. Everything not on the
/// lists above gets the screen and its arguments; `with files COMMAND`
/// and `with readonly COMMAND` add the files, and `with programs COMMAND`
/// the files and the right to start programs, by the user's own
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
    /// The files, and init: the program may start programs of its own
    /// (posix_spawn), handing them what it was given and no more. clang
    /// needs it to run ld.lld.
    Programs,
}

struct Shell {
    /// Where the shell asks for services by name.
    services: Services,
    keys: Handle,
    /// Keys that arrived while a program was running and not asking
    /// for input, oldest first. They go to whoever reads next: the
    /// program, if it asks after all, or else the shell's next line.
    typed_ahead: RefCell<VecDeque<Vec<u8>>>,
    /// The line being typed to the program in the foreground.
    typing: RefCell<Typing>,
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
    /// Where the output of the line being run goes, and where its input
    /// comes from, if the line said so. Set by `run` for the length of
    /// one line.
    redirection: RefCell<Redirection>,
    /// Everything the shell has running: see `Job`.
    jobs: RefCell<Vec<Job>>,
    /// Whether the line being run has failed: a program that ended with
    /// a code other than 0 (or was killed), a command that is not
    /// there, a directory `cd` could not go to. Cleared as each line
    /// starts; `source` reads it after each line.
    failed: Cell<bool>,
    /// How many scripts are running inside one another.
    sourcing: Cell<u32>,
}

/// Programs the shell started and has not finished with: one program,
/// or the several of a pipeline, with what the shell does for them
/// while they run.
struct Job {
    /// What the user calls it: 1, 2, ... for a background job. 0 is
    /// the job in the foreground, which needs no name - there is at
    /// most one, and the shell is waiting for it.
    number: usize,
    /// The line that started it, to show in `jobs`.
    text: String,
    /// Its programs: names, process handles, and exit codes - None
    /// while a program is running. All three in the pipeline's order.
    names: Vec<String>,
    processes: Vec<Handle>,
    codes: Vec<Option<i64>>,
    /// The shell's end of the channel the last program writes to, if
    /// its output does not go to the screen.
    output: Option<Handle>,
    /// The file that output is added to. None for the foreground job:
    /// its output goes where the line being run says (`Shell::write`).
    /// A background job outlives its line, so it carries the path.
    sink: Option<String>,
    /// The input the shell is sending the first program, if any.
    feed: Option<Feed>,
    /// The first thing that went wrong writing `sink`.
    failed: Option<FileError>,
}

impl Job {
    /// Whether every program has ended.
    fn ended(&self) -> bool {
        self.codes.iter().all(Option::is_some)
    }

    /// Whether there is nothing left to do for it: the programs have
    /// ended and their last output has been taken.
    fn over(&self) -> bool {
        self.ended() && self.output.is_none() && self.feed.is_none()
    }

    /// How it ended, in a word or a few - the first program that did
    /// not simply finish decides.
    fn ending(&self) -> String {
        for (name, code) in self.names.iter().zip(&self.codes) {
            match code {
                Some(abi::STOPPED) => return String::from("stopped"),
                Some(abi::KILLED) => return alloc::format!("{name} was killed by the kernel"),
                Some(code) if abi::signal::of_exit_code(*code) == Some(abi::signal::SIGINT) => return String::from("stopped"),
                Some(code) if let Some(signal) = abi::signal::of_exit_code(*code) => {
                    return alloc::format!("{name} was ended by {}", abi::signal::name(signal));
                }
                // A writer whose reader left has finished, as far as
                // anyone cares.
                Some(0) | Some(abi::NO_READER) | None => {}
                Some(code) => return alloc::format!("{name} exited with code {code}"),
            }
        }
        String::from("done")
    }
}

/// What `attend` is to wait for.
#[derive(Clone, Copy, PartialEq)]
enum Until {
    /// The foreground job being over.
    Foreground,
    /// A key.
    Key,
    /// Every background job being over: the command `wait`.
    Jobs,
}

/// What `attend` came back with.
enum Attended {
    /// A key, of this many bytes, in the buffer it was given.
    Key(usize),
    /// The foreground job is over: its programs' exit codes. (For
    /// `Until::Jobs`: the background jobs are over; no codes.)
    Ended(Vec<i64>),
}

/// Where the output of what the shell runs goes, when not to the user.
#[derive(Clone)]
enum Sink {
    /// Added to the end of this file: a whole path from the root.
    File(String),
    /// Kept in memory (`Redirection::kept`), for the next command of a
    /// pipeline.
    Memory,
}

/// Where a program's input comes from, when the shell provides it.
enum Supply {
    /// This file: a whole path from the root.
    File(String),
    /// These bytes: what one of the shell's own commands printed.
    Memory(Vec<u8>),
    /// The keyboard: what is typed while the program is asking for
    /// input, a line at a time.
    Keyboard,
}

/// A line as it is typed: the bytes so far. Used for the shell's own
/// command line, and for a line typed to a program - the same keys do
/// the same things in both.
struct Typing {
    line: [u8; LINE_MAX],
    length: usize,
}

impl Typing {
    const fn new() -> Typing {
        Typing { line: [0; LINE_MAX], length: 0 }
    }

    fn bytes(&self) -> &[u8] {
        &self.line[..self.length]
    }

    fn clear(&mut self) {
        self.length = 0;
    }

    /// A key that changes the line: a character is added, Backspace
    /// takes the last one away. `show` is given what to put on the
    /// screen for it. (Enter, Ctrl+C and Ctrl+D are the caller's: what
    /// they mean depends on whose line this is.)
    fn key(&mut self, key: &[u8], show: impl FnOnce(&[u8])) {
        match key {
            b"\x08" => {
                if self.length > 0 {
                    // Drop the whole last character, however many bytes.
                    self.length -= 1;
                    while self.length > 0 && self.line[self.length] & 0xC0 == 0x80 {
                        self.length -= 1;
                    }
                    show(b"\x08 \x08"); // back, blank, back
                }
            }
            [first, ..] if *first < 0x20 => {} // other control keys, escapes: not yet
            text if self.length + text.len() <= LINE_MAX => {
                self.line[self.length..self.length + text.len()].copy_from_slice(text);
                self.length += text.len();
                show(text);
            }
            _ => {} // line full
        }
    }
}

/// A line's redirections.
#[derive(Default)]
struct Redirection {
    output: Option<Sink>,
    /// Taken by the first program the line starts.
    input: Option<Supply>,
    /// The first thing that went wrong writing the output. After it
    /// nothing more is written, and the line's end reports it.
    failed: Option<FileError>,
    /// What `Sink::Memory` has collected.
    kept: Vec<u8>,
    /// Whether more than KEPT_MAX arrived, and was dropped.
    overflowed: bool,
    /// Whether the line ended in `&`: what it starts is not waited for.
    background: bool,
    /// The line as typed, without the `&`: a background job's name.
    text: String,
}

/// An input being sent to a program as its standard input, a piece at
/// a time: where the next piece starts, and the piece that was made
/// ready but has not fitted into the channel yet.
struct Feed {
    to: Handle,
    from: Supply,
    offset: u64,
    /// A whole stream message: the DATA byte, then the piece.
    message: [u8; abi::MESSAGE_MAX as usize],
    /// Its length; 0 if no piece is waiting.
    length: usize,
    /// Whether the waiting piece is the last.
    last: bool,
    /// For the keyboard: whether the program has asked for input and
    /// not been given a line yet.
    wanted: bool,
}

/// What a program is to have as its standard input: its end of a
/// channel, if there is an input, and whether the far end is the
/// keyboard - an input that sends nothing until it is asked.
struct Stdin {
    channel: Option<Handle>,
    keyboard: bool,
}

/// What a program is to have as its standard output.
enum Output {
    /// The screen, as the shell has it.
    Screen,
    /// The sending end of a channel: to the shell, or to the next
    /// program of a pipeline.
    Channel(Handle),
}

/// Formats into a buffer, then writes it at once as output of the
/// command being run: to screen and log, or to where the line sent it.
macro_rules! say {
    ($shell:expr, $($arg:tt)*) => {{
        let mut text = Buffer::<512>::new();
        let _ = write!(text, $($arg)*);
        $shell.write(text.as_bytes());
    }};
}

/// The same, but always to the screen and the log: what the shell has
/// to tell the user about a command, which is not the command's output.
macro_rules! tell {
    ($shell:expr, $($arg:tt)*) => {{
        let mut text = Buffer::<512>::new();
        let _ = write!(text, $($arg)*);
        $shell.tell(text.as_bytes());
    }};
}

impl Shell {
    /// Output of the command being run: `echo`'s text, `ps`'s table.
    /// It goes where the line said - a file, the next command - or
    /// else to the user.
    fn write(&self, bytes: &[u8]) {
        // A copy: the borrow must end before `to_file` borrows the
        // same cell, to note a failure.
        let output = self.redirection.borrow().output.clone();
        match output {
            Some(Sink::File(path)) => self.to_file(&path, bytes),
            Some(Sink::Memory) => {
                let mut redirection = self.redirection.borrow_mut();
                let room = KEPT_MAX - redirection.kept.len();
                redirection.overflowed |= bytes.len() > room;
                redirection.kept.extend_from_slice(&bytes[..bytes.len().min(room)]);
            }
            None => self.tell(bytes),
        }
    }

    /// To the user, whatever the line said about its output: the
    /// screen, and the debug log.
    fn tell(&self, bytes: &[u8]) {
        // Where the screen is the log, once is enough.
        if !(taaj::LOG_IS_SCREEN && self.screen.is_some()) {
            let _ = taaj::write(bytes);
        }
        self.screen_only(bytes);
    }

    /// Adds `bytes` to the end of the output file. The first failure
    /// is kept, and nothing is written after it: a file with a hole in
    /// the middle would look whole.
    fn to_file(&self, path: &str, bytes: &[u8]) {
        if self.redirection.borrow().failed.is_some() {
            return;
        }
        let written = match &self.files {
            Some(files) => files.append(path, bytes).map(|_| ()),
            None => Err(FileError::Unreachable(Error::PeerClosed)),
        };
        if let Err(error) = written {
            self.redirection.borrow_mut().failed = Some(error);
        }
    }

    /// Gets the line's output file and input file ready, before the
    /// command runs. False - with the reason said - if either cannot
    /// be: then nothing is run, and nothing was changed.
    ///
    /// The output file is made, or emptied, here - before the command
    /// starts, as in a Unix shell. So `ls > list` lists `list` itself,
    /// empty; and `cat < f > f` empties `f` before `cat` has read it.
    /// The input is looked at first, so that a line whose input is not
    /// there does not cost an output file its contents.
    fn redirect(&self, input: Option<&String>, output: Option<&(String, bool)>) -> bool {
        let here = self.directory.borrow().clone();
        let mut redirection = Redirection::default();
        if let Some(name) = input {
            let path = path::resolve(&here, name);
            let Some(files) = self.look.as_ref().or(self.files.as_ref()) else {
                tell!(self, "{name}: no file service\n");
                return false;
            };
            match files.stat(&path) {
                Ok((Kind::File, _)) => {}
                Ok((Kind::Directory, _)) => {
                    tell!(self, "{name}: is a directory\n");
                    return false;
                }
                Err(error) => {
                    tell!(self, "{name}: {}\n", error.describe());
                    return false;
                }
            }
            redirection.input = Some(Supply::File(path));
        }
        if let Some((name, append)) = output {
            let path = path::resolve(&here, name);
            let Some(files) = &self.files else {
                tell!(self, "{name}: no file service\n");
                return false;
            };
            // Adding nothing makes the file if it is not there, and
            // leaves it alone if it is.
            let made = if *append { files.append(&path, &[]) } else { files.write(&path, &[]) };
            if let Err(error) = made {
                tell!(self, "{name}: {}\n", error.describe());
                return false;
            }
            redirection.output = Some(Sink::File(path));
        }
        *self.redirection.borrow_mut() = redirection;
        true
    }

    fn screen_only(&self, bytes: &[u8]) {
        if let Some(screen) = self.screen {
            taaj::write_to_display(screen, bytes);
        }
    }

    /// The prompt says where the shell is: "taaj:/docs> ". On the
    /// screen only - the log has the line that follows it.
    fn prompt(&self) {
        // Before the prompt is the moment to say which background jobs
        // have ended: never in the middle of another command's output.
        self.report_jobs();
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
            Ok((Kind::Directory, _)) => return *self.directory.borrow_mut() = wanted,
            Ok((Kind::File, _)) => say!(self, "cd: {argument}: not a directory\n"),
            Err(error) => say!(self, "cd: {argument}: {}\n", error.describe()),
        }
        self.failed.set(true);
    }

    /// `source FILE`: the file's lines, run one by one as if typed -
    /// except blank lines and those that begin with `#`, which are for
    /// the reader. Each is shown first, after a `+`, so the screen says
    /// how far it got.
    ///
    /// It stops at the first line that fails, and says which. A Unix
    /// shell would go on (unless told `set -e`); but a script here is a
    /// build, and a build that goes on after a compiler error only
    /// buries the error under the ones that follow from it.
    fn source(&self, name: &str) {
        let path = path::resolve(&self.directory.borrow(), name);
        let Some(files) = self.look.as_ref().or(self.files.as_ref()) else {
            self.failed.set(true);
            return tell!(self, "source: no file service\n");
        };
        let size = match files.stat(&path) {
            Ok((Kind::File, size)) => size,
            Ok((Kind::Directory, _)) => {
                self.failed.set(true);
                return tell!(self, "source: {name}: is a directory\n");
            }
            Err(error) => {
                self.failed.set(true);
                return tell!(self, "source: {name}: {}\n", error.describe());
            }
        };
        if size > SCRIPT_MAX {
            self.failed.set(true);
            return tell!(self, "source: {name}: longer than {} KiB\n", SCRIPT_MAX / 1024);
        }
        if self.sourcing.get() >= SOURCE_DEPTH_MAX {
            self.failed.set(true);
            return tell!(self, "source: {name}: scripts {SOURCE_DEPTH_MAX} deep - does one source itself?\n");
        }
        let mut script = Vec::new();
        if let Err(error) = files.read(&path, |piece| script.extend_from_slice(piece)) {
            self.failed.set(true);
            return tell!(self, "source: {name}: {}\n", error.describe());
        }
        let Ok(script) = String::from_utf8(script) else {
            self.failed.set(true);
            return tell!(self, "source: {name}: not text (not UTF-8)\n");
        };
        self.sourcing.set(self.sourcing.get() + 1);
        for (n, line) in script.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            println!("[shell] {name} line {}: {line:?}", n + 1);
            tell!(self, "+ {line}\n");
            self.run(line);
            if self.failed.get() {
                tell!(self, "source: {name}: stopped at line {}\n", n + 1);
                break;
            }
        }
        self.sourcing.set(self.sourcing.get() - 1);
    }

    /// Runs a line as it was typed: divides it into words - spaces,
    /// quotes, backslashes (input::line) - and runs those. A line that
    /// cannot be divided, because a quote was never closed, is not run
    /// at all: half a command is worse than none.
    ///
    /// The line may also say where the output goes and where the input
    /// comes from. That holds for the whole line - `time ls > list`
    /// puts the listing and the time in `list` - and is forgotten when
    /// the line is done. And it may be several commands joined by `|`:
    /// then the input is the first one's and the output the last one's.
    fn run(&self, text: &str) {
        self.failed.set(false);
        let line = match input::line::parse(text) {
            Ok(line) => line,
            Err(error) => {
                self.failed.set(true);
                return tell!(self, "{}\n", error.describe());
            }
        };
        let commands = &line.commands;
        // `parse` gives at least one command, so there is a first and
        // a last - the same one, on most lines.
        let (Some(first), Some(last)) = (commands.first(), commands.last()) else { return };
        if line.background {
            // The shell's own commands are this code, running now:
            // they are over before "do not wait" could mean anything.
            // `run` and `with` start a program, which is what the &
            // is then for. (First in a pipeline one runs to its end,
            // and the & is for the programs after it.)
            if let ([only], false) = (commands.as_slice(), matches!(name(first), "run" | "with")) {
                if OWN_COMMANDS.contains(&name(only)) {
                    return tell!(self, "{} is one of the shell's own commands: it cannot run in the background\n", name(only));
                }
            }
            let watched: usize = 1 + self.jobs.borrow().iter().map(|job| job.processes.len() + 1).sum::<usize>();
            if watched + commands.len() + 1 > WATCHED_MAX {
                return self.tell(b"too many jobs are running: end one first (jobs, kill N)\n");
            }
        }
        if !self.redirect(first.input.as_ref(), last.output.as_ref()) {
            return self.failed.set(true);
        }
        {
            let mut redirection = self.redirection.borrow_mut();
            redirection.background = line.background;
            // The line without its &, and the spaces before it.
            redirection.text = String::from(text.trim().trim_end_matches('&').trim_end());
        }
        match commands.as_slice() {
            [only] => self.run_words(&only.words),
            several => self.run_pipeline(several),
        }
        // `take` leaves an empty Redirection behind: the next line
        // starts with none.
        let done = self.redirection.take();
        if let (Some((name, _)), Some(error)) = (&last.output, done.failed) {
            self.failed.set(true);
            tell!(self, "{name}: {}\n", error.describe());
        }
    }

    /// Runs commands joined by `|`: each one's output is the next
    /// one's input.
    ///
    /// Between two programs the shell makes a channel and gives away
    /// both ends - the sending end as one program's standard output,
    /// the receiving end as the other's standard input. It keeps
    /// neither, and that matters: the end of a stream is its sender
    /// closing, so a copy of the sending end left here would keep the
    /// reader waiting for ever after the writer had gone.
    ///
    /// All the programs are started, and then all waited for together.
    /// They must run at the same time: a channel holds 32 messages, so
    /// a writer with more to say than that needs its reader running.
    ///
    /// One of the shell's own commands can only be first. It cannot
    /// run "at the same time" - it is this code - so it runs to its
    /// end with its output kept in memory, and that is then sent to
    /// the next command as a file would be.
    fn run_pipeline(&self, commands: &[Command]) {
        if commands.len() > PIPELINE_MAX {
            self.failed.set(true);
            return tell!(self, "a pipeline can join {PIPELINE_MAX} commands at most\n");
        }
        if let Some(own) = commands[1..].iter().map(name).find(|name| OWN_COMMANDS.contains(name)) {
            self.failed.set(true);
            return tell!(self, "{own} is one of the shell's own commands: it reads nothing, so it can only begin a pipeline\n");
        }
        // Every program is found and read before any is started: a
        // mistyped name at the end must not let the commands before it
        // run - `rm notes | wcc` would have removed the file.
        let own_first = OWN_COMMANDS.contains(&name(&commands[0]));
        let programs = &commands[usize::from(own_first)..];
        let mut images = Vec::new();
        for command in programs {
            let name = name(command);
            let image = match self.program_image(name) {
                Ok(Some(image)) => Ok(image),
                Ok(None) => Err("unknown command"),
                Err(why) => Err(why),
            };
            match image {
                Ok(image) => images.push(image),
                Err(why) => {
                    close_all(images);
                    self.failed.set(true);
                    return match why {
                        "unknown command" => tell!(self, "unknown command: {name} (try help)\n"),
                        why => tell!(self, "run: {name} {why}\n"),
                    };
                }
            }
        }
        if own_first {
            // Its output goes into memory for the length of the
            // command, and then becomes the input of what follows.
            // And it is waited for, whatever the line ends in: the &
            // is put aside until the programs after it are started.
            let (output, background) = {
                let mut redirection = self.redirection.borrow_mut();
                (redirection.output.replace(Sink::Memory), core::mem::take(&mut redirection.background))
            };
            self.run_words(&commands[0].words);
            let mut redirection = self.redirection.borrow_mut();
            redirection.output = output;
            redirection.background = background;
            let kept = core::mem::take(&mut redirection.kept);
            redirection.input = Some(Supply::Memory(kept));
            if core::mem::take(&mut redirection.overflowed) {
                // The borrow must end before `tell` - which does not
                // use it, but the rule is simpler kept than argued.
                drop(redirection);
                tell!(self, "{}: more than {} KiB of output; the rest was dropped\n", name(&commands[0]), KEPT_MAX / 1024);
            }
        }
        // Start them, first to last. `carried` is the receiving end of
        // the channel the previous program writes to: this one's input.
        let mut started: Vec<Handle> = Vec::new();
        let mut carried: Option<Handle> = None;
        let mut ours: (Option<Handle>, Option<Feed>) = (None, None);
        let mut images = images.into_iter();
        let mut failed = false;
        for (n, command) in programs.iter().enumerate() {
            let Some((name, arguments)) = command.words.split_first() else { break };
            let image = images.next();
            // Its input: the previous program, or - for the first -
            // whatever the line gave.
            let stdin = match carried.take() {
                Some(previous) => Ok(Some(previous)),
                None => self.input_end().map(|(theirs, feed)| {
                    ours.1 = feed;
                    theirs
                }),
            };
            // Only the first program can have the keyboard as its
            // input: the others read the program before them.
            let keyboard = n == 0 && matches!(&ours.1, Some(Feed { from: Supply::Keyboard, .. }));
            // Its output: the next program, or - for the last - the
            // file or the screen.
            let stdout = if n + 1 < programs.len() {
                taaj::channel_create().map(|(send, receive)| {
                    carried = Some(receive);
                    Output::Channel(send)
                })
            } else {
                self.output_end().map(|(theirs, kept)| {
                    ours.0 = kept;
                    theirs
                })
            };
            let process = match (stdin, stdout) {
                (Ok(stdin), Ok(stdout)) => self.start(name, arguments, image, Grant::Usual, stdout, Stdin { channel: stdin, keyboard }),
                (stdin, stdout) => {
                    tell!(self, "run: {name}: no channel for its output or input\n");
                    let stdout = stdout.ok().and_then(|output| match output {
                        Output::Channel(handle) => Some(handle),
                        Output::Screen => None,
                    });
                    close_all(stdin.ok().flatten().into_iter().chain(stdout).chain(image));
                    None
                }
            };
            match process {
                Some(process) => started.push(process),
                None => {
                    failed = true;
                    break;
                }
            }
        }
        if failed {
            // One could not be started: the ones before it have nobody
            // to write to. End them, and let go of everything.
            self.failed.set(true);
            close_all(images.chain(carried));
            for &process in &started {
                let _ = taaj::kill(process);
            }
        }
        let (output, feed) = ours;
        if !failed && self.redirection.borrow().background {
            let names = programs.iter().map(|command| String::from(name(command))).collect();
            return self.background(names, started, output, feed);
        }
        let codes = self.wait_for(&started, output, feed);
        for (n, &process) in started.iter().enumerate() {
            if !failed {
                let code = codes.as_ref().map(|codes| codes[n]).map_err(|&error| error);
                self.report(name(&programs[n]), code, false);
            }
            let _ = taaj::close(process);
        }
    }

    /// Runs a command: its name, then its arguments. Where one argument
    /// ends and the next begins was settled by `run`, and is not looked
    /// at again - not here, and not by the program that gets them.
    fn run_words(&self, words: &[String]) {
        // `split_first` gives the first element and a slice of the
        // rest, or None for an empty slice: a blank line.
        let Some((command, arguments)) = words.split_first() else { return };
        // The arguments as one text, for the commands that print them
        // or read numbers from them.
        let rest = arguments.join(" ");
        let rest = rest.as_str();
        match command.as_str() {
            "help" => self.write(
                b"commands:\n\
                  \x20 help, echo TEXT   this list; print TEXT\n\
                  \x20   (a word with spaces: \"two words\", 'two words', two\\ words)\n\
                  \x20 clear, uptime, mem, ps   clear the screen; time since boot; memory;\n\
                  \x20              processes\n\
                  \x20 COMMAND > FILE, >> FILE, < FILE   output into, added to; input from\n\
                  \x20                  (with no input given, a program reads what is typed;\n\
                  \x20                  Ctrl+D ends it)\n\
                  \x20 A | B        what A prints is what B reads\n\
                  \x20 COMMAND &    run in the background: jobs lists those, kill N ends\n\
                  \x20              job N, wait waits for them all\n\
                  \x20 cd [DIR], pwd    go to DIR (to the root); say where this is\n\
                  \x20 time COMMAND     run COMMAND, and say how long it took\n\
                  \x20 source FILE      run FILE's lines as commands; stop at one that fails\n\
                  \x20 run NAME ARGS    run a program: a file, or hello, peek (Ctrl+C stops it)\n\
                  \x20 NAME ARGS    the same, for a file in the root (./NAME, or a\n\
                  \x20              path with a /, for one elsewhere). On the disk:\n\
                  \x20   ls [DIR...]        list a directory (this one)\n\
                  \x20   cat FILE..., wc FILE...   show files; count lines and words\n\
                  \x20   yes [TEXT]         print TEXT (y) for ever\n\
                  \x20   write FILE TEXT, append FILE TEXT   set, or add to, FILE\n\
                  \x20   mv FROM TO, cp FROM TO   move or rename; copy a file\n\
                  \x20   mkdir DIR...       make directories\n\
                  \x20   rm NAME...         remove files, empty directories\n\
                  \x20   (a name is a path from here, ../plan - or /from/the/root)\n\
                  \x20   given              say what it was given\n\
                  \x20 with files COMMAND      run COMMAND with the files too (readonly: to read)\n\
                  \x20 with programs COMMAND   the files, and it may start programs (clang x.c -o x)\n\
                  \x20 fs stall SECONDS   a test: the file service goes silent\n\
                  \x20 disk stall SECONDS, display stall SECONDS   the same,\n\
                  \x20              for the disk driver and the display\n\
                  \x20 (F11 hangs the keyboard driver, F12 crashes it)\n\
                  \x20 display layout NAME   the keyboard's layout: us, dvorak\n\
                  \x20 services [NAME]   the services the shell can ask for; ask for one\n\
                  \x20 disk, disk read N, disk write N TEXT   size; show sector N; set it\n\
                  \x20 disk check        test the disk driver\n\
                  \x20 disk crash N      a test: the disk driver dies in the middle of its Nth write\n\
                  \x20 poweroff, reboot  stop the services, then switch the machine off or reset it\n",
            ),
            "echo" => say!(self, "{rest}\n"),
            // Slice patterns: no argument, exactly one, or anything else.
            "cd" => match arguments {
                [] => self.cd("/"),
                [directory] => self.cd(directory),
                _ => self.write(b"usage: cd [DIR]\n"),
            },
            "pwd" => say!(self, "{}\n", path::display(&self.directory.borrow())),
            "clear" => self.screen_only(b"\x0c"),
            "uptime" => self.uptime(),
            "mem" => self.mem(),
            "ps" => self.ps(),
            "jobs" => self.list_jobs(),
            // Until every background job is over, or Ctrl+C. The next
            // prompt says how each ended.
            "wait" => {
                let _ = self.attend(Until::Jobs, &mut [0u8; 16]);
            }
            "kill" => match arguments {
                // `%1` is how a Unix shell writes job 1; allowed.
                [job] => match job.strip_prefix('%').unwrap_or(job).parse::<usize>() {
                    Ok(number @ 1..) => self.kill_job(number),
                    _ => self.write(b"usage: kill N (a job's number: see jobs)\n"),
                },
                _ => self.write(b"usage: kill N (a job's number: see jobs)\n"),
            },
            "time" if !arguments.is_empty() => {
                // Ticks are 10 ms apart: good for "is it slow", no finer.
                let start = taaj::ticks();
                // Two clocks: the time that passed, and the work the
                // CPUs did in it. With one CPU the second cannot be
                // more than the first; with four, four programs at
                // once make it four times as much.
                // Asked quietly: a shell without the system handle still
                // times things, with the one clock everyone may read.
                let look = || self.system.and_then(|system| taaj::system_info(system).ok());
                let busy = look().map(|info| info.busy_ticks);
                // The words that follow are a command, already divided.
                self.run_words(arguments);
                let hundredths = taaj::ticks() - start;
                match (busy, look()) {
                    (Some(busy), Some(info)) => {
                        let worked = info.busy_ticks - busy;
                        say!(
                            self,
                            "took {}.{:02} s; the {} worked for {}.{:02} s in all\n",
                            hundredths / 100,
                            hundredths % 100,
                            cpus_text(info.cpus),
                            worked / 100,
                            worked % 100
                        );
                    }
                    _ => say!(self, "took {}.{:02} s\n", hundredths / 100, hundredths % 100),
                }
            }
            "time" => self.write(b"usage: time COMMAND\n"),
            // `[name, arguments @ ..]`: a first element, and a name for
            // the slice of all the others (which may be empty).
            "run" => match arguments {
                [name, arguments @ ..] => self.run_program(name, arguments, true, Grant::Usual),
                [] => self.write(b"usage: run NAME ARGS\n"),
            },
            "with" => {
                let grant = match arguments.first().map(String::as_str) {
                    Some("files") => Some(Grant::Files),
                    Some("readonly") => Some(Grant::ReadOnly),
                    Some("programs") => Some(Grant::Programs),
                    _ => None,
                };
                match (grant, arguments) {
                    (Some(grant), [_, name, arguments @ ..]) => self.run_program(name, arguments, false, grant),
                    _ => self.write(b"usage: with files COMMAND, with readonly COMMAND, with programs COMMAND\n"),
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
                // The desktop benchmark (K114): composing and dragging,
                // timed by the display itself.
                _ if rest.trim() == "bench" => match self.screen {
                    Some(screen) => {
                        let _ = taaj::send(screen, &[abi::display::BENCH], &[]);
                    }
                    None => self.write(b"no display\n"),
                },
                // ... and key to screen: the next N keys, from their
                // arrival at the display to their echo on the screen.
                _ if rest.starts_with("keys ") => {
                    match (rest["keys ".len()..].trim().parse::<u32>().ok().filter(|n| (1..=abi::display::KEYS_MAX).contains(n)), self.screen) {
                        (Some(count), Some(screen)) => {
                            let mut message = [0u8; 5];
                            message[0] = abi::display::KEYS;
                            message[1..].copy_from_slice(&count.to_le_bytes());
                            let _ = taaj::send(screen, &message, &[]);
                        }
                        (Some(_), None) => self.write(b"no display\n"),
                        (None, _) => say!(self, "usage: display keys COUNT (1 to {})\n", abi::display::KEYS_MAX),
                    }
                }
                // The keyboard layout (K116): the display makes every
                // keyboard's characters, in the layout chosen here.
                _ if rest.trim() == "layout" || rest.starts_with("layout ") => {
                    let name = rest["layout".len()..].trim();
                    match (input::Layout::from_name(name.as_bytes()), self.screen) {
                        (Some(layout), Some(screen)) => {
                            if let Some((message, n)) = abi::display::layout_message(layout.name().as_bytes()) {
                                let _ = taaj::send(screen, &message[..n], &[]);
                                say!(self, "display: keyboard layout {}\n", layout.name());
                            }
                        }
                        (Some(_), None) => self.write(b"no display: the serial terminal's keys are the host's\n"),
                        (None, _) => {
                            self.write(b"usage: display layout NAME - the layouts:");
                            for layout in input::Layout::ALL {
                                say!(self, " {}", layout.name());
                            }
                            self.write(b"\n");
                        }
                    }
                }
                _ => self.write(b"usage: display stall SECONDS (1 to 30), display bench, display keys COUNT, display layout NAME\n"),
            },
            "disk" => self.disk(rest),
            "poweroff" | "reboot" => self.power(command == "poweroff"),
            "services" => match arguments {
                [] => match self.services.list() {
                    Ok(names) => say!(self, "{}\n", names.join(" ")),
                    Err(error) => say!(self, "services: {}\n", error.describe()),
                },
                // Asks, as a program would - and gives back at once
                // what it gets: the answer is what is being shown.
                [name] => match self.services.lookup(name) {
                    Ok(handle) => {
                        let _ = taaj::close(handle);
                        say!(self, "{name}: there for the shell\n");
                    }
                    Err(ServiceError::NotFound) => say!(self, "{name}: no such service - for the shell\n"),
                    Err(error) => say!(self, "{name}: {}\n", error.describe()),
                },
                _ => self.write(b"usage: services [NAME]\n"),
            },
            "source" => match arguments {
                [file] => self.source(file),
                _ => {
                    self.failed.set(true);
                    self.write(b"usage: source FILE\n");
                }
            },
            "registry" => match (rest, self.services.crash()) {
                ("crash", Ok(())) => self.write(b"registry: asked to crash\n"),
                ("crash", Err(error)) => say!(self, "registry: {}\n", error.describe()),
                _ => self.write(b"usage: registry crash (a test: init starts another)\n"),
            },
            // Not the shell's own: a program in a file, if there is one.
            // Any word added above belongs in OWN_COMMANDS too.
            _ => self.run_program(command, arguments, false, Grant::Usual),
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
        let cpus = cpus_text(info.cpus);
        say!(self, "up {}.{:02} s ({} ticks), on {cpus}\n", hundredths / 100, hundredths % 100, info.ticks);
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
        // CPU: the time the process's threads were on a CPU (journal
        // entry 114), in seconds to the millisecond.
        self.write(b"  PID  NAME        STATE           CPU\n");
        for entry in &entries[..total.min(entries.len())] {
            let state = match entry.exit_code {
                None => alloc::string::String::from("running"),
                Some(code) => alloc::format!("ended ({code})"),
            };
            let millis = entry.cpu_ns / 1_000_000;
            say!(self, "{:>5}  {:<10}  {:<12}  {:>3}.{:03} s\n", entry.pid, entry.name(), state, millis / 1000, millis % 1000);
        }
        if let Some(info) = self.system_info() {
            say!(self, "{} processes, {} threads, {} channels\n", info.processes, info.threads, info.channels);
        }
    }

    /// `poweroff`, `reboot`: asks init, which stops every service - this
    /// shell first - and then switches the machine off or resets it. So
    /// this returns only if init says it cannot.
    fn power(&self, off: bool) {
        self.write(if off { b"Switching off...\n" } else { b"Resetting...\n" });
        let Some(init) = self.init else {
            return self.write(b"cannot reach init\n");
        };
        let ask = if off { abi::power::ASK_OFF } else { abi::power::ASK_RESET };
        if taaj::send(init, ask, &[]).is_err() {
            return self.write(b"cannot reach init\n");
        }
        let mut reply = [0u8; 128];
        if let Ok((n, _)) = taaj::receive(init, &mut reply, &mut []) {
            self.write(&reply[..n]);
            self.write(b"\n");
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
            // The test of a driver killed mid-I/O (K82): it dies in the
            // middle of its Nth write from now, half of it written.
            ("crash", Some(Ok(writes)), None) => match abi::block::crash_request(writes) {
                Some(request) => match disk.raw(&request, None) {
                    Ok(()) => say!(self, "disk: the driver will die in the middle of write {writes} from now\n"),
                    Err(error) => say!(self, "disk: {}\n", error.describe()),
                },
                None => say!(self, "usage: disk crash WRITES (1 to {})\n", abi::block::CRASH_MAX),
            },
            ("check", None, None) => match self.disk_check(disk) {
                Ok(checks) => say!(self, "disk check: all {checks} checks passed\n"),
                Err(failed) => say!(self, "disk check: FAILED - {failed}\n"),
            },
            _ => self.write(b"usage: disk, disk read N, disk write N TEXT, disk check, disk stall SECONDS, disk crash WRITES\n"),
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
        let path = self.program_path(name);
        let name = path.as_str();
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
        // The file service writes the file into the object itself, 4 MiB
        // an exchange (READ_INTO). Asked for 1 KiB at a time, llvm-mc's
        // 10 MB took 10 seconds to arrive.
        let filled = match files.read_into(name, memory) {
            Ok(got) if got == size => Ok(()),
            Ok(_) => Err("changed while it was being read"),
            Err(error) => Err(error.describe()),
        };
        match filled {
            Ok(()) => Ok(Some(memory)),
            Err(why) => {
                let _ = taaj::close(memory);
                Err(why)
            }
        }
    }

    /// The file a program's name means, as a path from the root without
    /// the '/' in front ("" is the root): a name with a '/' in it is a
    /// path from here; a bare name is a file in the root, or else one in
    /// /bin - the two places libc's exec looks without a PATH (see
    /// `program_image`). If neither has it, the root's name, for the
    /// message.
    fn program_path(&self, name: &str) -> String {
        if name.contains('/') {
            return path::resolve(&self.directory.borrow(), name);
        }
        let in_bin = alloc::format!("bin/{name}");
        let files = self.look.as_ref().or(self.files.as_ref());
        match files {
            Some(files) if files.size(name).is_err() && matches!(files.stat(&in_bin), Ok((Kind::File, _))) => in_bin,
            _ => String::from(name),
        }
    }

    /// A copy of one of the shell's own handles, to give to a program:
    /// good for sending on, and for handing over - to init now, and by
    /// the program, copied, to a program it starts - nothing else.
    fn delegate(handle: Option<Handle>) -> Option<Handle> {
        use taaj::rights::{DUPLICATE, SEND, TRANSFER};
        taaj::duplicate(handle?, SEND | DUPLICATE | TRANSFER).ok()
    }

    /// The program's end of a channel for its output, and ours - if
    /// the line sent the output anywhere but the screen.
    fn output_end(&self) -> Result<(Output, Option<Handle>), Error> {
        if self.redirection.borrow().output.is_none() {
            return Ok((Output::Screen, None));
        }
        let (ours, theirs) = taaj::channel_create()?;
        Ok((Output::Channel(theirs), Some(ours)))
    }

    /// The program's end of a channel for its input, and what is to be
    /// sent down ours. It is given once: to the first program the line
    /// starts. If the line said where the input comes from, it is
    /// that; if not, it is the keyboard - except in the background,
    /// where there is no input at all: what is typed is for the shell
    /// and for the program in front.
    fn input_end(&self) -> Result<(Option<Handle>, Option<Feed>), Error> {
        let from = {
            let mut redirection = self.redirection.borrow_mut();
            match redirection.input.take() {
                Some(from) => from,
                None if redirection.background => return Ok((None, None)),
                None => Supply::Keyboard,
            }
        };
        let (ours, theirs) = taaj::channel_create()?;
        let feed =
            Feed { to: ours, from, offset: 0, message: [0; abi::MESSAGE_MAX as usize], length: 0, last: false, wanted: false };
        Ok((Some(theirs), Some(feed)))
    }

    /// Asks init to start `name`, then waits for it - a foreground job.
    /// A file of that name is read and sent along; otherwise, for `run`
    /// (`explicit`), init looks among the programs built into the kernel
    /// image.
    ///
    /// `run NAME` says how the program ended; a program run by its name
    /// alone is a command, and says nothing unless it failed.
    fn run_program(&self, name: &str, arguments: &[String], explicit: bool, grant: Grant) {
        let image = match self.program_image(name) {
            Ok(None) if !explicit => {
                self.failed.set(true);
                return tell!(self, "unknown command: {name} (try help)\n");
            }
            Ok(image) => image,
            Err(why) => {
                self.failed.set(true);
                return tell!(self, "run: {name} {why}\n");
            }
        };
        // Standard output and input. With no redirection: the screen,
        // as the shell has it, and no input. With one: a new channel,
        // one end for the program and the other kept here, where
        // `wait_for` moves the bytes between it and the file.
        let (stdout, output, stdin, feed) = match (self.output_end(), self.input_end()) {
            (Ok((stdout, output)), Ok((stdin, feed))) => (stdout, output, stdin, feed),
            (output, input) => {
                tell!(self, "run: {name}: no channel for its output or input\n");
                // Whichever was made is let go of.
                let output = output.ok().into_iter().flat_map(|(theirs, ours)| match theirs {
                    Output::Channel(theirs) => [Some(theirs), ours],
                    Output::Screen => [None, ours],
                });
                let input = input.ok().into_iter().flat_map(|(theirs, feed)| [theirs, feed.map(|feed| feed.to)]);
                return close_all(output.chain(input).flatten().chain(image));
            }
        };
        let keyboard = matches!(&feed, Some(Feed { from: Supply::Keyboard, .. }));
        let Some(process) = self.start(name, arguments, image, grant, stdout, Stdin { channel: stdin, keyboard }) else {
            self.failed.set(true);
            return close_all(output.into_iter().chain(feed.map(|feed| feed.to)));
        };
        if self.redirection.borrow().background {
            return self.background(alloc::vec![String::from(name)], alloc::vec![process], output, feed);
        }
        let code = self.wait_for(&[process], output, feed).map(|codes| codes[0]);
        self.report(name, code, explicit);
        let _ = taaj::close(process);
    }

    /// Says how a program ended, if that needs saying. It is the
    /// shell's to say, and not part of what the program printed: it
    /// goes to the user even when the output goes to a file.
    fn report(&self, name: &str, code: Result<i64, Error>, explicit: bool) {
        if !matches!(code, Ok(0) | Ok(abi::NO_READER)) {
            self.failed.set(true);
        }
        match code {
            Ok(abi::KILLED) => tell!(self, "{name} was killed by the kernel (it broke a rule)\n"),
            Ok(abi::STOPPED) => tell!(self, "{name} was stopped\n"),
            // Ctrl+C: stopped, as far as the person who pressed it knows.
            Ok(code) if abi::signal::of_exit_code(code) == Some(abi::signal::SIGINT) => tell!(self, "{name} was stopped\n"),
            Ok(code) if let Some(signal) = abi::signal::of_exit_code(code) => {
                tell!(self, "{name} was ended by signal {signal} ({})\n", abi::signal::name(signal))
            }
            // Its reader went away and it stopped writing: in `yes |
            // given` that is how things are meant to end.
            Ok(abi::NO_READER) => {}
            Ok(0) if !explicit => {}
            Ok(code) => tell!(self, "{name} exited with code {code}\n"),
            Err(error) => tell!(self, "lost track of {name}: {error:?}\n"),
        }
    }

    /// Asks init to start `name`, and returns the new process - not
    /// waiting for it. `image` is the program, read from its file; with
    /// none, init looks among the programs built into the kernel image.
    /// The program gets `arguments`, `stdout` and `stdin` - and the
    /// files only if it is one of the listed programs, or the user said
    /// so (`grant`): a program gets what it needs, not what the shell
    /// happens to hold. A program that gets the files is also told the
    /// current directory; one that does not has no use for it.
    ///
    /// The image and the two channel ends are given away here whatever
    /// happens: if the program cannot be started, this says why and
    /// closes them.
    fn start(
        &self,
        name: &str,
        arguments: &[String],
        image: Option<Handle>,
        grant: Grant,
        stdout: Output,
        stdin: Stdin,
    ) -> Option<Handle> {
        let Stdin { channel: stdin, keyboard } = stdin;
        let on_screen = matches!(stdout, Output::Screen);
        let stdout = match stdout {
            Output::Screen => Self::delegate(self.screen),
            Output::Channel(handle) => Some(handle),
        };
        // The arguments as they travel: each followed by a zero byte,
        // so the program gets them divided as they are here.
        let mut encoded = [0u8; 2 * LINE_MAX];
        let Some(length) = abi::arguments::encode(arguments.iter().map(String::as_str), &mut encoded) else {
            close_all([image, stdout, stdin].into_iter().flatten());
            tell!(self, "run: {name}: the arguments are too long\n");
            return None;
        };
        let arguments = &encoded[..length];
        // The program's services: the names that will exist for it,
        // and what each means here. Every program that uses files asks
        // for "files"; what it gets by that name is decided on this
        // line - the channel that can change them, or the one that can
        // only read. (The files go only to a program from a file: "ls"
        // built into the kernel image would be another program of the
        // same name.)
        let on_disk = |list: &[&str]| image.is_some() && list.contains(&name);
        let names: &[(&str, &str)] = match grant {
            // With the files, /tmp (J64): to change if the files may be
            // changed, to read if not; and /dev (J65), whose devices are
            // the same for both - writing to /dev/null changes nothing.
            Grant::Files => &[("files", "files"), ("tmp", "tmp"), ("dev", "dev")],
            Grant::Programs => &[("files", "files"), ("tmp", "tmp"), ("dev", "dev"), ("init", "init")],
            Grant::ReadOnly => &[("files", "files.look"), ("tmp", "tmp.look"), ("dev", "dev")],
            Grant::Usual if on_disk(CHANGE_PROGRAMS) => &[("files", "files"), ("tmp", "tmp"), ("dev", "dev")],
            Grant::Usual if on_disk(LOOK_PROGRAMS) => &[("files", "files.look"), ("tmp", "tmp.look"), ("dev", "dev")],
            Grant::Usual if on_disk(CARD_PROGRAMS) => &[("nic", "nic")],
            Grant::Usual if on_disk(NET_PROGRAMS) => &[("net", "net")],
            Grant::Usual if on_disk(WEB_PROGRAMS) => &[("net", "net"), ("files", "files.look"), ("tmp", "tmp.look"), ("dev", "dev")],
            Grant::Usual if on_disk(WINDOW_PROGRAMS) => &[("windows", "windows")],
            Grant::Usual if on_disk(EDIT_PROGRAMS) => &[("windows", "windows"), ("files", "files"), ("tmp", "tmp"), ("dev", "dev")],
            Grant::Usual => &[],
        };
        // The registry makes a channel on which only those names exist,
        // and the program gets it. With no names there is nothing to
        // ask for, and no channel.
        let services = match names {
            [] => None,
            names => match self.services.narrow(&label(name), names) {
                Ok(channel) => Some(channel),
                Err(error) => {
                    close_all([image, stdout, stdin].into_iter().flatten());
                    tell!(self, "run: {name}: it cannot be given its services: {}\n", error.describe());
                    return None;
                }
            },
        };
        let mut handles = [0u64; 5];
        let mut count = 0;
        // SCREEN brings no handle: it says what the output is.
        let mut flags = if on_screen && stdout.is_some() { abi::startup::SCREEN as u8 } else { 0 };
        // Nor does KEYBOARD: it says what the input is - one that
        // sends nothing until the program asks.
        if keyboard && stdin.is_some() {
            flags |= abi::startup::KEYBOARD as u8;
        }
        // In the order of the flags' bits, which is the order init
        // expects the handles in.
        let offered = [
            (0, image),
            (abi::startup::STDOUT, stdout),
            (abi::startup::SERVICES, services),
            (abi::startup::STDIN, stdin),
            // Standard error is the screen, wherever the output goes:
            // a complaint is for whoever is watching, not for the file
            // or the next program.
            (abi::startup::STDERR, Self::delegate(self.screen)),
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
        let directory = if names.is_empty() { String::new() } else { self.directory.borrow().clone() };
        // The program's file, which it is told (as Linux's programs read
        // /proc/self/exe): clang finds its headers from it. A program
        // built into the kernel image has none.
        let file = if image.is_some() { alloc::format!("/{}", self.program_path(name)) } else { String::new() };
        let request = abi::init::Run {
            kind: if image.is_some() { abi::init::IMAGE } else { abi::init::BUILT_IN },
            flags,
            name,
            directory: &directory,
            file: &file,
            arguments,
        };
        let mut message = [0u8; abi::MESSAGE_MAX as usize];
        let sent = match (self.init, request.encode(&mut message)) {
            (Some(init), Some(length)) => taaj::send(init, &message[..length], handles).map(|()| init),
            _ => Err(Error::InvalidArgument),
        };
        let Ok(init) = sent else {
            close_all(handles.iter().copied()); // not sent: still ours
            self.tell(b"cannot reach init: cannot run programs\n");
            return None;
        };
        let mut reply = [0u8; 64];
        let mut handle = [0u64; 1];
        match taaj::receive(init, &mut reply, &mut handle) {
            Ok((2, 1)) if &reply[..2] == b"ok" => Some(handle[0]),
            Ok((n, _)) => {
                self.tell(&reply[..n]);
                self.tell(b"\n");
                None
            }
            Err(error) => {
                tell!(self, "no answer from init: {error:?}\n");
                None
            }
        }
    }
}

impl Shell {
    /// Waits for the foreground programs to end - one, or the several
    /// of a pipeline - and returns their exit codes, in their order.
    /// They become the foreground job, and `attend` does the rest:
    /// Ctrl+C stops them, other keys are kept, and their output and
    /// input - if the line redirected them - are moved while they run.
    /// The process handles stay the caller's.
    fn wait_for(&self, processes: &[Handle], output: Option<Handle>, feed: Option<Feed>) -> Result<Vec<i64>, Error> {
        self.jobs.borrow_mut().push(Job {
            number: 0,
            text: String::new(),
            names: Vec::new(),
            processes: processes.to_vec(),
            codes: alloc::vec![None; processes.len()],
            output,
            sink: None,
            feed,
            failed: None,
        });
        match self.attend(Until::Foreground, &mut [0u8; 16]) {
            Ok(Attended::Ended(codes)) => Ok(codes),
            result => {
                // The wait itself failed: the job is given up.
                let mut jobs = self.jobs.borrow_mut();
                if let Some(at) = jobs.iter().position(|job| job.number == 0) {
                    let job = jobs.remove(at);
                    close_all(job.output.into_iter().chain(job.feed.map(|feed| feed.to)));
                }
                Err(result.err().unwrap_or(Error::InvalidArgument))
            }
        }
    }

    /// Makes what was just started a background job: says its number,
    /// and does not wait. From here on `attend` looks after it, and
    /// the job owns the process handles.
    fn background(&self, names: Vec<String>, processes: Vec<Handle>, output: Option<Handle>, feed: Option<Feed>) {
        let (text, sink) = {
            let redirection = self.redirection.borrow();
            let sink = match &redirection.output {
                Some(Sink::File(path)) => Some(path.clone()),
                _ => None,
            };
            (redirection.text.clone(), sink)
        };
        let mut jobs = self.jobs.borrow_mut();
        // One more than the highest in use: a number is not given to
        // another job while the user may still be thinking of the old.
        let number = jobs.iter().map(|job| job.number).max().unwrap_or(0) + 1;
        tell!(self, "[{number}] {text}\n");
        let codes = alloc::vec![None; processes.len()];
        jobs.push(Job { number, text, names, processes, codes, output, sink, feed, failed: None });
    }

    /// The shell's one place of waiting. It looks after every job -
    /// noting programs that end, adding output to files, sending input
    /// - until what the caller is waiting for happens:
    ///
    ///   - `Foreground`: the foreground job being over. Ctrl+C asks
    ///     the kernel to end its programs, and the wait goes on until
    ///     they have ended; any other key is kept for later.
    ///   - `Key`: a key being typed, which is put in `input`.
    ///   - `Jobs`: every background job being over. Ctrl+C ends the
    ///     wait - not the jobs; any other key is kept for later.
    ///
    /// The shell is the far end of every redirected output and input,
    /// so this must keep running for as long as programs do - a
    /// program whose output nobody takes waits for ever with a full
    /// channel. That is why there is one loop and not two: a
    /// background job must be served while a foreground one runs, and
    /// while the shell is idle at its prompt.
    fn attend(&self, until: Until, input: &mut [u8; 16]) -> Result<Attended, Error> {
        #[derive(Clone, Copy)]
        enum Source {
            Keys,
            /// This job (by its place in the list), this program of it.
            Process(usize, usize),
            /// This job's output channel.
            Output(usize),
            /// This job's input channel, when its input is the
            /// keyboard: where the program says it wants a line.
            Asking(usize),
        }
        let mut keys = Some(self.keys);
        loop {
            // The list is borrowed for one turn of the loop. Nothing
            // called below looks at it: they are given the job.
            let mut jobs = self.jobs.borrow_mut();
            for job in jobs.iter_mut() {
                self.settle(job);
            }
            if until == Until::Jobs && jobs.iter().all(|job| job.over()) {
                return Ok(Attended::Ended(Vec::new()));
            }
            if until == Until::Foreground {
                if let Some(at) = jobs.iter().position(|job| job.number == 0 && job.over()) {
                    // `flatten` drops the Nones - there are none: it
                    // is over.
                    return Ok(Attended::Ended(jobs.remove(at).codes.into_iter().flatten().collect()));
                }
            }
            // Send as much of each input as its channel will take now.
            // False: all of it has gone, or it cannot go on - closing
            // our end is what tells the program its input has ended.
            let mut feeding = false;
            for job in jobs.iter_mut() {
                if let Some(sending) = &mut job.feed {
                    if matches!(sending.from, Supply::Keyboard) {
                        // Nothing to hurry for: a line goes when it has
                        // been typed, and keys wake this loop.
                        if !self.type_to(sending) {
                            let _ = taaj::close(sending.to);
                            job.feed = None;
                        }
                    } else if self.feed(sending) {
                        feeding = true;
                    } else {
                        let _ = taaj::close(sending.to);
                        job.feed = None;
                    }
                }
            }
            // What to wait on: the keyboard while there is one; every
            // program still running; every output channel still open.
            let mut handles = Vec::new();
            let mut sources = Vec::new();
            let mut watch = |handle: Handle, source: Source| {
                handles.push(handle);
                sources.push(source);
            };
            if let Some(keys) = keys {
                watch(keys, Source::Keys);
            }
            for (at, job) in jobs.iter().enumerate() {
                for (n, &process) in job.processes.iter().enumerate() {
                    if job.codes[n].is_none() {
                        watch(process, Source::Process(at, n));
                    }
                }
                if let Some(output) = job.output {
                    watch(output, Source::Output(at));
                }
                if let Some(Feed { to, from: Supply::Keyboard, .. }) = &job.feed {
                    watch(*to, Source::Asking(at));
                }
            }
            // With input still to send, the wait is short: there is no
            // way to wait for room in a channel, so the shell looks
            // again a tick later.
            let ready = if feeding {
                taaj::object_wait_until(&handles, taaj::ticks() + 1)?
            } else {
                Some(taaj::object_wait(&handles)?)
            };
            match ready.map(|index| sources[index]) {
                None => {} // time to try the input again
                Some(Source::Process(at, n)) => {
                    let job = &mut jobs[at];
                    job.codes[n] = Some(taaj::wait_any(&[job.processes[n]])?.1);
                }
                Some(Source::Output(at)) => {
                    let job = &mut jobs[at];
                    if let Some(channel) = job.output {
                        if !self.take_output(channel, &job.sink, &mut job.failed) {
                            let _ = taaj::close(channel);
                            job.output = None;
                        }
                    }
                }
                Some(Source::Asking(at)) => {
                    let job = &mut jobs[at];
                    if let Some(asking) = &mut job.feed {
                        let mut message = [0u8; 16];
                        match taaj::receive(asking.to, &mut message, &mut []) {
                            // From here on, what is typed is the
                            // program's - starting with whatever was
                            // typed before it asked (`type_to`, at the
                            // top of the loop).
                            Ok((1, _)) if message[0] == abi::stream::WANT => asking.wanted = true,
                            Ok(_) => {}
                            // The program has let go of its input.
                            Err(_) => {
                                let _ = taaj::close(asking.to);
                                job.feed = None;
                                self.typing.borrow_mut().clear();
                            }
                        }
                    }
                }
                Some(Source::Keys) => match taaj::receive(self.keys, input, &mut []) {
                    Ok((n, _)) if until == Until::Key => return Ok(Attended::Key(n)),
                    Ok((n, _)) if &input[..n] == CTRL_C => {
                        self.tell(b"^C\n");
                        if until == Until::Jobs {
                            return Ok(Attended::Ended(Vec::new()));
                        }
                        // What was typed before it and not yet read
                        // was meant for the program being stopped,
                        // not for the shell's next line.
                        self.typed_ahead.borrow_mut().clear();
                        self.typing.borrow_mut().clear();
                        // The foreground job only: a background job is
                        // ended by name, with `kill`.
                        for job in jobs.iter().filter(|job| job.number == 0) {
                            for (n, &process) in job.processes.iter().enumerate() {
                                // One that ended a moment ago, and has
                                // not been noticed here yet, cannot be
                                // stopped: nothing to say about that.
                                // SIGINT, which a program may catch - to
                                // tidy up before it ends, or not to end.
                                // One that does not catch it ends as if
                                // killed (`kill` below always kills).
                                if job.codes[n].is_none() {
                                    let _ = taaj::signal_send(process, abi::signal::SIGINT);
                                }
                            }
                        }
                    }
                    Ok((n, _)) => {
                        let mut typed = self.typed_ahead.borrow_mut();
                        if typed.len() < TYPED_AHEAD_MAX {
                            typed.push_back(input[..n].to_vec());
                        }
                    }
                    Err(error) if until == Until::Key => return Err(error),
                    // No keyboard: nothing to watch there any more.
                    Err(_) => keys = None,
                },
            }
        }
    }

    /// If every program of the job has ended, finishes with it: what
    /// was printed last may still be in the channel, so that is taken
    /// first - only what is there already, since nothing more can come
    /// - and then the shell's ends are closed.
    fn settle(&self, job: &mut Job) {
        if !job.ended() {
            return;
        }
        if let Some(channel) = job.output.take() {
            while let Ok(Some(_)) = taaj::poll(&[channel]) {
                if !self.take_output(channel, &job.sink, &mut job.failed) {
                    break;
                }
            }
            let _ = taaj::close(channel);
        }
        if let Some(feed) = job.feed.take() {
            let _ = taaj::close(feed.to);
            // Half a line typed to a program that has ended is nobody's.
            if matches!(feed.from, Supply::Keyboard) {
                self.typing.borrow_mut().clear();
            }
        }
    }

    /// Gives the keyboard to a program that has asked for input: the
    /// keys typed - before it asked, and since - go into a line, shown
    /// as they are typed, and Enter sends the line. Then the program
    /// has what it asked for, and what is typed next waits again, for
    /// whoever reads next.
    ///
    /// This is the shell doing for a program what it does for itself
    /// at its prompt: the program never sees a key, a Backspace or a
    /// half-typed line - only whole lines. (Unix has the same division,
    /// with the kernel's terminal driver in the shell's place.)
    ///
    /// Ctrl+D ends the input: at the start of a line the channel is
    /// closed, which is how any input ends - false is returned for
    /// that, and when the program is no longer there to be sent to. In
    /// the middle of a line it sends what has been typed, with no new
    /// line after it.
    fn type_to(&self, feed: &mut Feed) -> bool {
        while feed.wanted {
            let Some(key) = self.typed_ahead.borrow_mut().pop_front() else { break };
            let mut typing = self.typing.borrow_mut();
            let end: &[u8] = match key.as_slice() {
                b"\n" => {
                    self.tell(b"\n");
                    b"\n"
                }
                CTRL_D if typing.bytes().is_empty() => return false,
                CTRL_D => b"",
                key => {
                    typing.key(key, |shown| self.tell(shown));
                    continue;
                }
            };
            let line = typing.bytes();
            let length = 1 + line.len() + end.len();
            feed.message[0] = abi::stream::DATA;
            feed.message[1..=line.len()].copy_from_slice(line);
            feed.message[1 + line.len()..length].copy_from_slice(end);
            typing.clear();
            feed.wanted = false;
            if taaj::send(feed.to, &feed.message[..length], &[]).is_err() {
                return false;
            }
        }
        true
    }

    /// Says which background jobs are over, and forgets them.
    fn report_jobs(&self) {
        let over: Vec<Job> = {
            let mut jobs = self.jobs.borrow_mut();
            for job in jobs.iter_mut() {
                self.settle(job);
            }
            // `partition` divides one list into two by a test; the
            // ones still going are put back.
            let (over, going): (Vec<Job>, Vec<Job>) = jobs.drain(..).partition(|job| job.number != 0 && job.over());
            *jobs = going;
            over
        };
        for job in over {
            tell!(self, "[{}] {}  {}\n", job.number, job.ending(), job.text);
            if let (Some(path), Some(error)) = (&job.sink, job.failed) {
                tell!(self, "[{}] /{path}: {}\n", job.number, error.describe());
            }
            close_all(job.processes);
        }
    }

    /// `jobs`: the background jobs, by number.
    fn list_jobs(&self) {
        // The lines are made first and written after: `write` must not
        // run while the list is borrowed, if only by habit.
        let lines: Vec<String> = self
            .jobs
            .borrow()
            .iter()
            .filter(|job| job.number != 0)
            .map(|job| alloc::format!("[{}] {}  {}\n", job.number, if job.ended() { "ended" } else { "running" }, job.text))
            .collect();
        if lines.is_empty() {
            return self.write(b"no jobs\n");
        }
        for line in lines {
            self.write(line.as_bytes());
        }
    }

    /// `kill N`: ends every program of background job N still running,
    /// and waits until each has ended - which is at once: the kernel
    /// ends a program wherever it is. The next prompt says the job
    /// was stopped.
    fn kill_job(&self, number: usize) {
        let mut jobs = self.jobs.borrow_mut();
        let Some(job) = jobs.iter_mut().find(|job| job.number == number) else {
            drop(jobs);
            return say!(self, "kill: no job {number} (see jobs)\n");
        };
        for (n, &process) in job.processes.iter().enumerate() {
            if job.codes[n].is_some() {
                continue;
            }
            // A kill that fails found the program already ended; the
            // wait then returns at once, with how it did end.
            let _ = taaj::kill(process);
            if let Ok((_, code)) = taaj::wait_any(&[process]) {
                job.codes[n] = Some(code);
            }
        }
    }

    /// Takes one message of a program's output and puts it where it
    /// goes: added to the file `sink`, or - with none, for the
    /// foreground job - wherever the line being run says. False when
    /// there will be no more: the program closed its end, and
    /// everything it sent has been taken.
    fn take_output(&self, channel: Handle, sink: &Option<String>, failed: &mut Option<FileError>) -> bool {
        let mut message = [0u8; abi::MESSAGE_MAX as usize];
        match taaj::receive(channel, &mut message, &mut []) {
            Ok((length, _)) if length > 1 && message[0] == abi::stream::DATA => {
                let bytes = &message[1..length];
                match (sink, &self.files) {
                    (None, _) => self.write(bytes),
                    // After the first failure nothing more is written:
                    // a file with a hole in the middle would look whole.
                    (Some(_), _) if failed.is_some() => {}
                    (Some(path), Some(files)) => *failed = files.append(path, bytes).err(),
                    (Some(_), None) => *failed = Some(FileError::Unreachable(Error::PeerClosed)),
                }
                true
            }
            Ok(_) => true, // empty, or not part of the stream
            Err(_) => false,
        }
    }

    /// Sends the next pieces of the input to the program, as many as
    /// its channel has room for. True if there is more to send later;
    /// false when all has been sent, the file cannot be read, or the
    /// program has stopped reading.
    fn feed(&self, feed: &mut Feed) -> bool {
        loop {
            if feed.length == 0 {
                // The next piece, into `message`. The fields of `feed`
                // are borrowed one by one - `from` to read, `message`
                // to write - which is allowed, since they are apart.
                let message = &mut feed.message;
                message[0] = abi::stream::DATA;
                let taken = match &feed.from {
                    Supply::File(path) => {
                        let Some(files) = self.look.as_ref().or(self.files.as_ref()) else { return false };
                        let read = files.read_from(path, feed.offset, |piece| message[1..=piece.len()].copy_from_slice(piece));
                        match read {
                            Ok(n) => (n, n < fileserver::protocol::READ_MAX),
                            Err(error) => {
                                tell!(self, "the input stopped: {}\n", error.describe());
                                return false;
                            }
                        }
                    }
                    // Typed, not read from anywhere: `type_to`.
                    Supply::Keyboard => return true,
                    Supply::Memory(bytes) => {
                        let at = feed.offset as usize;
                        let piece = &bytes[at..bytes.len().min(at + abi::stream::DATA_MAX)];
                        message[1..=piece.len()].copy_from_slice(piece);
                        (piece.len(), at + piece.len() == bytes.len())
                    }
                };
                match taken {
                    (0, _) => return false, // it ended on a boundary, or was empty
                    (n, last) => {
                        feed.length = n + 1;
                        feed.offset += n as u64;
                        feed.last = last;
                    }
                }
            }
            match taaj::send(feed.to, &feed.message[..feed.length], &[]) {
                Ok(()) if feed.last => return false,
                Ok(()) => feed.length = 0,
                Err(Error::QueueFull) => return true, // it has not read what it has: later
                Err(_) => return false,               // it is not reading any more
            }
        }
    }

    /// The next key: one typed while a program ran, if any are left,
    /// else one from the keyboard driver - waiting for it, and looking
    /// after the background jobs meanwhile.
    fn next_key(&self, input: &mut [u8; 16]) -> Result<usize, Error> {
        if let Some(key) = self.typed_ahead.borrow_mut().pop_front() {
            input[..key.len()].copy_from_slice(&key);
            return Ok(key.len());
        }
        match self.attend(Until::Key, input)? {
            Attended::Key(n) => Ok(n),
            Attended::Ended(_) => Err(Error::InvalidArgument), // not asked for
        }
    }
}

/// The name of a command of a pipeline. Every one has a name: `parse`
/// refuses a pipeline with an empty side.
fn name(command: &Command) -> &str {
    command.words.first().map_or("", String::as_str)
}

/// A program's name as a label for the registry's log: the last part
/// of a path, with whatever a label may not hold made a '-'.
fn label(name: &str) -> String {
    let last = name.rsplit('/').next().unwrap_or(name);
    let label: String = last
        .chars()
        .take(registry::protocol::NAME_MAX)
        .map(|c| if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '-' { c } else { '-' })
        .collect();
    if label.is_empty() { String::from("program") } else { label }
}

/// Closes handles the shell has no more use for.
fn close_all(handles: impl IntoIterator<Item = Handle>) {
    for handle in handles {
        let _ = taaj::close(handle);
    }
}

/// `files` with each service given mounted where it goes - /tmp's
/// (J64), /dev's (J65) - if there is one.
fn with_mounts(files: Files, mounts: [(&str, Option<Handle>); 2]) -> Files {
    mounts.into_iter().fold(files, |files, (at, service)| match service.and_then(|service| taaj::files::File::from_handle(service).ok()) {
        Some(service) => files.mounted(at, service).unwrap_or(files),
        None => files,
    })
}

fn main() -> i64 {
    // Everything the shell works with, asked for by name. Only the
    // keyboard is needed: without the others there is less it can do,
    // and it says so when asked to.
    let services = Services::new(taaj::bootstrap());
    let service = |name: &str| services.lookup(name).ok();
    let keys = match services.lookup("keys") {
        Ok(keys) => keys,
        Err(error) => {
            println!("[shell] no keyboard: {}", error.describe());
            return 1;
        }
    };
    let shell = Shell {
        keys,
        typed_ahead: RefCell::new(VecDeque::new()),
        typing: RefCell::new(Typing::new()),
        screen: service("display"),
        init: service("init"),
        system: service("system"),
        // Each with /tmp mounted (J64), to change or to read, and /dev
        // (J65), as the programs it starts have them: `cd /tmp`, `ls
        // /tmp`, `> /dev/null` here mean what they mean there.
        files: service("files").map(|files| with_mounts(Files::new(files), [("/tmp", service("tmp")), ("/dev", service("dev"))])),
        disk: service("disk.look").map(Disk::new),
        look: service("files.look").map(|look| with_mounts(Files::new(look), [("/tmp", service("tmp.look")), ("/dev", service("dev"))])),
        directory: RefCell::new(String::new()),
        redirection: RefCell::new(Redirection::default()),
        jobs: RefCell::new(Vec::new()),
        failed: Cell::new(false),
        sourcing: Cell::new(0),
        // Last: the lookups above borrow it, and this moves it.
        services,
    };
    println!("[shell] ready - type a command (try help)");
    shell.screen_only(b"Welcome to TheTaaJ. Type help for the commands.\n\n");
    shell.prompt();

    let mut line = Typing::new();
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
                shell.tell(b"\n");
                let text = core::str::from_utf8(line.bytes()).unwrap_or("(not UTF-8)");
                println!("[shell] line: {text:?} (process {})", taaj::process_id());
                shell.run(text);
                println!("[shell] ran {text:?}");
                shell.prompt();
                line.clear();
            }
            CTRL_C => {
                // Abandon the line.
                shell.tell(b"^C\n");
                shell.prompt();
                line.clear();
            }
            key => line.key(key, |shown| shell.tell(shown)),
        }
    }
}
