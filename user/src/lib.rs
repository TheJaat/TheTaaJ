//! `taaj` - the runtime every TheTaaJ user program links: system call
//! wrappers, `print!`, the heap, the panic handler and the program entry.
//!
//! A user program has no C library and no Rust `std` underneath it
//! (yet): only `core`, and the system calls in `abi`. This is the
//! thinnest layer that makes writing one pleasant.

#![no_std]

extern crate alloc;

mod allocator;
pub mod block_service;
pub mod disk;
pub mod files;
pub mod net;
pub mod nic;
pub mod nic_service;
pub mod services;
pub mod sync;
pub mod thread;
pub mod window;

use core::arch::asm;
use core::fmt::{self, Write};
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

pub use abi::{call, rights, Error};
pub use allocator::heap_usage;

/// A handle: a number naming an entry in this process's handle table.
pub type Handle = u64;

/// The raw system call, with up to three arguments.
///
/// This is the one place where a program speaks to the kernel, and it
/// is different on each CPU - the instruction, and which registers
/// carry what:
///
/// ```text
///              instruction   number   arguments                  result
///   x86_64     syscall       RAX      RDI RSI RDX R10 R8 R9      RAX
///   AArch64    svc #0        x8       x0  x1  x2  x3  x4 x5      x0
/// ```
///
/// Everything above this function is the same on both.
#[inline(always)]
pub fn syscall(number: u64, a: u64, b: u64, c: u64) -> u64 {
    syscall6(number, a, b, c, 0, 0, 0)
}

/// The raw system call with five arguments.
#[inline(always)]
pub fn syscall5(number: u64, a: u64, b: u64, c: u64, d: u64, e: u64) -> u64 {
    syscall6(number, a, b, c, d, e, 0)
}

/// The raw system call with six arguments. On x86 the `syscall`
/// instruction itself overwrites RCX and R11, so they are clobbers.
#[cfg(target_arch = "x86_64")]
#[inline(always)]
pub fn syscall6(number: u64, a: u64, b: u64, c: u64, d: u64, e: u64, f: u64) -> u64 {
    let result: u64;
    // SAFETY: the kernel validates every argument; the worst a bad one
    // does is return an error.
    unsafe {
        asm!("syscall",
             inlateout("rax") number => result,
             in("rdi") a, in("rsi") b, in("rdx") c, in("r10") d, in("r8") e, in("r9") f,
             out("rcx") _, out("r11") _,
             options(nostack));
    }
    result
}

/// The raw system call with six arguments. `svc` ("supervisor call")
/// changes no register by itself, and the kernel returns every one
/// but x0 as it found it.
#[cfg(target_arch = "aarch64")]
#[inline(always)]
pub fn syscall6(number: u64, a: u64, b: u64, c: u64, d: u64, e: u64, f: u64) -> u64 {
    let result: u64;
    // SAFETY: the kernel validates every argument; the worst a bad one
    // does is return an error.
    unsafe {
        asm!("svc #0",
             in("x8") number,
             inlateout("x0") a => result,
             in("x1") b, in("x2") c, in("x3") d, in("x4") e, in("x5") f,
             options(nostack));
    }
    result
}

pub fn exit(code: i64) -> ! {
    syscall(call::EXIT, code as u64, 0, 0);
    unreachable!("exit returned")
}

/// Whether what is shown on the screen reaches the kernel's debug log
/// by itself - because the two are one thing: a serial port, on a
/// machine with no other terminal (QEMU's AArch64 machine; `serial` is
/// its driver). There a copy to the log would show every line twice.
/// On the PC they are two things, a framebuffer and COM1, and the copy
/// is what makes the log read as the screen does.
///
/// Decided by the CPU for now, as init's `SERIAL_TERMINAL` is.
pub const LOG_IS_SCREEN: bool = cfg!(not(target_arch = "x86_64"));

/// Writes raw bytes to standard output: wherever whoever started this
/// program pointed it. A program does not know where that is, and does
/// not need to - which is the point: the same `cat` shows a file on the
/// screen, or copies it into another file.
///
///   - the screen: shown there, and copied to the kernel's debug log
///     (the serial line), so that the log reads as the screen does;
///   - anything else - a file being written, another program: sent
///     there, all of it, and nowhere else;
///   - no standard output at all (a service, a test program): the
///     debug log.
///
/// If the output is a stream and its reader has gone - the other end
/// is closed - the program ends here, with the exit code NO_READER. A
/// program writes for someone; with nobody left to read, going on is
/// only work, and a program that writes for ever (`yes`) would never
/// stop. (Unix does this with a signal, SIGPIPE.) Few programs check
/// what a print returned, so it cannot be left to them.
pub fn write(bytes: &[u8]) -> Result<u64, Error> {
    match stdout() {
        Some(screen) if output_is_screen() => {
            write_to_display(screen, bytes);
            if LOG_IS_SCREEN {
                return Ok(bytes.len() as u64);
            }
            debug_write(bytes)
        }
        Some(output) => match write_to_stream(output, bytes) {
            Ok(()) => Ok(bytes.len() as u64),
            Err(Error::PeerClosed) => exit(abi::NO_READER),
            Err(error) => Err(error),
        },
        None => debug_write(bytes),
    }
}

/// Writes raw bytes to standard error: where a program says what went
/// wrong, apart from what it was asked to produce. `eprintln!` ends up
/// here. For now that is always the screen (and the debug log, as for
/// anything shown there) - so it is written as text for the display
/// is written, and given up on if the display has stopped. A program started
/// without one - a service, a test - has only the debug log.
pub fn write_error(bytes: &[u8]) -> Result<u64, Error> {
    if let Some(screen) = flagged_handle(abi::startup::STDERR) {
        write_to_display(screen, bytes);
        if LOG_IS_SCREEN {
            return Ok(bytes.len() as u64);
        }
    }
    debug_write(bytes)
}

