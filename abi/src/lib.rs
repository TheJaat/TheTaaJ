//! The TheTaaJ system call interface: the contract between the kernel
//! and every user program.
//!
//! One crate, used by both sides, so the numbers cannot drift apart. It
//! holds only constants and plain types - no code that runs.
//!
//! The calling convention (x86_64), close to Linux's so it is familiar:
//!
//!   `syscall` instruction
//!   RAX        system call number
//!   RDI, RSI, RDX, R10, R8, R9   arguments 1-6 (R10, not RCX: the
//!              instruction itself overwrites RCX with the return address)
//!   RAX        result: >= 0 success, < 0 an `Error`, negated
//!   RCX, R11   destroyed (the CPU uses them); everything else preserved
//!
//! This is the native interface, deliberately small. The POSIX layer will
//! sit on top of it, in user space.
//!
//! Capabilities: a process reaches kernel objects only through handles -
//! small numbers naming entries in its own handle table, each with a set
//! of `rights`. A handle is meaningful only to the process holding it;
//! the same number in another process names something else, or nothing.
//! Handles are passed between processes by sending them in messages.
//!
//! A program starts with the handles its parent gave it - its startup
//! handles - and nothing else. They are numbered 1, 2, ... n, in the
//! order the parent listed them, with n in RDI. By convention handle 1
//! is the bootstrap channel, when there is one.

#![no_std]

/// System call numbers.
pub mod call {
    /// exit(code) - ends the process. Never returns.
    pub const EXIT: u64 = 0;
    /// debug_write(pointer, length) - writes text to the kernel console.
    /// A stand-in until there are real I/O channels. Returns the length.
    pub const DEBUG_WRITE: u64 = 1;
    /// yield() - lets other threads run.
    pub const YIELD: u64 = 2;
    /// sleep(ticks) - sleeps for at least that many timer ticks.
    pub const SLEEP: u64 = 3;
    /// process_id() - this process's id.
    pub const PROCESS_ID: u64 = 4;
    /// ticks() - timer ticks since boot.
    pub const TICKS: u64 = 5;
    /// channel_create(out: *mut [u64; 2]) - a new channel; writes the
    /// handles of its two ends, each with all rights.
    pub const CHANNEL_CREATE: u64 = 6;
    /// channel_send(handle, data, data_length, handles, handle_count) -
    /// queues a message on the other end. Never blocks: a full queue is
    /// `QueueFull`. The handles move with the message - they leave the
    /// sender's table, all of them or (on any error) none.
    pub const CHANNEL_SEND: u64 = 7;
    /// channel_receive(handle, buffer, buffer_length, handles,
    /// handle_capacity) - takes the next message, waiting for one if
    /// needed. Returns `data_length | handle_count << 32`. A message that
    /// does not fit is left queued (`BufferTooSmall`).
    pub const CHANNEL_RECEIVE: u64 = 8;
    /// handle_close(handle).
    pub const HANDLE_CLOSE: u64 = 9;
    /// handle_duplicate(handle, rights) - a second handle to the same
    /// object, with `rights`, which must be a subset of the original's.
    /// Needs DUPLICATE.
    pub const HANDLE_DUPLICATE: u64 = 10;
    /// interrupt_wait(handle) - waits until the interrupt has fired;
    /// returns how many times it did since the last wait. Needs RECEIVE.
    pub const INTERRUPT_WAIT: u64 = 11;
    /// ioports_enable(handle) - lets this process use the I/O ports the
    /// handle names directly, with `in` and `out`. Any other port is
    /// still a fault.
    pub const IOPORTS_ENABLE: u64 = 12;
    /// process_spawn(launcher, name, name_length, handles, count) -
    /// starts the program `name`. `handles` points to `count` pairs of
    /// u64s, (handle, rights): each handle moves into the child as its
    /// startup handle, with exactly `rights` - a subset of what the
    /// parent held, so a parent can hand on less than it has. Moving
    /// needs TRANSFER on the parent's handle. Returns a handle to the
    /// new process. All-or-nothing, like a send.
    pub const PROCESS_SPAWN: u64 = 13;
    /// process_wait(handles, count, code) - waits until any of `count`
    /// processes has ended; returns its index in the list and writes its
    /// exit code (an i64) to `code`.
    pub const PROCESS_WAIT: u64 = 14;
    /// memory_create(size) - a new memory object of `size` bytes (rounded
    /// up to whole pages), zeroed. Returns a handle with all rights.
    pub const MEMORY_CREATE: u64 = 15;
    /// memory_map(handle, writable) - maps the whole object into this
    /// process, at an address the kernel chooses; returns it. Needs READ,
    /// and WRITE too if `writable`. Never executable.
    pub const MEMORY_MAP: u64 = 16;
    /// memory_unmap(address) - removes the mapping that starts there.
    pub const MEMORY_UNMAP: u64 = 17;
}

/// What a handle permits. A right, once removed, cannot be added back:
/// duplicates and transfers keep or narrow rights, never widen them.
pub mod rights {
    /// Send on a channel.
    pub const SEND: u32 = 1 << 0;
    /// Receive from a channel.
    pub const RECEIVE: u32 = 1 << 1;
    /// Make another handle to the object.
    pub const DUPLICATE: u32 = 1 << 2;
    /// Send the handle to another process.
    pub const TRANSFER: u32 = 1 << 3;
    /// Map a memory object for reading.
    pub const READ: u32 = 1 << 4;
    /// Map a memory object for writing.
    pub const WRITE: u32 = 1 << 5;
    pub const ALL: u32 = SEND | RECEIVE | DUPLICATE | TRANSFER | READ | WRITE;
}

