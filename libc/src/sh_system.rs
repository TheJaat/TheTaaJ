//! The shell (sh.rs) on TheTaaJ: what it asks of a system, done with
//! this library - programs started as posix_spawn starts them
//! (spawn.rs `start_jobs`), wildcards matched by `glob` (unix.c), the
//! current directory and the files as the rest of the library has them.
//! And `__taaj_sh`, the shell program's whole `main` (programs/sh.c):
//!
//!   sh [-ex] -c COMMAND [NAME [ARGUMENT...]]    runs COMMAND; $0 NAME
//!   sh [-ex] FILE [ARGUMENT...]                 runs the script in FILE
//!
//! A builtin or function in a pipeline, a compound command that is
//! (`for ...; done | wc`), a subshell and a command substitution are
//! run by another copy of this program - the file it was started from
//! (`__taaj_executable`) - as a Unix shell forks one: `sh -c '"$@"' sh
//! echo hi`, the functions defined first. Its output goes where the
//! redirections say, with nobody here to copy it.
//!
//! A builtin, function or compound command with redirections, alone,
//! runs here: this program's own descriptors are changed with dup2 and
//! put back after, from copies kept out of the way (10 and up, marked
//! close-on-exec so that no program started meanwhile gets them).
//!
//! A here-document is a pipe, its text written into it before the
//! command starts and the write end closed: the reader sees the text,
//! then the end. So it must fit in what a pipe holds unread
//! (abi::CHANNEL_QUEUE_MAX messages) - a longer one is refused rather
//! than left waiting for a reader that has not started.

use crate::sh::{Command, Kind, Redirection, Shell, System};
use crate::spawn::{close_fd, output_flags, reap, start_jobs, Action, Failed, Job, INPUT_FLAGS, READ_WRITE_FLAGS};
use alloc::ffi::CString;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::ffi::{c_char, c_int, CStr};

/// TheTaaJ, as the shell sees it.
struct TheTaaJ {
    /// This program's own file, from the root: "/sh".
    myself: String,
}

/// An environment's strings, as a Run request has them.
fn environment_bytes(environment: &[String]) -> Vec<u8> {
    let mut out = Vec::new();
    for entry in environment {
        out.extend_from_slice(entry.as_bytes());
        out.push(0);
    }
    out
}

/// What waitpid says, as `$?` says it.
fn status_of(waited: c_int) -> i32 {
    if waited & 0x7f == 0 {
        (waited >> 8) & 0xff
    } else {
        128 + (waited & 0x7f)
    }
}

/// open's flags, as fcntl.h has them.
const O_RDONLY: c_int = 0;
const O_WRONLY: c_int = 1;
const O_CREAT: c_int = 0o100;
const O_TRUNC: c_int = 0o1000;
const O_APPEND: c_int = 0o2000;
/// fcntl's: a copy at the lowest free descriptor from the argument on,
/// close-on-exec.
const F_DUPFD_CLOEXEC: c_int = 1030;
/// Where the copies that put this shell's descriptors back are kept:
/// above what scripts use (`exec 3<`... up to 9).
const SAVED_FROM: c_int = 10;

/// What the last call that failed said, as text.
fn why() -> &'static str {
    let code = crate::errno::errno.load(core::sync::atomic::Ordering::Relaxed);
    crate::errno::message(code).trim_end_matches('\0')
}

/// `path` opened with `flags`: its descriptor, or why not.
fn open(path: &str, flags: c_int) -> Result<c_int, String> {
    let name = CString::new(path).map_err(|_| format!("{path}: a zero byte in the name"))?;
    // SAFETY: a string ending in a zero byte.
    let fd = unsafe { crate::descriptors::__taaj_open(name.as_ptr(), flags) };
    if fd < 0 {
        return Err(format!("{path}: {}", why()));
    }
    Ok(fd)
}

/// A pipe with `text` in it and its write end closed: the read end,
/// close-on-exec until dup2 makes it the descriptor meant.
fn here_document(text: &str) -> Result<c_int, String> {
    let room = abi::CHANNEL_QUEUE_MAX * abi::stream::DATA_MAX;
    if text.len() > room {
        return Err(format!("a here-document of {} bytes: more than a pipe holds ({room})", text.len()));
    }
    let mut ends = [0; 2];
    if crate::pipes::private_pipe(&mut ends) != 0 {
        return Err(String::from("no descriptor for a here-document's pipe"));
    }
    let mut rest = text.as_bytes();
    while !rest.is_empty() {
        // SAFETY: the bytes of a live string.
        let wrote = unsafe { crate::descriptors::write(ends[1], rest.as_ptr().cast(), rest.len()) };
        if wrote <= 0 {
            break;
        }
        rest = &rest[wrote as usize..];
    }
    crate::descriptors::close(ends[1]);
    Ok(ends[0])
}