/// Whether standard output is the screen - someone is looking at it -
/// and not a file or another program. Almost nothing should ask: a
/// program that prints differently depending on who reads cannot be
/// trusted in a pipeline. The fair use is decoration that is noise
/// anywhere else - as `cat` ends an unfinished last line on the screen,
/// and must not add a byte to a copy. (Unix calls the question isatty.)
pub fn output_is_screen() -> bool {
    stdout().is_some() && STARTUP_ARGUMENT.load(Ordering::Relaxed) & abi::startup::SCREEN != 0
}

/// Writes raw bytes to the kernel's debug log, whatever standard
/// output is.
pub fn debug_write(bytes: &[u8]) -> Result<u64, Error> {
    abi::decode(syscall(call::DEBUG_WRITE, bytes.as_ptr() as u64, bytes.len() as u64, 0))
}

/// Sends `bytes` on `channel` as a stream (abi::stream): every byte, in
/// order. A channel holds only so many messages; when it is full the
/// reader has work to do, and this steps aside until there is room -
/// however long that takes. Nothing is dropped: what is on the other
/// end may be a file. It ends early only if the other end is closed -
/// nobody is reading any more - and says so.
///
/// (Stepping aside is a loop that asks again: the kernel has no "wake
/// me when there is room" yet. It yields at first, since a reader with
/// a full queue is ready to run, and then sleeps a tick at a time, so
/// that a reader that is itself waiting for a disk is not raced.)
pub fn write_to_stream(channel: Handle, bytes: &[u8]) -> Result<(), Error> {
    let mut message = [0u8; abi::MESSAGE_MAX as usize];
    message[0] = abi::stream::DATA;
    for chunk in bytes.chunks(abi::stream::DATA_MAX) {
        message[1..=chunk.len()].copy_from_slice(chunk);
        let mut tries = 0u32;
        loop {
            match send(channel, &message[..=chunk.len()], &[]) {
                Ok(()) => break,
                Err(Error::QueueFull) if tries < 64 => yield_now(),
                Err(Error::QueueFull) => sleep(1),
                Err(error) => return Err(error),
            }
            tries += 1;
        }
    }
    Ok(())
}

/// Standard input: what this program was given to read, if anything.
///
/// A stream has no lines and no fixed pieces - bytes arrive in
/// messages of whatever size the sender chose - so this keeps what a
/// message brought until the program has asked for all of it.
pub struct Input {
    channel: Option<Handle>,
    /// The message being handed out, and how much of it already was.
    held: alloc::vec::Vec<u8>,
    taken: usize,
    /// Whether the input is the keyboard: then nothing comes until it
    /// is asked for (abi::stream::WANT).
    keyboard: bool,
    /// Whether it has been asked for, and has not come yet. One
    /// asking, one line: a second would have the shell hold the next
    /// line typed for a program that is not waiting for it.
    asked: bool,
}

/// This program's standard input. Make one, and keep it: a second one
/// would not have what the first had read but not yet handed out.
pub fn input() -> Input {
    Input {
        channel: flagged_handle(abi::startup::STDIN),
        held: alloc::vec::Vec::new(),
        taken: 0,
        keyboard: input_is_keyboard(),
        asked: false,
    }
}

/// Whether standard input is the keyboard: someone typing, a line at a
/// time, and not a file or another program. (Unix: isatty(0).)
pub fn input_is_keyboard() -> bool {
    let flags = STARTUP_ARGUMENT.load(Ordering::Relaxed);
    flags & abi::startup::STDIN != 0 && flags & abi::startup::KEYBOARD != 0
}

impl Input {
    /// Whether there is an input at all. Without one, `read` says "the
    /// end" at once.
    pub fn is_given(&self) -> bool {
        self.channel.is_some()
    }

    /// Up to `out.len()` bytes, waiting until there are some. 0 means
    /// the end: the sender closed its end and everything was read.
    pub fn read(&mut self, out: &mut [u8]) -> usize {
        loop {
            if self.taken < self.held.len() {
                let n = out.len().min(self.held.len() - self.taken);
                out[..n].copy_from_slice(&self.held[self.taken..self.taken + n]);
                self.taken += n;
                return n;
            }
            let Some(channel) = self.channel else { return 0 };
            // A keyboard is asked before it is waited for. If the
            // asking cannot be sent, nobody is there to answer.
            if self.keyboard && !self.asked {
                if send(channel, &[abi::stream::WANT], &[]).is_err() {
                    self.channel = None;
                    return 0;
                }
                self.asked = true;
            }
            let mut message = [0u8; abi::MESSAGE_MAX as usize];
            match receive(channel, &mut message, &mut []) {
                // A message with no bytes in it is not the end: only a
                // closed channel is. Go round again.
                Ok((length, _)) if length >= 1 && message[0] == abi::stream::DATA => {
                    self.asked = false;
                    self.held.clear();
                    self.held.extend_from_slice(&message[1..length]);
                    self.taken = 0;
                }
                Ok(_) => {} // not part of the stream
                Err(_) => {
                    self.channel = None;
                    return 0;
                }
            }
        }
    }

    /// Whether a read would not wait, finding out without waiting:
    /// `(bytes, ended)` - bytes held or come in, or the end reached.
    /// Messages already there are taken in. A keyboard is asked for its
    /// next line, as `read` would: whoever asks whether there is input
    /// means to read it.
    pub fn ready(&mut self) -> (bool, bool) {
        loop {
            if self.taken < self.held.len() {
                return (true, false);
            }
            let Some(channel) = self.channel else { return (false, true) };
            if self.keyboard && !self.asked {
                if send(channel, &[abi::stream::WANT], &[]).is_err() {
                    self.channel = None;
                    return (false, true);
                }
                self.asked = true;
            }
            if !matches!(poll(&[channel]), Ok(Some(_))) {
                return (false, false);
            }
            let mut message = [0u8; abi::MESSAGE_MAX as usize];
            match receive(channel, &mut message, &mut []) {
                Ok((length, _)) if length >= 1 && message[0] == abi::stream::DATA => {
                    self.asked = false;
                    self.held.clear();
                    self.held.extend_from_slice(&message[1..length]);
                    self.taken = 0;
                }
                Ok(_) => {}
                Err(_) => self.channel = None,
            }
        }
    }