/// Most bytes in one message.
pub const MESSAGE_MAX: u64 = 1024;
/// Most handles in one message.
pub const MESSAGE_HANDLES_MAX: u64 = 8;
/// Messages waiting at one channel end before sends fail with QueueFull.
pub const CHANNEL_QUEUE_MAX: usize = 32;
/// Most startup handles a process may be given, and most processes one
/// process_wait may watch.
pub const SPAWN_HANDLES_MAX: u64 = 8;
/// Longest program name.
pub const PROGRAM_NAME_MAX: u64 = 64;

/// Largest memory object: 16 MiB. (Per-process memory quotas come
/// later; until then this bounds what one call can take.)
pub const MEMORY_MAX: u64 = 16 << 20;
pub const PAGE_SIZE: u64 = 4096;

/// Most handles one process may hold.
pub const HANDLES_MAX: usize = 1024;

/// Why a system call failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i64)]
pub enum Error {
    /// No system call has that number.
    UnknownCall = 1,
    /// An argument is out of range.
    InvalidArgument = 2,
    /// A pointer argument points at memory the process cannot access.
    BadAddress = 3,
    /// Not a handle in this process's table (never was, or closed).
    InvalidHandle = 4,
    /// The handle lacks a right the call needs.
    AccessDenied = 5,
    /// The other end of the channel is closed.
    PeerClosed = 6,
    /// The other end has too many unread messages.
    QueueFull = 7,
    /// The waiting message is larger than the buffers given.
    BufferTooSmall = 8,
    /// The handle table is full.
    TooManyHandles = 9,
    /// The handle is to the wrong kind of object for this call.
    WrongType = 10,
    /// No such program.
    NotFound = 11,
    /// Not enough memory.
    OutOfMemory = 12,
}

impl Error {
    pub fn from_code(code: i64) -> Option<Error> {
        match code {
            1 => Some(Error::UnknownCall),
            2 => Some(Error::InvalidArgument),
            3 => Some(Error::BadAddress),
            4 => Some(Error::InvalidHandle),
            5 => Some(Error::AccessDenied),
            6 => Some(Error::PeerClosed),
            7 => Some(Error::QueueFull),
            8 => Some(Error::BufferTooSmall),
            9 => Some(Error::TooManyHandles),
            10 => Some(Error::WrongType),
            11 => Some(Error::NotFound),
            12 => Some(Error::OutOfMemory),
            _ => None,
        }
    }
}

/// Packs a result into RAX.
pub fn encode(result: Result<u64, Error>) -> u64 {
    match result {
        Ok(value) => value,
        Err(error) => (-(error as i64)) as u64,
    }
}

/// Unpacks RAX.
pub fn decode(raw: u64) -> Result<u64, Error> {
    let signed = raw as i64;
    if signed < 0 {
        Err(Error::from_code(-signed).unwrap_or(Error::InvalidArgument))
    } else {
        Ok(raw)
    }
}

/// Most bytes one debug_write may carry.
pub const DEBUG_WRITE_MAX: u64 = 4096;

/// The highest user address + 1: the lower half of the address space.
pub const USER_END: u64 = 0x0000_8000_0000_0000;

/// Exit code the kernel records for a process it killed for a fault.
pub const KILLED: i64 = -1000;

/// The screen, as the kernel describes it to init: a linear framebuffer
/// of `pitch` bytes per line, each pixel `bpp` bits with its red, green
/// and blue channels at the given bit positions. Sent as bytes, in a
/// message, with a handle to the framebuffer's memory object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FramebufferInfo {
    pub width: u32,
    pub height: u32,
    pub pitch: u32,
    pub bpp: u8,
    pub red_size: u8,
    pub red_shift: u8,
    pub green_size: u8,
    pub green_shift: u8,
    pub blue_size: u8,
    pub blue_shift: u8,
}

impl FramebufferInfo {
    pub const SIZE: usize = 19;

    pub fn to_bytes(&self) -> [u8; Self::SIZE] {
        let mut b = [0; Self::SIZE];
        b[0..4].copy_from_slice(&self.width.to_le_bytes());
        b[4..8].copy_from_slice(&self.height.to_le_bytes());
        b[8..12].copy_from_slice(&self.pitch.to_le_bytes());
        b[12..].copy_from_slice(&[
            self.bpp,
            self.red_size,
            self.red_shift,
            self.green_size,
            self.green_shift,
            self.blue_size,
            self.blue_shift,
        ]);
        b
    }

    pub fn from_bytes(b: &[u8]) -> Option<FramebufferInfo> {
        if b.len() != Self::SIZE {
            return None;
        }
        let u32_at = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
        Some(FramebufferInfo {
            width: u32_at(0),
            height: u32_at(4),
            pitch: u32_at(8),
            bpp: b[12],
            red_size: b[13],
            red_shift: b[14],
            green_size: b[15],
            green_shift: b[16],
            blue_size: b[17],
            blue_shift: b[18],
        })
    }
}