/// Makes `fd` what `from` is, and closes `from`.
fn move_to(from: c_int, fd: c_int) {
    if from != fd {
        crate::descriptors::dup2(from, fd);
        crate::descriptors::close(from);
    }
}

impl TheTaaJ {
    /// What starts `command`. The read ends of its here-documents'
    /// pipes are added to `heres`: the caller closes them once it has
    /// started.
    fn job(&self, command: &Command, heres: &mut Vec<c_int>) -> Result<Job, String> {
        let mut actions = Vec::new();
        for redirection in &command.redirections {
            actions.push(match redirection {
                Redirection::In { fd, path } => Action::Open { descriptor: *fd as c_int, path: path.clone(), flags: INPUT_FLAGS },
                Redirection::Out { fd, path, append } => {
                    Action::Open { descriptor: *fd as c_int, path: path.clone(), flags: output_flags(*append) }
                }
                Redirection::ReadWrite { fd, path } => {
                    Action::Open { descriptor: *fd as c_int, path: path.clone(), flags: READ_WRITE_FLAGS }
                }
                Redirection::Copy { fd, to } => Action::Dup2 { from: *to as c_int, to: *fd as c_int },
                Redirection::Close { fd } => Action::Close(*fd as c_int),
                Redirection::Here { fd, text } => {
                    let read = here_document(text)?;
                    heres.push(read);
                    Action::Dup2 { from: read, to: *fd as c_int }
                }
            });
        }
        let environment = environment_bytes(&command.environment);
        if let Some(copy) = &command.shell {
            let script = format!("{}{}", copy.functions, copy.script);
            let mut words: Vec<String> = alloc::vec![String::from("sh"), String::from("-c"), script, copy.zero.clone()];
            words.extend(command.words.iter().cloned());
            Ok(Job { path: self.myself.clone(), words, actions, environment })
        } else {
            Ok(Job { path: command.words[0].clone(), words: command.words.clone(), actions, environment })
        }
    }

    /// The jobs for `commands`, and their here-documents' read ends;
    /// or why not, said, and nothing left open.
    fn jobs(&mut self, commands: &[Command]) -> Option<(Vec<Job>, Vec<c_int>)> {
        let mut heres = Vec::new();
        let mut jobs = Vec::new();
        for command in commands {
            match self.job(command, &mut heres) {
                Ok(job) => jobs.push(job),
                Err(why) => {
                    heres.into_iter().for_each(|fd| close_fd(Some(fd)));
                    self.write(2, &format!("sh: {why}\n"));
                    return None;
                }
            }
        }
        Some((jobs, heres))
    }

    /// Does one redirection to this program's own descriptors.
    fn apply(&mut self, redirection: &Redirection) -> Result<(), String> {
        let fd = redirection.fd() as c_int;
        match redirection {
            Redirection::In { path, .. } => move_to(open(path, O_RDONLY)?, fd),
            Redirection::Out { path, append, .. } => {
                // Always appending, the file emptied first for `>`: a
                // program started with this as its output adds to the
                // end of the file (spawn.rs), and what this shell writes
                // after must come after that, not over it.
                if !append {
                    crate::descriptors::close(open(path, O_WRONLY | O_CREAT | O_TRUNC)?);
                }
                move_to(open(path, O_WRONLY | O_CREAT | O_APPEND)?, fd);
            }
            Redirection::ReadWrite { path, .. } => move_to(open(path, READ_WRITE_FLAGS)?, fd),
            Redirection::Copy { to, .. } => {
                if crate::descriptors::dup2(*to as c_int, fd) < 0 {
                    return Err(format!("{to}: not an open descriptor"));
                }
            }
            Redirection::Close { .. } => {
                crate::descriptors::close(fd);
            }
            Redirection::Here { text, .. } => move_to(here_document(text)?, fd),
        }
        Ok(())
    }
}

impl System for TheTaaJ {
    /// Each descriptor changed, with a copy of what it was (-1 if it was
    /// not open).
    type Saved = Vec<(c_int, c_int)>;

    fn redirect(&mut self, redirections: &[Redirection], save: bool) -> Result<Self::Saved, String> {
        let mut saved = Vec::new();
        for redirection in redirections {
            let fd = redirection.fd() as c_int;
            if save && !saved.iter().any(|&(n, _)| n == fd) {
                saved.push((fd, crate::descriptors::__taaj_fcntl(fd, F_DUPFD_CLOEXEC, SAVED_FROM.into())));
            }
            if let Err(why) = self.apply(redirection) {
                self.restore(saved);
                return Err(why);
            }
        }
        Ok(saved)
    }

