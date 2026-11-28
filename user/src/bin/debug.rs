//! debug: a small debugger (K142). Starts a program - and so holds its
//! handle with the DEBUG right, which only whoever starts a program
//! gets - then takes commands, a line each:
//!
//!   stop              stop it (each thread at its next return to it)
//!   wait [TICKS]      wait for a thread to stop (default 300 ticks)
//!   cont              let it go on - past the breakpoint it stopped at
//!   regs              the stopped thread's registers
//!   x ADDR [N]        N bytes of its memory at ADDR (default 16)
//!   w ADDR BYTE...    write bytes there (hex)
//!   break ADDR        a breakpoint at ADDR: it stops there, once
//!   break pc          one where the stopped thread goes on from
//!   kill              end it
//!   quit              end it, and this
//!
//!   usage: debug PROGRAM [ARGUMENTS]
//!
//! The shell gives it init's name, to start programs with.

#![no_std]
#![no_main]

extern crate alloc;

use abi::debug::{op, reason, Stop};
use abi::signal::Context;
use alloc::string::String;
use alloc::vec::Vec;
use taaj::{eprintln, print, println, Handle};

taaj::program!(main);

#[cfg(target_arch = "x86_64")]
const BREAKPOINT: &[u8] = &abi::debug::BREAKPOINT_X86_64;
#[cfg(target_arch = "aarch64")]
const BREAKPOINT: &[u8] = &abi::debug::BREAKPOINT_AARCH64;

#[cfg(target_arch = "x86_64")]
const REGISTERS: [&str; 16] = ["rax", "rbx", "rcx", "rdx", "rsi", "rdi", "rbp", "r8", "r9", "r10", "r11", "r12", "r13", "r14", "r15", ""];
#[cfg(target_arch = "aarch64")]
const REGISTERS: [&str; 31] = [
    "x0", "x1", "x2", "x3", "x4", "x5", "x6", "x7", "x8", "x9", "x10", "x11", "x12", "x13", "x14", "x15", "x16", "x17", "x18", "x19", "x20",
    "x21", "x22", "x23", "x24", "x25", "x26", "x27", "x28", "x29", "x30",
];

/// Starts `name` from the disk through init, with this program's output
/// as its own; its process handle - with DEBUG, as the starter's. The
/// program's file goes to init as a memory object (abi::init::IMAGE),
/// read here, as libc's spawn does.
fn start(name: &str, arguments: &[String]) -> Result<Handle, String> {
    let services = taaj::services().ok_or("given no services")?;
    let init = services.lookup("init").map_err(|_| "not given init")?;
    let files = taaj::files::Files::new(services.lookup("files").map_err(|_| "not given the files")?);
    let path = alloc::format!("/{name}");
    let size = files.size(&path).map_err(|_| "no such program on the disk")?;
    if size == 0 || size > abi::PROGRAM_IMAGE_MAX {
        return Err(String::from("not a program"));
    }
    let image = taaj::memory_create(size).map_err(|_| "no memory for its file")?;
    if files.read_into(&path, image) != Ok(size) {
        let _ = taaj::close(image);
        return Err(String::from("its file could not be read"));
    }
    let mut encoded = [0u8; abi::MESSAGE_MAX as usize];
    let length = abi::arguments::encode(arguments.iter().map(String::as_str), &mut encoded).ok_or("arguments too long")?;
    let file = path;
    let request = abi::init::Run {
        kind: abi::init::IMAGE | abi::init::REPLY,
        flags: abi::startup::STDOUT as u8,
        name,
        directory: "",
        file: &file,
        arguments: &encoded[..length],
    };
    let mut message = [0u8; abi::MESSAGE_MAX as usize];
    let size = request.encode(&mut message).ok_or("request too long")?;
    let output = taaj::stdout().and_then(|out| taaj::duplicate(out, abi::rights::SEND | abi::rights::DUPLICATE | abi::rights::TRANSFER).ok());
    let (mine, theirs) = taaj::channel_create().map_err(|_| "no channel")?;
    // The image, the output (the flags' order), the reply.
    let Some(output) = output else {
        return Err(String::from("no output to give it"));
    };
    let given = [image, output, theirs];
    taaj::send(init, &message[..size], &given).map_err(|e| alloc::format!("init did not take the request: {e:?}"))?;
    let mut answer = [0u8; 128];
    let mut process = [0u64; 1];
    let reply = taaj::receive(mine, &mut answer, &mut process);
    let _ = taaj::close(mine);
    match reply {
        Ok((2, 1)) if &answer[..2] == b"ok" => Ok(process[0]),
        Ok((n, _)) => Err(String::from(core::str::from_utf8(&answer[..n]).unwrap_or("error"))),
        Err(e) => Err(alloc::format!("no answer from init: {e:?}")),
    }
}

