//! `taaj` - the runtime every TheTaaJ user program links: system call
//! wrappers, `print!`, the heap, the panic handler and the program entry.
//!
//! A user program has no C library and no Rust `std` underneath it
//! (yet): only `core`, and the system calls in `abi`. This is the
//! thinnest layer that makes writing one pleasant.

#![no_std]

extern crate alloc;

mod allocator;
pub mod disk;
pub mod files;

use core::arch::asm;
use core::fmt::{self, Write};
use core::sync::atomic::{AtomicU64, Ordering};

pub use abi::{call, rights, Error};
pub use allocator::heap_usage;

/// A handle: a number naming an entry in this process's handle table.
pub type Handle = u64;

/// The raw system call: number in RAX, arguments in RDI, RSI, RDX. The
/// `syscall` instruction overwrites RCX and R11, so they are clobbers.
#[inline(always)]
pub fn syscall(number: u64, a: u64, b: u64, c: u64) -> u64 {
    let result: u64;
    // SAFETY: the kernel validates every argument; the worst a bad one
    // does is return an error.
    unsafe {
        asm!("syscall",
             inlateout("rax") number => result,
             in("rdi") a, in("rsi") b, in("rdx") c,
             out("rcx") _, out("r11") _,
             options(nostack));
    }
    result
}

/// The raw system call with five arguments (RDI, RSI, RDX, R10, R8).
#[inline(always)]
pub fn syscall5(number: u64, a: u64, b: u64, c: u64, d: u64, e: u64) -> u64 {
    syscall6(number, a, b, c, d, e, 0)
}

/// The raw system call with six arguments (RDI, RSI, RDX, R10, R8, R9).
#[inline(always)]
pub fn syscall6(number: u64, a: u64, b: u64, c: u64, d: u64, e: u64, f: u64) -> u64 {
    let result: u64;
    // SAFETY: as for `syscall`.
    unsafe {
        asm!("syscall",
             inlateout("rax") number => result,
             in("rdi") a, in("rsi") b, in("rdx") c, in("r10") d, in("r8") e, in("r9") f,
             out("rcx") _, out("r11") _,
             options(nostack));
    }
    result
}

pub fn exit(code: i64) -> ! {
    syscall(call::EXIT, code as u64, 0, 0);
    unreachable!("exit returned")
}