    fn restore(&mut self, saved: Self::Saved) {
        for (fd, copy) in saved.into_iter().rev() {
            if copy < 0 {
                crate::descriptors::close(fd);
            } else {
                move_to(copy, fd);
            }
        }
    }

    fn pipeline(&mut self, commands: &[Command]) -> i32 {
        let Some((jobs, heres)) = self.jobs(commands) else { return 1 };
        let started = start_jobs(&jobs, None, None, "sh");
        heres.into_iter().for_each(|fd| close_fd(Some(fd)));
        match started {
            Ok(ids) => ids.into_iter().fold(0, |_, id| status_of(reap(id))),
            Err(Failed::NotStarted(code)) if code == crate::errno::ENOEXEC => 126,
            Err(Failed::NotStarted(_)) => 127,
            // Not given the right to start programs, or no descriptor
            // for a pipe: said already.
            Err(Failed::Refused(_)) => 126,
        }
    }

    fn capture(&mut self, command: &Command) -> (String, i32) {
        let mut ends = [0; 2];
        if crate::pipes::private_pipe(&mut ends) != 0 {
            self.write(2, "sh: no descriptor for a command substitution's pipe\n");
            return (String::new(), 126);
        }
        let Some((jobs, heres)) = self.jobs(core::slice::from_ref(command)) else {
            close_fd(Some(ends[0]));
            close_fd(Some(ends[1]));
            return (String::new(), 1);
        };
        let started = start_jobs(&jobs, None, Some(ends[1]), "sh");
        heres.into_iter().for_each(|fd| close_fd(Some(fd)));
        let mut output = Vec::new();
        if started.is_ok() {
            let mut buffer = [0u8; 4096];
            loop {
                // SAFETY: a buffer of our own.
                let got = unsafe { crate::descriptors::read(ends[0], buffer.as_mut_ptr().cast(), buffer.len()) };
                if got <= 0 {
                    break;
                }
                output.extend_from_slice(&buffer[..got as usize]);
            }
        }
        close_fd(Some(ends[0]));
        let status = match started {
            Ok(ids) => ids.into_iter().fold(0, |_, id| status_of(reap(id))),
            Err(_) => 127,
        };
        (String::from_utf8_lossy(&output).into_owned(), status)
    }

    fn glob(&mut self, pattern: &str) -> Vec<String> {
        #[repr(C)]
        struct Glob {
            count: usize,
            paths: *mut *mut c_char,
            offsets: usize,
        }
        extern "C" {
            fn glob(pattern: *const c_char, flags: c_int, errors: *const u8, found: *mut Glob) -> c_int;
            fn globfree(found: *mut Glob);
        }
        let Ok(pattern) = CString::new(pattern) else { return Vec::new() };
        let mut found = Glob { count: 0, paths: core::ptr::null_mut(), offsets: 0 };
        // SAFETY: a string ending in a zero byte, and a glob_t of ours.
        let matched = unsafe { glob(pattern.as_ptr(), 0, core::ptr::null(), &mut found) };
        let mut out = Vec::new();
        if matched == 0 {
            for n in 0..found.count {
                // SAFETY: glob gave `count` strings.
                let path = unsafe { CStr::from_ptr(*found.paths.add(n)) };
                out.push(path.to_string_lossy().into_owned());
            }
        }
        if !found.paths.is_null() {
            // SAFETY: what glob made.
            unsafe { globfree(&mut found) };
        }
        out
    }

    fn chdir(&mut self, path: &str) -> Result<(), String> {
        match self.kind(path) {
            Some(Kind::Directory) => {}
            Some(Kind::File { .. }) => return Err(String::from("not a directory")),
            None => return Err(String::from("no such directory")),
        }
        let name = CString::new(path).map_err(|_| String::from("no such directory"))?;
        // SAFETY: a string ending in a zero byte.
        match unsafe { crate::status::chdir(name.as_ptr()) } {
            0 => Ok(()),
            _ => Err(String::from("cannot go there")),
        }
    }

    fn cwd(&mut self) -> String {
        crate::descriptors::with(|state| state.files().map(|files| format!("/{}", files.directory())))
            .unwrap_or_else(|| String::from("/"))
    }