fn number(word: &str) -> Option<u64> {
    let word = word.trim_start_matches("0x");
    u64::from_str_radix(word, 16).ok()
}

/// The last thread that stopped, and a breakpoint's original bytes.
struct Session {
    process: Handle,
    stopped: Option<(u64, Stop)>,
    breakpoints: Vec<(u64, Vec<u8>)>,
}

impl Session {
    fn wait(&mut self, ticks: u64) {
        let mut words = [0u64; Stop::WORDS];
        match taaj::process_debug(self.process, op::WAIT, ticks, words.as_mut_ptr() as u64, 0) {
            Ok(thread) => {
                let stop = Stop::from_words(words);
                let why = if stop.reason == reason::BREAKPOINT { "at a breakpoint" } else { "as asked" };
                println!("debug: thread {thread} of pid {} stopped {why}, at {:#x}", stop.pid, stop.address);
                self.stopped = Some((thread, stop));
            }
            Err(abi::Error::TimedOut) => println!("debug: no thread stopped"),
            Err(e) => println!("debug: {e:?}"),
        }
    }

    fn registers(&self) -> Option<(u64, Context)> {
        let (thread, _) = self.stopped?;
        let mut bytes = [0u8; abi::signal::CONTEXT_SIZE];
        taaj::process_debug(self.process, op::GET_REGISTERS, thread, bytes.as_mut_ptr() as u64, 0).ok()?;
        Context::from_bytes(&bytes).map(|c| (thread, c))
    }

    fn cont(&mut self) {
        // Stopped at one of ours: its bytes back, and the pc on it -
        // the breakpoint is gone (set it again to stop there again).
        if let Some((thread, stop)) = self.stopped.filter(|(_, s)| s.reason == reason::BREAKPOINT) {
            if let Some(at) = self.breakpoints.iter().position(|(a, _)| *a == stop.address) {
                let (address, original) = self.breakpoints.remove(at);
                let _ = taaj::process_debug(self.process, op::WRITE, address, original.as_ptr() as u64, original.len() as u64);
                if let Some((_, mut context)) = self.registers() {
                    context.pc = address;
                    let bytes = context.to_bytes();
                    let _ = taaj::process_debug(self.process, op::SET_REGISTERS, thread, bytes.as_ptr() as u64, 0);
                }
            }
        }
        self.stopped = None;
        match taaj::process_debug(self.process, op::CONTINUE, 0, 0, 0) {
            Ok(_) => println!("debug: going on"),
            Err(e) => println!("debug: {e:?}"),
        }
    }