/// Writes raw bytes to the kernel's debug log (the serial line), and to
/// stdout - the display - if this program was given one.
pub fn write(bytes: &[u8]) -> Result<u64, Error> {
    if let Some(stdout) = stdout() {
        write_to_display(stdout, bytes);
    }
    abi::decode(syscall(call::DEBUG_WRITE, bytes.as_ptr() as u64, bytes.len() as u64, 0))
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

pub fn sleep(ticks: u64) {
    syscall(call::SLEEP, ticks, 0, 0);
}

pub fn process_id() -> u64 {
    syscall(call::PROCESS_ID, 0, 0, 0)
}

pub fn ticks() -> u64 {
    syscall(call::TICKS, 0, 0, 0)
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
    let packed = abi::decode(syscall5(
        call::CHANNEL_RECEIVE,
        channel,
        buffer.as_mut_ptr() as u64,
        buffer.len() as u64,
        handles.as_mut_ptr() as u64,
        handles.len() as u64,
    ))?;
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

/// How long a client waits for a service's answer before it gives up,
/// in ticks: 5 seconds. A service that is merely busy answers far
/// sooner; one that has stopped answering must not take its clients
/// with it.
pub const ANSWER_WAIT: u64 = 500;

pub fn close(handle: Handle) -> Result<(), Error> {
    abi::decode(syscall(call::HANDLE_CLOSE, handle, 0, 0)).map(|_| ())
}

/// Another handle to the same object, with fewer (or the same) rights.
pub fn duplicate(handle: Handle, rights: u32) -> Result<Handle, Error> {
    abi::decode(syscall(call::HANDLE_DUPLICATE, handle, u64::from(rights), 0))
}

/// Waits for the interrupt `line`; returns how often it fired.
pub fn interrupt_wait(line: Handle) -> Result<u64, Error> {
    abi::decode(syscall(call::INTERRUPT_WAIT, line, 0, 0))
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
    abi::decode(syscall(call::OBJECT_WAIT, handles.as_ptr() as u64, handles.len() as u64, 0)).map(|i| i as usize)
}

/// As `object_wait`, but only until the tick count reaches `deadline`:
/// None if nothing was ready by then.
pub fn object_wait_until(handles: &[Handle], deadline: u64) -> Result<Option<usize>, Error> {
    match abi::decode(syscall(call::OBJECT_WAIT_UNTIL, handles.as_ptr() as u64, handles.len() as u64, deadline)) {
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
    let index = abi::decode(syscall(
        call::PROCESS_WAIT,
        processes.as_ptr() as u64,
        processes.len() as u64,
        &raw mut code as u64,
    ))?;
    Ok((index as usize, code))
}

/// Asks the kernel to end `process` (the handle needs the KILL right).
/// It ends soon, not at once: wait for it, as for any other. Its exit
/// code will be abi::STOPPED.
pub fn kill(process: Handle) -> Result<(), Error> {
    abi::decode(syscall(call::PROCESS_KILL, process, 0, 0)).map(|_| ())
}

/// A new memory object of at least `size` bytes, zeroed.
pub fn memory_create(size: u64) -> Result<Handle, Error> {
    abi::decode(syscall(call::MEMORY_CREATE, size, 0, 0))
}

/// Maps a memory object; returns where.
pub fn memory_map(memory: Handle, writable: bool) -> Result<*mut u8, Error> {
    abi::decode(syscall(call::MEMORY_MAP, memory, u64::from(writable), 0)).map(|a| a as *mut u8)
}

/// The size of a memory object, in bytes.
pub fn memory_size(memory: Handle) -> Result<u64, Error> {
    abi::decode(syscall(call::MEMORY_SIZE, memory, 0, 0))
}

pub fn memory_unmap(address: *mut u8) -> Result<(), Error> {
    abi::decode(syscall(call::MEMORY_UNMAP, address as u64, 0, 0)).map(|_| ())
}

static STARTUP_HANDLES: AtomicU64 = AtomicU64::new(0);
static STARTUP_ARGUMENT: AtomicU64 = AtomicU64::new(0);

/// The startup handle a flag stands for, if the parent set that flag:
/// flagged handles come first, in the order of the flags' bits.
fn flagged_handle(flag: u64) -> Option<Handle> {
    let flags = STARTUP_ARGUMENT.load(Ordering::Relaxed);
    (flags & flag != 0).then(|| startup_handle(u64::from((flags & (flag - 1)).count_ones()))).flatten()
}

/// The display channel to print to, if the parent gave one.
pub fn stdout() -> Option<Handle> {
    flagged_handle(abi::startup::STDOUT)
}

/// The program's arguments: the text typed after its name, or nothing.
/// They arrive as one message, so this reads them once - call it once.
pub fn arguments() -> alloc::string::String {
    let mut message = [0u8; abi::MESSAGE_MAX as usize];
    let length = match flagged_handle(abi::startup::ARGUMENTS).map(|channel| receive(channel, &mut message, &mut [])) {
        Some(Ok((length, _))) => length,
        _ => 0,
    };
    alloc::string::String::from_utf8_lossy(&message[..length]).into_owned()
}

/// The file service, if the parent passed its channel on.
pub fn file_service() -> Option<files::Files> {
    flagged_handle(abi::startup::FILES).map(files::Files::new)
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
/// enters it with RSP set as if `_start` had just been called, the
/// number of startup handles in RDI and the parent's argument in RSI -
/// its two arguments - so it is an ordinary function.
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
    println!("panic: {info}");
    exit(-2)
}
