//! `taaj` - the runtime every TheTaaJ user program links: system call
//! wrappers, `print!`, the heap, the panic handler and the program entry.
//!
//! A user program has no C library and no Rust `std` underneath it
//! (yet): only `core`, and the system calls in `abi`. This is the
//! thinnest layer that makes writing one pleasant.

#![no_std]

extern crate alloc;

mod allocator;
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

/// Sends `bytes` as display TEXT messages on `channel`. A display that
/// lags or is restarting loses some text; the program carries on.
pub fn write_to_display(channel: Handle, bytes: &[u8]) {
    let mut message = [0u8; 256];
    for chunk in bytes.chunks(message.len() - 1) {
        message[0] = abi::display::TEXT;
        message[1..=chunk.len()].copy_from_slice(chunk);
        let _ = send(channel, &message[..=chunk.len()], &[]);
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

/// Lets this process use the I/O ports `ports` names, with `port::in8`
/// and `port::out8`.
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

/// Waits until any of `handles` is ready (a channel to receive from, a
/// process that ended, a pending interrupt); returns its index.
pub fn object_wait(handles: &[Handle]) -> Result<usize, Error> {
    abi::decode(syscall(call::OBJECT_WAIT, handles.as_ptr() as u64, handles.len() as u64, 0)).map(|i| i as usize)
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

/// A new memory object of at least `size` bytes, zeroed.
pub fn memory_create(size: u64) -> Result<Handle, Error> {
    abi::decode(syscall(call::MEMORY_CREATE, size, 0, 0))
}

/// Maps a memory object; returns where.
pub fn memory_map(memory: Handle, writable: bool) -> Result<*mut u8, Error> {
    abi::decode(syscall(call::MEMORY_MAP, memory, u64::from(writable), 0)).map(|a| a as *mut u8)
}

pub fn memory_unmap(address: *mut u8) -> Result<(), Error> {
    abi::decode(syscall(call::MEMORY_UNMAP, address as u64, 0, 0)).map(|_| ())
}

static STARTUP_HANDLES: AtomicU64 = AtomicU64::new(0);
static STARTUP_ARGUMENT: AtomicU64 = AtomicU64::new(0);

/// The display channel to print to, if the parent gave one (handle 1,
/// flagged by abi::startup::STDOUT).
pub fn stdout() -> Option<Handle> {
    (STARTUP_ARGUMENT.load(Ordering::Relaxed) & abi::startup::STDOUT != 0)
        .then(|| startup_handle(0))
        .flatten()
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