    /// The channel input comes on, while there may be more: what to
    /// wait on for it.
    pub fn handle(&self) -> Option<Handle> {
        self.channel
    }

    /// All of it, a piece at a time, until the end.
    pub fn each(&mut self, mut piece: impl FnMut(&[u8])) {
        let mut buffer = [0u8; 1024];
        loop {
            match self.read(&mut buffer) {
                0 => return,
                n => piece(&buffer[..n]),
            }
        }
    }
}

/// Sends `bytes` as display TEXT messages on `channel`.
///
/// A program can print faster than the display draws. When the
/// display's queue is full the program steps aside and tries again:
/// the text waits, it is not lost. But not forever - a display that has
/// stopped for good must not stop every program that prints. After
/// about a second without room the text is dropped, and from then on
/// nothing waits for that display until a message gets through again.
pub fn write_to_display(channel: Handle, bytes: &[u8]) {
    use core::sync::atomic::AtomicBool;
    /// Ticks to wait for room before giving up on a message.
    const PATIENCE: u64 = 100;
    static STALLED: AtomicBool = AtomicBool::new(false);

    let mut message = [0u8; 256];
    for chunk in bytes.chunks(message.len() - 1) {
        message[0] = abi::display::TEXT;
        message[1..=chunk.len()].copy_from_slice(chunk);
        let started = ticks();
        loop {
            match send(channel, &message[..=chunk.len()], &[]) {
                Err(Error::QueueFull) if ticks() - started < PATIENCE && !STALLED.load(Ordering::Relaxed) => {
                    // A full queue means the display has work, so it is
                    // ready to run: giving up the CPU lets it.
                    yield_now();
                }
                Err(Error::QueueFull) => {
                    STALLED.store(true, Ordering::Relaxed);
                    break; // dropped
                }
                Ok(()) => {
                    STALLED.store(false, Ordering::Relaxed);
                    break;
                }
                Err(_) => break, // no display at all
            }
        }
    }
}

/// The same call with an arbitrary pointer - for testing that the kernel
/// refuses bad ones.
pub fn write_raw(pointer: u64, length: u64) -> Result<u64, Error> {
    abi::decode(syscall(call::DEBUG_WRITE, pointer, length, 0))
}

pub fn yield_now() {
    syscall(call::YIELD, 0, 0, 0);
}

/// Sleeps for `ticks` timer ticks - less, if a signal the program
/// catches cuts it short.
pub fn sleep(ticks: u64) {
    syscall(call::SLEEP, ticks, 0, 0);
}

/// `sleep`, saying whether it slept the whole time: false if a signal
/// cut it short (its handler has run by now). The one wait here that is
/// not restarted (see `restarting`): nanosleep is to say EINTR.
pub fn sleep_unless_signalled(ticks: u64) -> bool {
    abi::decode(syscall(call::SLEEP, ticks, 0, 0)).is_ok()
}

/// Sleeps until the monotonic clock (`monotonic_ns`) reads at least
/// `nanoseconds`, to the nanosecond the timer gives (journal entry NNN).
/// False if a signal cut it short (its handler has run by now).
pub fn sleep_until_ns(nanoseconds: u64) -> bool {
    abi::decode(syscall(call::SLEEP_UNTIL, nanoseconds, 0, 0)).is_ok()
}

/// A call that waits, made again for as long as a signal cuts it short.
///
/// A signal a program catches ends whatever wait one of its threads is
/// in, with Interrupted, so that the thread goes back to user mode and
/// the handler runs there (kernel64's signals.rs). By the time the call
/// returns here, it has. The wait is then simply made again: what POSIX
/// calls SA_RESTART, and what every caller of these functions expects -
/// none of them was written for a wait that ends with nothing. (A
/// program being killed never sees Interrupted: it ends first.)
fn restarting(mut call: impl FnMut() -> u64) -> Result<u64, Error> {
    loop {
        match abi::decode(call()) {
            Err(Error::Interrupted) => continue,
            other => return other,
        }
    }
}

pub fn process_id() -> u64 {
    syscall(call::PROCESS_ID, 0, 0, 0)
}

pub fn ticks() -> u64 {
    syscall(call::TICKS, 0, 0, 0)
}

/// Nanoseconds since the system started, from the CPU's counter: finer
/// than `ticks`, and never going back.
pub fn monotonic_ns() -> u64 {
    abi::decode(syscall(call::CLOCK, abi::clock::MONOTONIC, 0, 0)).unwrap_or(0)
}

/// The time of day: nanoseconds since 1 January 1970, UTC - from the
/// machine's real-time clock at boot, plus the time since.
pub fn realtime_ns() -> u64 {
    abi::decode(syscall(call::CLOCK, abi::clock::REALTIME, 0, 0)).unwrap_or(0)
}

/// SCHED_SET: a thread's or a process's priority (as a level, see
/// abi::sched), CPUs, or a process's ceiling (journal entry 124).
pub fn sched_set(target: u64, id: u64, what: u64, value: u64) -> Result<(), Error> {
    abi::decode(syscall5(call::SCHED_SET, target, id, what, value, 0)).map(|_| ())
}

/// SCHED_GET: what SCHED_SET sets, and the calling thread's number.
pub fn sched_get(target: u64, id: u64, what: u64) -> Result<u64, Error> {
    abi::decode(syscall(call::SCHED_GET, target, id, what))
}

