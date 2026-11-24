//! The shell (sh.rs) on TheTaaJ: what it asks of a system, done with
//! this library - programs started as posix_spawn starts them
//! (spawn.rs `start_jobs`), wildcards matched by `glob` (unix.c), the
//! current directory and the files as the rest of the library has them.
//! And `__taaj_sh`, the shell program's whole `main` (programs/sh.c):
//!
//!   sh [-ex] -c COMMAND [NAME [ARGUMENT...]]    runs COMMAND; $0 NAME
//!   sh [-ex] FILE [ARGUMENT...]                 runs the script in FILE
//!
//! A builtin or function in a pipeline, or with redirections
//! (`echo hi > f`), a compound command that is (`for ...; done | wc`),
//! a subshell and a command substitution are run by another copy of
//! this program - the file it was started from (`__taaj_executable`) -
//! as a Unix shell forks one: `sh -c '"$@"' sh echo hi`, the functions
//! defined first. Its output goes where the redirections say, with
//! nobody here to copy it.

use crate::sh::{Command, Kind, Redirection, Shell, System};
use crate::spawn::{close_fd, output_flags, reap, start_jobs, Action, Failed, Job, INPUT_FLAGS};
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

impl TheTaaJ {
    fn job(&self, command: &Command) -> Job {
        let actions = command
            .redirections
            .iter()
            .map(|redirection| match redirection {
                Redirection::In { fd, path } => Action::Open { descriptor: *fd as c_int, path: path.clone(), flags: INPUT_FLAGS },
                Redirection::Out { fd, path, append } => {
                    Action::Open { descriptor: *fd as c_int, path: path.clone(), flags: output_flags(*append) }
                }
                Redirection::Copy { fd, to } => Action::Dup2 { from: *to as c_int, to: *fd as c_int },
                Redirection::Close { fd } => Action::Close(*fd as c_int),
            })
            .collect();
        let environment = environment_bytes(&command.environment);
        if let Some(copy) = &command.shell {
            let script = format!("{}{}", copy.functions, copy.script);
            let mut words: Vec<String> = alloc::vec![String::from("sh"), String::from("-c"), script, copy.zero.clone()];
            words.extend(command.words.iter().cloned());
            Job { path: self.myself.clone(), words, actions, environment }
        } else {
            Job { path: command.words[0].clone(), words: command.words.clone(), actions, environment }
        }
    }
}

impl System for TheTaaJ {
    fn pipeline(&mut self, commands: &[Command]) -> i32 {
        let jobs: Vec<Job> = commands.iter().map(|command| self.job(command)).collect();
        match start_jobs(&jobs, None, None, "sh") {
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
        // SAFETY: room for two.
        if unsafe { crate::pipes::pipe(ends.as_mut_ptr()) } != 0 {
            self.write(2, "sh: no descriptor for a command substitution's pipe\n");
            return (String::new(), 126);
        }
        let job = self.job(command);
        let started = start_jobs(core::slice::from_ref(&job), None, Some(ends[1]), "sh");
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