    fn command(&mut self, line: &str) -> bool {
        let words: Vec<&str> = line.split_whitespace().collect();
        match words.as_slice() {
            ["stop"] => {
                match taaj::process_debug(self.process, op::STOP, 0, 0, 0) {
                    Ok(_) => self.wait(300),
                    Err(e) => println!("debug: {e:?}"),
                }
            }
            ["wait"] => self.wait(300),
            ["wait", ticks] => self.wait(ticks.parse().unwrap_or(300)),
            ["cont"] => self.cont(),
            ["regs"] => match self.registers() {
                Some((thread, c)) => {
                    println!("debug: thread {thread}: pc {:#x}  sp {:#x}  flags {:#x}", c.pc, c.sp, c.flags);
                    for (n, name) in REGISTERS.iter().enumerate().filter(|(_, n)| !n.is_empty()) {
                        print!("{name:>4} {:#018x}{}", c.registers[n], if n % 4 == 3 { "\n" } else { "  " });
                    }
                    println!();
                }
                None => println!("debug: no thread is stopped (stop, or wait)"),
            },
            ["x", address] | ["x", address, _] => {
                let (Some(address), count) = (number(address), words.get(2).and_then(|n| n.parse().ok()).unwrap_or(16usize)) else {
                    println!("debug: x ADDRESS [COUNT]");
                    return true;
                };
                let mut bytes = alloc::vec![0u8; count.min(256)];
                match taaj::process_debug(self.process, op::READ, address, bytes.as_mut_ptr() as u64, bytes.len() as u64) {
                    Ok(_) => {
                        for (n, row) in bytes.chunks(16).enumerate() {
                            print!("{:#x}:", address + n as u64 * 16);
                            for byte in row {
                                print!(" {byte:02x}");
                            }
                            println!();
                        }
                    }
                    Err(e) => println!("debug: cannot read {address:#x}: {e:?}"),
                }
            }
            ["w", address, bytes @ ..] if !bytes.is_empty() => {
                let values: Option<Vec<u8>> = bytes.iter().map(|b| u8::from_str_radix(b.trim_start_matches("0x"), 16).ok()).collect();
                match (number(address), values) {
                    (Some(address), Some(values)) => {
                        match taaj::process_debug(self.process, op::WRITE, address, values.as_ptr() as u64, values.len() as u64) {
                            Ok(n) => println!("debug: {n} bytes written at {address:#x}"),
                            Err(e) => println!("debug: cannot write {address:#x}: {e:?}"),
                        }
                    }
                    _ => println!("debug: w ADDRESS BYTE..."),
                }
            }
            ["break", address] => match if *address == "pc" { self.registers().map(|(_, c)| c.pc) } else { number(address) } {
                Some(address) => {
                    let mut original = alloc::vec![0u8; BREAKPOINT.len()];
                    let read = taaj::process_debug(self.process, op::READ, address, original.as_mut_ptr() as u64, original.len() as u64);
                    let wrote = read.and_then(|_| taaj::process_debug(self.process, op::WRITE, address, BREAKPOINT.as_ptr() as u64, BREAKPOINT.len() as u64));
                    match wrote {
                        Ok(_) => {
                            self.breakpoints.push((address, original));
                            println!("debug: a breakpoint at {address:#x}");
                        }
                        Err(e) => println!("debug: cannot set a breakpoint at {address:#x}: {e:?}"),
                    }
                }
                None => println!("debug: break ADDRESS"),
            },
            ["kill"] | ["quit"] => {
                let _ = taaj::kill(self.process);
                println!("debug: ended it");
                return words[0] == "kill";
            }
            [] => {}
            _ => println!("debug: stop, wait [TICKS], cont, regs, x ADDR [N], w ADDR BYTE..., break ADDR, kill, quit"),
        }
        true
    }
}

fn main() -> i64 {
    let arguments = taaj::arguments();
    let Some((name, rest)) = arguments.split_first() else {
        eprintln!("usage: debug PROGRAM [ARGUMENTS]");
        return 2;
    };
    let process = match start(name, rest) {
        Ok(process) => process,
        Err(why) => {
            eprintln!("debug: {name}: {why}");
            return 1;
        }
    };
    println!("debug: started {name}; commands: stop, wait, cont, regs, x, w, break, kill, quit");
    let mut session = Session { process, stopped: None, breakpoints: Vec::new() };
    let mut input = taaj::input();
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        if input.read(&mut byte) == 0 {
            let _ = taaj::kill(process);
            return 0;
        }
        if byte[0] != b'\n' {
            line.push(byte[0]);
            continue;
        }
        let text = String::from_utf8_lossy(&line).into_owned();
        line.clear();
        if !session.command(text.trim()) {
            return 0;
        }
    }
}