/// The CPU time this process has used, all its threads together, in
/// nanoseconds: the time they were on a CPU (clock::PROCESS_CPU).
pub fn process_cpu_ns() -> u64 {
    abi::decode(syscall(call::CLOCK, abi::clock::PROCESS_CPU, 0, 0)).unwrap_or(0)
}

/// A new channel: the handles of its two ends.
pub fn channel_create() -> Result<(Handle, Handle), Error> {
    let mut ends = [0u64; 2];
    abi::decode(syscall(call::CHANNEL_CREATE, ends.as_mut_ptr() as u64, 0, 0))?;
    Ok((ends[0], ends[1]))
}

/// Sends `data` and `handles` (which leave this process) on `channel`.
pub fn send(channel: Handle, data: &[u8], handles: &[Handle]) -> Result<(), Error> {
    abi::decode(syscall5(
        call::CHANNEL_SEND,
        channel,
        data.as_ptr() as u64,
        data.len() as u64,
        handles.as_ptr() as u64,
        handles.len() as u64,
    ))
    .map(|_| ())
}

/// Waits for a message on `channel`; returns (bytes, handles) received.
pub fn receive(channel: Handle, buffer: &mut [u8], handles: &mut [Handle]) -> Result<(usize, usize), Error> {
    let packed = restarting(|| {
        syscall5(
            call::CHANNEL_RECEIVE,
            channel,
            buffer.as_mut_ptr() as u64,
            buffer.len() as u64,
            handles.as_mut_ptr() as u64,
            handles.len() as u64,
        )
    })?;
    Ok(((packed & 0xFFFF_FFFF) as usize, (packed >> 32) as usize))
}

/// As `receive`, but waits only until the tick count reaches
/// `deadline`: `TimedOut` if no message (and no closed peer) by then.
/// For a wait on someone who may never answer.
pub fn receive_until(channel: Handle, buffer: &mut [u8], handles: &mut [Handle], deadline: u64) -> Result<(usize, usize), Error> {
    match object_wait_until(&[channel], deadline)? {
        Some(_) => receive(channel, buffer, handles),
        None => Err(Error::TimedOut),
    }
}

/// Answers every ping waiting on `pings` (abi::PING): init's way of
/// asking a service whether it is still running. Called from the
/// service's own loop - the point is that the loop is turning. False
/// once the channel is closed: nobody is asking any more.
pub fn answer_pings(pings: Handle) -> bool {
    let mut message = [0u8; 8];
    while let Ok(Some(_)) = poll(&[pings]) {
        let mut handles = [0u64; abi::MESSAGE_HANDLES_MAX as usize];
        match receive(pings, &mut message, &mut handles) {
            Ok((_, count)) => {
                if count == 1 {
                    let _ = send(handles[0], &[abi::PING], &[]);
                }
                for &handle in &handles[..count] {
                    let _ = close(handle);
                }
            }
            Err(Error::BufferTooSmall) => return false, // not a ping: stop listening
            Err(_) => return false,
        }
    }
    true
}

/// How long a client waits for a service's answer before it gives up,
/// in ticks: 10 seconds. A service that is merely busy answers far
/// sooner; one that has stopped answering must not take its clients
/// with it. Longer than init takes to notice a silent service and
/// start another (see its watchdog): a request that was waiting when
/// the service was replaced is answered by the new one, and the client
/// sees a pause, not a failure.
pub const ANSWER_WAIT: u64 = 1000;

pub fn close(handle: Handle) -> Result<(), Error> {
    abi::decode(syscall(call::HANDLE_CLOSE, handle, 0, 0)).map(|_| ())
}

/// Another handle to the same object, with fewer (or the same) rights.
pub fn duplicate(handle: Handle, rights: u32) -> Result<Handle, Error> {
    abi::decode(syscall(call::HANDLE_DUPLICATE, handle, u64::from(rights), 0))
}

/// Waits for the interrupt `line`; returns how often it fired.
pub fn interrupt_wait(line: Handle) -> Result<u64, Error> {
    restarting(|| syscall(call::INTERRUPT_WAIT, line, 0, 0))
}

/// Lets this process use the I/O ports `ports` names, with the `port`
/// functions.
pub fn ioports_enable(ports: Handle) -> Result<(), Error> {
    abi::decode(syscall(call::IOPORTS_ENABLE, ports, 0, 0)).map(|_| ())
}

/// Direct I/O port access - the same instructions the kernel uses, in
/// ring 3. Allowed only for ports enabled with `ioports_enable`; any
/// other port is a general protection fault, and the kernel kills the
/// process.
#[cfg(target_arch = "x86_64")]
pub mod port {
    use core::arch::asm;

    /// # Safety
    /// The port must be enabled, and reading it must be harmless to the
    /// device (some registers change state when read).
    pub unsafe fn in8(port: u16) -> u8 {
        let value: u8;
        asm!("in al, dx", out("al") value, in("dx") port, options(nomem, nostack, preserves_flags));
        value
    }

    /// # Safety
    /// The port must be enabled, and the value right for the device.
    pub unsafe fn out8(port: u16, value: u8) {
        asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack, preserves_flags));
    }

    /// # Safety
    /// As for `in8`.
    pub unsafe fn in16(port: u16) -> u16 {
        let value: u16;
        asm!("in ax, dx", out("ax") value, in("dx") port, options(nomem, nostack, preserves_flags));
        value
    }

    /// # Safety
    /// As for `out8`.
    pub unsafe fn out16(port: u16, value: u16) {
        asm!("out dx, ax", in("dx") port, in("ax") value, options(nomem, nostack, preserves_flags));
    }

    /// # Safety
    /// As for `in8`.
    pub unsafe fn in32(port: u16) -> u32 {
        let value: u32;
        asm!("in eax, dx", out("eax") value, in("dx") port, options(nomem, nostack, preserves_flags));
        value
    }

    /// # Safety
    /// As for `out8`.
    pub unsafe fn out32(port: u16, value: u32) {
        asm!("out dx, eax", in("dx") port, in("eax") value, options(nomem, nostack, preserves_flags));
    }
}