    fn kind(&mut self, path: &str) -> Option<Kind> {
        crate::descriptors::with(|state| match state.files()?.stat(path).ok()? {
            (taaj::files::Kind::Directory, _) => Some(Kind::Directory),
            (taaj::files::Kind::File, size) => Some(Kind::File { size }),
        })
    }

    fn write(&mut self, fd: u32, text: &str) {
        let mut rest = text.as_bytes();
        while !rest.is_empty() {
            // SAFETY: the bytes of a live string.
            let wrote = unsafe { crate::descriptors::write(fd as c_int, rest.as_ptr().cast(), rest.len()) };
            if wrote <= 0 {
                return;
            }
            rest = &rest[wrote as usize..];
        }
    }

    fn read_line(&mut self) -> Option<String> {
        // A byte at a time: what follows the line is the next command's.
        let mut line = Vec::new();
        loop {
            let mut byte = 0u8;
            // SAFETY: one byte of our own.
            let got = unsafe { crate::descriptors::read(0, (&mut byte as *mut u8).cast(), 1) };
            if got <= 0 {
                return (!line.is_empty()).then(|| String::from_utf8_lossy(&line).into_owned());
            }
            if byte == b'\n' {
                return Some(String::from_utf8_lossy(&line).into_owned());
            }
            line.push(byte);
        }
    }

    fn pid(&self) -> i32 {
        crate::system::getpid()
    }
}

extern "C" {
    /// This program's environment (libc/c/unix.c).
    static environ: *const *const c_char;
}

/// The shell program: its arguments as main has them; its status.
///
/// # Safety
/// `argv` holds `argc` strings ending in zero bytes, as main's does.
#[no_mangle]
pub unsafe extern "C" fn __taaj_sh(argc: c_int, argv: *const *const c_char) -> c_int {
    let mut arguments: Vec<String> = (0..argc.max(0) as usize)
        // SAFETY: the caller's promise.
        .map(|n| unsafe { CStr::from_ptr(*argv.add(n)) }.to_string_lossy().into_owned())
        .collect();
    let mut environment = Vec::new();
    // SAFETY: environ is always an array of strings, ending in null.
    unsafe {
        let mut at = environ;
        while let Some(entry) = at.read().as_ref() {
            environment.push(CStr::from_ptr(entry).to_string_lossy().into_owned());
            at = at.add(1);
        }
    }
    let name = if arguments.is_empty() { String::from("sh") } else { arguments.remove(0) };
    let myself = {
        let file = crate::__taaj_executable();
        if file.is_null() {
            String::from("/sh")
        } else {
            // SAFETY: a string the library keeps, ending in a zero byte.
            unsafe { CStr::from_ptr(file) }.to_string_lossy().into_owned()
        }
    };
    let mut system = TheTaaJ { myself };
    let usage = "usage: sh [-ex] -c COMMAND [NAME [ARGUMENT...]]\n       sh [-ex] FILE [ARGUMENT...]\n";
    let (mut errexit, mut xtrace, mut command) = (false, false, false);
    while let Some(first) = arguments.first() {
        let on = match first.as_bytes() {
            [b'-', b'-'] => {
                arguments.remove(0);
                break;
            }
            [b'-', _, ..] => true,
            [b'+', _, ..] => false,
            _ => break,
        };
        for option in first.chars().skip(1) {
            match option {
                'e' => errexit = on,
                'x' => xtrace = on,
                'c' => command = true,
                _ => {
                    system.write(2, &format!("sh: -{option}: not an option this shell has\n{usage}"));
                    return 2;
                }
            }
        }
        arguments.remove(0);
    }
    let (script, zero) = if command {
        if arguments.is_empty() {
            system.write(2, &format!("sh: -c needs a command\n{usage}"));
            return 2;
        }
        let script = arguments.remove(0);
        let zero = if arguments.is_empty() { name } else { arguments.remove(0) };
        (script, zero)
    } else {
        let Some(file) = (!arguments.is_empty()).then(|| arguments.remove(0)) else {
            system.write(2, usage);
            return 2;
        };
        let mut text = Vec::new();
        let read = crate::descriptors::with(|state| {
            let files = state.files().ok_or("this program was not given the files")?;
            files.read(&file, |piece| text.extend_from_slice(piece)).map_err(|error| error.describe())
        });
        if let Err(why) = read {
            system.write(2, &format!("sh: {file}: {why}\n"));
            return 127;
        }
        (String::from_utf8_lossy(&text).into_owned(), file)
    };
    let mut shell = Shell::new(system, &environment, zero, arguments);
    shell.set_errexit(errexit);
    shell.set_xtrace(xtrace);
    shell.run(&script)
}