/// Starts the program `name` with `handles` - (handle, rights) pairs -
/// as its startup handles. They leave this process. Returns a handle to
/// the new process.
pub fn spawn(launcher: Handle, name: &str, handles: &[(Handle, u32)], argument: u64) -> Result<Handle, Error> {
    let mut pairs = [0u64; 2 * abi::SPAWN_HANDLES_MAX as usize];
    if handles.len() > abi::SPAWN_HANDLES_MAX as usize {
        return Err(Error::InvalidArgument);
    }
    for (n, &(handle, rights)) in handles.iter().enumerate() {
        pairs[2 * n] = handle;
        pairs[2 * n + 1] = u64::from(rights);
    }
    abi::decode(syscall6(
        call::PROCESS_SPAWN,
        launcher,
        name.as_ptr() as u64,
        name.len() as u64,
        pairs.as_ptr() as u64,
        handles.len() as u64,
        argument,
    ))
}

/// Starts a program from an ELF file in the memory object `image` -
/// one read from the disk, say. `name` is what the process is called.
/// The image stays with the caller; `handles` leave, as for `spawn`.
pub fn spawn_image(
    launcher: Handle,
    image: Handle,
    name: &str,
    handles: &[(Handle, u32)],
    argument: u64,
) -> Result<Handle, Error> {
    let mut pairs = [0u64; 2 * abi::SPAWN_HANDLES_MAX as usize];
    if handles.len() > abi::SPAWN_HANDLES_MAX as usize {
        return Err(Error::InvalidArgument);
    }
    for (n, &(handle, rights)) in handles.iter().enumerate() {
        pairs[2 * n] = handle;
        pairs[2 * n + 1] = u64::from(rights);
    }
    let request: [u64; abi::SPAWN_REQUEST_WORDS] =
        [name.as_ptr() as u64, name.len() as u64, pairs.as_ptr() as u64, handles.len() as u64, argument];
    abi::decode(syscall(call::PROCESS_SPAWN_IMAGE, launcher, image, request.as_ptr() as u64))
}

/// Waits until any of `handles` is ready (a channel to receive from, a
/// process that ended, a pending interrupt); returns its index.
pub fn object_wait(handles: &[Handle]) -> Result<usize, Error> {
    restarting(|| syscall(call::OBJECT_WAIT, handles.as_ptr() as u64, handles.len() as u64, 0)).map(|i| i as usize)
}

/// As `object_wait`, but only until the tick count reaches `deadline`:
/// None if nothing was ready by then.
pub fn object_wait_until(handles: &[Handle], deadline: u64) -> Result<Option<usize>, Error> {
    match restarting(|| syscall(call::OBJECT_WAIT_UNTIL, handles.as_ptr() as u64, handles.len() as u64, deadline)) {
        Ok(index) => Ok(Some(index as usize)),
        Err(Error::TimedOut) => Ok(None),
        Err(error) => Err(error),
    }
}

/// As `object_wait_until` (or `object_wait`, with no deadline), but a
/// signal that runs a handler ends the wait: `Err(Interrupted)`, after
/// the handler ran. For the calls POSIX says fail with EINTR whatever
/// SA_RESTART says - poll and select among them.
pub fn object_wait_or_signal(handles: &[Handle], deadline: Option<u64>) -> Result<Option<usize>, Error> {
    let made = match deadline {
        Some(deadline) => syscall(call::OBJECT_WAIT_UNTIL, handles.as_ptr() as u64, handles.len() as u64, deadline),
        None => syscall(call::OBJECT_WAIT, handles.as_ptr() as u64, handles.len() as u64, 0),
    };
    match abi::decode(made) {
        Ok(index) => Ok(Some(index as usize)),
        Err(Error::TimedOut) => Ok(None),
        Err(error) => Err(error),
    }
}

/// Which of `handles` is ready right now, if any - without waiting.
pub fn poll(handles: &[Handle]) -> Result<Option<usize>, Error> {
    object_wait_until(handles, 0)
}

pub fn system_info(system: Handle) -> Result<abi::SystemInfo, Error> {
    let mut words = [0u64; abi::SystemInfo::WORDS];
    abi::decode(syscall(call::SYSTEM_INFO, system, words.as_mut_ptr() as u64, 0))?;
    Ok(abi::SystemInfo::from_words(words))
}

/// Fills `out` with processes; returns how many exist (maybe more).
pub fn process_list(system: Handle, out: &mut [abi::ProcessEntry]) -> Result<usize, Error> {
    const W: usize = abi::ProcessEntry::WORDS;
    let mut words = [0u64; 32 * W];
    let capacity = out.len().min(32);
    let total = abi::decode(syscall(call::PROCESS_LIST, system, words.as_mut_ptr() as u64, capacity as u64))?;
    for (n, entry) in out.iter_mut().take(capacity).enumerate() {
        *entry = abi::ProcessEntry::from_words(words[n * W..(n + 1) * W].try_into().unwrap());
    }
    Ok(total as usize)
}

/// Waits until one of `processes` ends: returns its index and exit code.
pub fn wait_any(processes: &[Handle]) -> Result<(usize, i64), Error> {
    let mut code = 0i64;
    let index = restarting(|| syscall(call::PROCESS_WAIT, processes.as_ptr() as u64, processes.len() as u64, &raw mut code as u64))?;
    Ok((index as usize, code))
}

/// Asks the kernel to end `process` (the handle needs the KILL right).
/// It ends soon, not at once: wait for it, as for any other. Its exit
/// code will be abi::STOPPED.
pub fn kill(process: Handle) -> Result<(), Error> {
    abi::decode(syscall(call::PROCESS_KILL, process, 0, 0)).map(|_| ())
}

/// How this process takes signals (abi::call::SIGNAL_SETUP): libc's
/// business. A Rust program that never calls it is ended by any signal
/// not ignored by default - Ctrl+C among them.
pub fn signal_setup(entry: u64, caught: u64, ignored: u64, on_stack: u64) -> Result<(), Error> {
    abi::decode(syscall5(call::SIGNAL_SETUP, entry, caught, ignored, on_stack, 0)).map(|_| ())
}

/// Fills `buffer` with random bytes from the kernel's generator (at
/// most abi::RANDOM_MAX at a time). Never blocks.
pub fn random(buffer: &mut [u8]) -> Result<(), Error> {
    for chunk in buffer.chunks_mut(abi::RANDOM_MAX as usize) {
        abi::decode(syscall(call::RANDOM, chunk.as_mut_ptr() as u64, chunk.len() as u64, 0))?;
    }
    Ok(())
}

/// This program's ELF program headers, copied into `buffer` as far as
/// they fit; how many bytes they all take.
pub fn program_headers(buffer: &mut [u8]) -> Result<usize, Error> {
    abi::decode(syscall(call::PROGRAM_HEADERS, buffer.as_mut_ptr() as u64, buffer.len() as u64, 0)).map(|n| n as usize)
}

/// This thread's signal stack: `size` bytes from `base` (0: none).
pub fn signal_stack(base: u64, size: u64) -> Result<(), Error> {
    abi::decode(syscall(call::SIGNAL_STACK, base, size, 0)).map(|_| ())
}

/// `signal` to this process when `process` ends.
pub fn process_notify(process: Handle, signal: u64) -> Result<(), Error> {
    abi::decode(syscall(call::PROCESS_NOTIFY, process, signal, 0)).map(|_| ())
}

/// SIGALRM to this process `nanoseconds` from now, in place of any alarm
/// set before (0: none); the nanoseconds the one before had left.
pub fn alarm(nanoseconds: u64) -> Result<u64, Error> {
    abi::decode(syscall(call::ALARM, nanoseconds, 0, 0))
}

/// `alarm`, and then again every `interval` nanoseconds if that is not 0
/// (journal entry NNN); the nanoseconds the one before had left.
pub fn alarm_every(nanoseconds: u64, interval: u64) -> Result<u64, Error> {
    abi::decode(syscall(call::ALARM, nanoseconds, interval, 0))
}

/// The nanoseconds this process's alarm has left, or 0: nothing changes.
pub fn alarm_left() -> u64 {
    abi::decode(syscall(call::ALARM, 0, 0, 1)).unwrap_or(0)
}

/// Sends `signal` to `process` (a handle with the KILL right, or
/// abi::signal::SELF).
pub fn signal_send(process: Handle, signal: u64) -> Result<(), Error> {
    abi::decode(syscall(call::SIGNAL_SEND, process, signal, 0)).map(|_| ())
}

/// A new memory object of at least `size` bytes, zeroed.
pub fn memory_create(size: u64) -> Result<Handle, Error> {
    abi::decode(syscall(call::MEMORY_CREATE, size, 0, 0))
}

/// Maps a memory object; returns where.
pub fn memory_map(memory: Handle, writable: bool) -> Result<*mut u8, Error> {
    abi::decode(syscall(call::MEMORY_MAP, memory, u64::from(writable), 0)).map(|a| a as *mut u8)
}

/// Asks the firmware, through a power handle (only `power` is given
/// one, on AArch64), to switch the machine off (abi::power::OFF) or
/// reset it (abi::power::RESET). Returns only if the firmware refused.
/// Starts another thread in this program at `entry`, with its stack
/// pointer at `stack` (16-byte aligned: the top of room made for it),
/// `argument` as its first argument, and thread pointer `pointer`. Its
/// id. (`thread::spawn` does all this for a closure.)
pub fn thread_create(entry: u64, stack: u64, argument: u64, pointer: u64) -> Result<u64, Error> {
    abi::decode(syscall5(call::THREAD_CREATE, entry, stack, argument, pointer, 0))
}

/// Ends this thread, after setting `clear` to 0 and waking whoever
/// waits on it - or ends the program, if this is its last thread.
pub fn thread_exit(clear: &AtomicU32) -> ! {
    syscall(call::THREAD_EXIT, clear as *const AtomicU32 as u64, 0, 0);
    unreachable!("thread_exit returned")
}

/// Sets this thread's thread pointer (FS base / TPIDR_EL0).
pub fn set_thread_pointer(value: u64) -> Result<(), Error> {
    abi::decode(syscall(call::THREAD_POINTER, value, 0, 0)).map(|_| ())
}

/// Sleeps while `word` holds `expected`, until a futex_wake on it or
/// the tick count reaches `deadline` (0: no deadline). Err(Changed) at
/// once if it does not hold `expected`; Err(TimedOut) at the deadline.
pub fn futex_wait(word: &AtomicU32, expected: u32, deadline: u64) -> Result<(), Error> {
    restarting(|| syscall(call::FUTEX_WAIT, word as *const AtomicU32 as u64, u64::from(expected), deadline)).map(|_| ())
}

/// Wakes up to `count` threads waiting on `word`; how many it woke.
pub fn futex_wake(word: &AtomicU32, count: u32) -> u64 {
    abi::decode(syscall(call::FUTEX_WAKE, word as *const AtomicU32 as u64, u64::from(count), 0)).unwrap_or(0)
}

pub fn power(handle: Handle, action: u8) -> Result<(), Error> {
    abi::decode(syscall(call::POWER, handle, u64::from(action), 0)).map(|_| ())
}

/// The size of a memory object, in bytes.
pub fn memory_size(memory: Handle) -> Result<u64, Error> {
    abi::decode(syscall(call::MEMORY_SIZE, memory, 0, 0))
}

/// Removes a mapping, or reserved memory, given where it starts. Its
/// pages go back to the kernel (an object's once nothing else holds it).
pub fn memory_unmap(address: *mut u8) -> Result<(), Error> {
    abi::decode(syscall(call::MEMORY_UNMAP, address as u64, 0, 0)).map(|_| ())
}

/// Reserves `size` bytes of address space; returns where. Each page
/// gets zeroed memory when first touched, counted against this
/// program's limit then; touching one past the limit ends the program.
/// Given back by `memory_unmap`.
pub fn memory_reserve(size: u64, writable: bool) -> Result<*mut u8, Error> {
    abi::decode(syscall(call::MEMORY_RESERVE, size, u64::from(writable), 0)).map(|a| a as *mut u8)
}

/// What this program may do with the pages of `start..start+length`
/// in its reserved memory: `access` is one of abi::access's - nothing,
/// read, or read and write. Pages keep their bytes whatever the access.
/// Whole pages, in one reservation (MEMORY_PROTECT).
pub fn memory_protect(start: *mut u8, length: u64, access: u64) -> Result<(), Error> {
    abi::decode(syscall(call::MEMORY_PROTECT, start as u64, length, access)).map(|_| ())
}

/// Gives back the memory of the pages of `start..start+length` in this
/// program's reserved memory; the address space stays, and each page is
/// zeroed again when next touched (MEMORY_DISCARD).
pub fn memory_discard(start: *mut u8, length: u64) -> Result<(), Error> {
    abi::decode(syscall(call::MEMORY_DISCARD, start as u64, length, 0)).map(|_| ())
}

/// Lowers this program's memory limit to `bytes` if that is lower (0
/// only asks); the limit now, in bytes. Programs it starts later start
/// with it.
pub fn memory_limit(bytes: u64) -> u64 {
    abi::decode(syscall(call::MEMORY_LIMIT, bytes, 0, 0)).unwrap_or(0)
}

/// The bytes counted against this program now: its loaded file, the
/// pages of its stack and reserved memory it has touched, and memory
/// objects it made.
pub fn memory_used() -> u64 {
    abi::decode(syscall(call::MEMORY_USED, 0, 0, 0)).unwrap_or(0)
}

static STARTUP_HANDLES: AtomicU64 = AtomicU64::new(0);
static STARTUP_ARGUMENT: AtomicU64 = AtomicU64::new(0);

/// The startup handle a flag stands for, if the parent set that flag:
/// flagged handles come first, in the order of the flags' bits
/// (abi::startup::index - KEYBOARD and SCREEN bring none).
fn flagged_handle(flag: u64) -> Option<Handle> {
    abi::startup::index(STARTUP_ARGUMENT.load(Ordering::Relaxed), flag).and_then(startup_handle)
}

/// The channel of descriptors beyond 0, 1 and 2 a C program's parent
/// handed down (abi::startup::DESCRIPTORS), if it did: libc reads it.
pub fn descriptors() -> Option<Handle> {
    flagged_handle(abi::startup::DESCRIPTORS)
}

/// Standard output's channel, if the parent gave one.
pub fn stdout() -> Option<Handle> {
    flagged_handle(abi::startup::STDOUT)
}

/// Standard input's channel, if the parent gave one. (To read it, use
/// `input`; this is the handle, to hand on.)
pub fn stdin() -> Option<Handle> {
    flagged_handle(abi::startup::STDIN)
}

/// Standard error's channel, if the parent gave one.
pub fn stderr() -> Option<Handle> {
    flagged_handle(abi::startup::STDERR)
}

/// The program's arguments: what was typed after its name, as whoever
/// started it divided it - `write "my file" hello` gives "my file" and
/// "hello". None at all is an empty list. They arrive as one message,
/// so this reads them once - call it once.
pub fn arguments() -> alloc::vec::Vec<alloc::string::String> {
    command_line().arguments
}

/// The whole command line: the arguments, and the program itself - the
/// name it was started by and its file (abi::startup::ARGUMENTS).
pub struct CommandLine {
    /// What it was started as: "ls", "tools/given". None if whoever
    /// started it did not say.
    pub name: Option<alloc::string::String>,
    /// Its file, as a path from the root - "/ls" - or None: built into
    /// the kernel image, or not said.
    pub file: Option<alloc::string::String>,
    pub arguments: alloc::vec::Vec<alloc::string::String>,
    /// The environment whoever started it gave - `NAME=value` words -
    /// or none (abi::init::ENVIRONMENT). Only C programs look at it.
    pub environment: alloc::vec::Vec<alloc::string::String>,
}

/// The command line, read from its channel. Like `arguments`, which is
/// this without the program's name: call one of the two, once.
pub fn command_line() -> CommandLine {
    let words = |message: &[u8]| -> alloc::vec::Vec<alloc::string::String> {
        abi::arguments::decode(message).map(|word| alloc::string::String::from_utf8_lossy(word).into_owned()).collect()
    };
    let mut message = [0u8; abi::MESSAGE_MAX as usize];
    let mut next = || -> Option<alloc::vec::Vec<alloc::string::String>> {
        let channel = flagged_handle(abi::startup::ARGUMENTS)?;
        let (length, _) = receive(channel, &mut message, &mut []).ok()?;
        Some(words(&message[..length]))
    };
    let arguments = next().unwrap_or_default();
    // The program, if init said: what it was started as, and its file.
    // A channel with only the arguments on it is closed by now, so this
    // does not wait.
    let program = next().unwrap_or_default();
    let environment = next().unwrap_or_default();
    let mut program = program.into_iter().filter(|word| !word.is_empty());
    CommandLine { name: program.next(), file: program.next(), arguments, environment }
}

/// Where this program asks for services by name: its channel to the
/// registry, if whoever started it gave it one.
pub fn services() -> Option<services::Services> {
    flagged_handle(abi::startup::SERVICES).map(services::Services::new)
}

/// The file service, if this program was given it - asked for by its
/// name, "files", which is all a program knows of it: whether that
/// means the channel that can change files or the one that can only
/// read them was decided by whoever started the program. It comes with
/// the directory this program was started in, so that the names it is
/// given mean what they meant to whoever typed them. The directory
/// arrives as one message, like the arguments: call this once.
pub fn file_service() -> Option<files::Files> {
    let services = services()?;
    let channel = services.lookup("files").ok()?;
    let mut message = [0u8; abi::MESSAGE_MAX as usize];
    let length = match flagged_handle(abi::startup::DIRECTORY).map(|from| receive(from, &mut message, &mut [])) {
        Some(Ok((length, _))) => length,
        _ => 0, // none given: the root
    };
    // Not text? Then the root: a wrong directory would send every name
    // somewhere the user did not mean.
    let directory = core::str::from_utf8(&message[..length]).unwrap_or("");
    let files = files::Files::in_directory(channel, directory);
    // /tmp (J64), if this program was given it: another file service,
    // its files in memory, mounted at /tmp in this program's namespace
    // (J62). Without it, /tmp is whatever the disk has there.
    // And /dev (J65), the same way: the devices' service, at /dev.
    let mount = |files: files::Files, at: &str, name: &str| match services.lookup(name).ok().and_then(|service| files::File::from_handle(service).ok()) {
        Some(service) => files.mounted(at, service).unwrap_or(files),
        None => files,
    };
    let files = mount(files, "/tmp", "tmp");
    Some(mount(files, "/dev", "dev"))
}

/// Startup handle `n` (counting from 0), if the parent gave that many.
/// They are numbered 1..=count by the ABI.
pub fn startup_handle(n: u64) -> Option<Handle> {
    (n < STARTUP_HANDLES.load(Ordering::Relaxed)).then_some(n + 1)
}

/// The bootstrap channel: by convention, the first startup handle. 0 if
/// there is none.
pub fn bootstrap() -> Handle {
    startup_handle(0).unwrap_or(0)
}

#[doc(hidden)]
pub fn set_startup(count: u64, argument: u64) {
    STARTUP_HANDLES.store(count, Ordering::Relaxed);
    STARTUP_ARGUMENT.store(argument, Ordering::Relaxed);
}

pub struct Console;

impl Write for Console {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        for chunk in text.as_bytes().chunks(abi::DEBUG_WRITE_MAX as usize) {
            write(chunk).map_err(|_| fmt::Error)?;
        }
        Ok(())
    }
}

/// The same for standard error.
pub struct ErrorConsole;

impl Write for ErrorConsole {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        for chunk in text.as_bytes().chunks(abi::DEBUG_WRITE_MAX as usize) {
            write_error(chunk).map_err(|_| fmt::Error)?;
        }
        Ok(())
    }
}

/// Text formatted into a fixed buffer, with no allocation - so printing
/// works even when the heap does not. Writes past the end are cut off
/// (and reported as an error by `write_str`).
pub struct Buffer<const N: usize> {
    bytes: [u8; N],
    length: usize,
}

impl<const N: usize> Buffer<N> {
    pub const fn new() -> Self {
        Buffer { bytes: [0; N], length: 0 }
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.length]
    }
}

impl<const N: usize> Default for Buffer<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> Write for Buffer<N> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        let room = N - self.length;
        let n = text.len().min(room);
        self.bytes[self.length..self.length + n].copy_from_slice(&text.as_bytes()[..n]);
        self.length += n;
        if n < text.len() { Err(fmt::Error) } else { Ok(()) }
    }
}

/// Formats everything first, then writes it with one system call. The
/// kernel prints each call whole, so a line cannot be split by another
/// process printing at the same moment - which `format_args`, writing
/// piece by piece, would otherwise allow. Longer text falls back to
/// writing in pieces.
#[doc(hidden)]
pub fn print_args(args: fmt::Arguments) {
    let mut buffer = Buffer::<512>::new();
    if buffer.write_fmt(args).is_ok() {
        let _ = write(buffer.as_bytes());
    } else {
        let _ = Console.write_fmt(args);
    }
}

/// The same, to standard error.
#[doc(hidden)]
pub fn eprint_args(args: fmt::Arguments) {
    let mut buffer = Buffer::<512>::new();
    if buffer.write_fmt(args).is_ok() {
        let _ = write_error(buffer.as_bytes());
    } else {
        let _ = ErrorConsole.write_fmt(args);
    }
}

/// `eprint!` and `eprintln!`: as `print!` and `println!`, to standard
/// error. For complaints and usage lines - whatever is about the
/// program rather than from it - so that `ls nosuch > list` shows the
/// complaint and leaves `list` holding only a listing.
#[macro_export]
macro_rules! eprint {
    ($($arg:tt)*) => { $crate::eprint_args(format_args!($($arg)*)) };
}

#[macro_export]
macro_rules! eprintln {
    () => { $crate::eprint!("\n") };
    ($($arg:tt)*) => { $crate::eprint!("{}\n", format_args!($($arg)*)) };
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => { $crate::print_args(format_args!($($arg)*)) };
}

#[macro_export]
macro_rules! println {
    () => { $crate::print!("\n") };
    ($($arg:tt)*) => { $crate::print!("{}\n", format_args!($($arg)*)) };
}

/// Declares the program's `main`: `taaj::program!(main);` with
/// `fn main() -> i64`, whose return value becomes the exit code.
///
/// It defines `_start`, where the kernel starts the program. The kernel
/// enters it with the stack pointer set as if `_start` had just been
/// called, and the number of startup handles and the parent's argument
/// where the calling convention puts a function's first two arguments
/// (RDI and RSI on x86_64, x0 and x1 on AArch64) - so it is an
/// ordinary function.
#[macro_export]
macro_rules! program {
    ($main:path) => {
        #[no_mangle]
        pub extern "C" fn _start(startup_handles: u64, argument: u64) -> ! {
            $crate::set_startup(startup_handles, argument);
            $crate::exit($main())
        }
    };
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    eprintln!("panic: {info}");
    exit(-2)
}
