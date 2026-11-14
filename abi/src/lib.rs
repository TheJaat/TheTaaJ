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
    /// process_spawn(launcher, name, name_length, handles, count,
    /// argument) - starts the program `name`. `handles` points to `count` pairs of
    /// u64s, (handle, rights): each handle moves into the child as its
    /// startup handle, with exactly `rights` - a subset of what the
    /// parent held, so a parent can hand on less than it has. Moving
    /// needs TRANSFER on the parent's handle. `argument` reaches the
    /// child's `_start` in RSI (see `startup`). Returns a handle to the
    /// new process, which may be passed on (TRANSFER) - it only lets its
    /// holder wait for the process. All-or-nothing, like a send.
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
    /// object_wait(handles, count) - waits until any of the handles is
    /// ready, and returns its index. Ready means: a channel has a message
    /// or its peer closed (a receive would not wait); a process has
    /// ended; an interrupt is pending. Nothing is consumed - the caller
    /// then receives, or reads the exit code, as usual.
    pub const OBJECT_WAIT: u64 = 18;
    /// system_info(system, out) - writes a `SystemInfo` (as u64 words).
    /// Needs a system handle: what is running is not everyone's business.
    pub const SYSTEM_INFO: u64 = 19;
    /// process_list(system, out, capacity) - writes up to `capacity`
    /// `ProcessEntry`s (as u64 words); returns how many processes exist.
    pub const PROCESS_LIST: u64 = 20;
    /// process_spawn_image(launcher, image, request) - starts a program
    /// from an ELF file held in the memory object `image` (which needs
    /// READ, and stays with the caller). `request` points at
    /// SPAWN_REQUEST_WORDS u64s: name pointer, name length, pointer to
    /// (handle, rights) pairs, their count, and the argument - the same
    /// things process_spawn takes, in memory because seven values do not
    /// fit in six registers. Returns a handle to the new process;
    /// NotExecutable if the file is not a valid program.
    pub const PROCESS_SPAWN_IMAGE: u64 = 21;
}

/// The u64s process_spawn_image's request holds.
pub const SPAWN_REQUEST_WORDS: usize = 5;
/// The largest program file process_spawn_image accepts: 4 MiB.
pub const PROGRAM_IMAGE_MAX: u64 = 4 << 20;

/// What a program learns from its parent besides its handles: the
/// argument of process_spawn, in RSI. Each flag says one more startup
/// handle is present; they come in the order of the flags' bits.
pub mod startup {
    /// A channel to the display - print there too.
    pub const STDOUT: u64 = 1 << 0;
    /// A channel holding one message: the program's arguments, as the
    /// text typed after its name.
    pub const ARGUMENTS: u64 = 1 << 1;
    /// The file service's request channel.
    pub const FILES: u64 = 1 << 2;
}

/// Requests to init, from the shell: "start this program".
///
///   [kind] [flags] [name length] [name] [arguments]
///
/// `kind` is BUILT_IN (a program in the kernel image, by name) or IMAGE
/// (the first handle is a memory object holding the program's file).
/// `flags` are startup flags, STDOUT and FILES: for each one set, one
/// more handle follows, in that order - the caller's own handles, which
/// it hands down to the new program. init adds nothing of its own, so a
/// program can get no more than whoever asked for it holds.
///
/// The reply: "ok" and a handle to the new process, or "error: ...".
pub mod init {
    pub const BUILT_IN: u8 = 1;
    pub const IMAGE: u8 = 2;

    pub struct Run<'a> {
        pub kind: u8,
        pub flags: u8,
        pub name: &'a str,
        pub arguments: &'a [u8],
    }

    impl<'a> Run<'a> {
        /// Lays the request out in `out`; its length, or None if it
        /// does not fit.
        pub fn encode(&self, out: &mut [u8]) -> Option<usize> {
            let length = 3 + self.name.len() + self.arguments.len();
            if self.name.len() > 255 || length > out.len() {
                return None;
            }
            out[..3].copy_from_slice(&[self.kind, self.flags, self.name.len() as u8]);
            out[3..3 + self.name.len()].copy_from_slice(self.name.as_bytes());
            out[3 + self.name.len()..length].copy_from_slice(self.arguments);
            Some(length)
        }

        pub fn decode(message: &'a [u8]) -> Option<Run<'a>> {
            let (&[kind, flags, name_length], rest) = message.split_first_chunk::<3>()?;
            let (name, arguments) = rest.split_at_checked(name_length as usize)?;
            let name = core::str::from_utf8(name).ok().filter(|name| !name.is_empty())?;
            matches!(kind, BUILT_IN | IMAGE).then_some(Run { kind, flags, name, arguments })
        }
    }
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
    /// The file is not a program this system can run.
    NotExecutable = 13,
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
            13 => Some(Error::NotExecutable),
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

/// The display service's messages: the first byte says which.
pub mod display {
    /// Followed by a FramebufferInfo: what to draw on. Sent by init to
    /// each display it starts, before anything else.
    pub const INFO: u8 = 1;
    /// Followed by terminal bytes to show.
    pub const TEXT: u8 = 2;
}

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

/// The kernel's state, for `system_info`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SystemInfo {
    pub ticks: u64,
    pub ticks_per_second: u64,
    pub frames_total: u64,
    pub frames_free: u64,
    pub heap_used: u64,
    pub heap_total: u64,
    pub processes: u64,
    pub threads: u64,
    pub channels: u64,
}

impl SystemInfo {
    pub const WORDS: usize = 9;

    pub fn to_words(&self) -> [u64; Self::WORDS] {
        [
            self.ticks,
            self.ticks_per_second,
            self.frames_total,
            self.frames_free,
            self.heap_used,
            self.heap_total,
            self.processes,
            self.threads,
            self.channels,
        ]
    }

    pub fn from_words(w: [u64; Self::WORDS]) -> SystemInfo {
        SystemInfo {
            ticks: w[0],
            ticks_per_second: w[1],
            frames_total: w[2],
            frames_free: w[3],
            heap_used: w[4],
            heap_total: w[5],
            processes: w[6],
            threads: w[7],
            channels: w[8],
        }
    }
}

/// One process, for `process_list`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ProcessEntry {
    pub pid: u64,
    /// The name, padded with zeros.
    pub name: [u8; 16],
    /// None while it runs.
    pub exit_code: Option<i64>,
}

impl ProcessEntry {
    pub const WORDS: usize = 4;
    const RUNNING: u64 = i64::MIN as u64;

    pub fn to_words(&self) -> [u64; Self::WORDS] {
        let name = |half: usize| u64::from_le_bytes(self.name[half * 8..half * 8 + 8].try_into().unwrap());
        [self.pid, name(0), name(1), self.exit_code.map_or(Self::RUNNING, |c| c as u64)]
    }

    pub fn from_words(w: [u64; Self::WORDS]) -> ProcessEntry {
        let mut name = [0; 16];
        name[..8].copy_from_slice(&w[1].to_le_bytes());
        name[8..].copy_from_slice(&w[2].to_le_bytes());
        ProcessEntry { pid: w[0], name, exit_code: (w[3] != Self::RUNNING).then_some(w[3] as i64) }
    }

    pub fn name(&self) -> &str {
        let end = self.name.iter().position(|&b| b == 0).unwrap_or(16);
        core::str::from_utf8(&self.name[..end]).unwrap_or("?")
    }
}

/// A PCI device the kernel set up for a driver, as described to init:
/// sent as b"pci" and these bytes, with handles to its resources -
/// its I/O ports, DMA memory and, if `msix`, its interrupt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PciDeviceInfo {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub vendor_id: u16,
    pub device_id: u16,
    /// The I/O port BAR: first port, and how many.
    pub io_base: u16,
    pub io_size: u16,
    /// Physically contiguous memory for the device to read and write:
    /// where it is (what the device must be told) and how big.
    pub dma_physical: u64,
    pub dma_size: u64,
    /// MSI-X is on, with message 0 going to the interrupt handle.
    pub msix: bool,
}

impl PciDeviceInfo {
    pub const SIZE: usize = 28;

    pub fn to_bytes(&self) -> [u8; Self::SIZE] {
        let mut b = [0; Self::SIZE];
        b[0..3].copy_from_slice(&[self.bus, self.device, self.function]);
        b[3..5].copy_from_slice(&self.vendor_id.to_le_bytes());
        b[5..7].copy_from_slice(&self.device_id.to_le_bytes());
        b[7..9].copy_from_slice(&self.io_base.to_le_bytes());
        b[9..11].copy_from_slice(&self.io_size.to_le_bytes());
        b[11..19].copy_from_slice(&self.dma_physical.to_le_bytes());
        b[19..27].copy_from_slice(&self.dma_size.to_le_bytes());
        b[27] = u8::from(self.msix);
        b
    }

    pub fn from_bytes(b: &[u8]) -> Option<PciDeviceInfo> {
        if b.len() != Self::SIZE {
            return None;
        }
        let u16_at = |o: usize| u16::from_le_bytes([b[o], b[o + 1]]);
        let u64_at = |o: usize| u64::from_le_bytes(b[o..o + 8].try_into().unwrap_or([0; 8]));
        Some(PciDeviceInfo {
            bus: b[0],
            device: b[1],
            function: b[2],
            vendor_id: u16_at(3),
            device_id: u16_at(5),
            io_base: u16_at(7),
            io_size: u16_at(9),
            dma_physical: u64_at(11),
            dma_size: u64_at(19),
            msix: b[27] != 0,
        })
    }
}

/// The block service's protocol: a disk as numbered 512-byte sectors.
///
/// A request: [op] [sector: u64 LE] [data: one sector, for WRITE]
/// A reply:   [status] [payload]
///
/// As with files, each request carries one handle - the end of a fresh
/// channel to send the reply on.
pub mod block {
    pub const SECTOR: usize = 512;

    /// Payload: the number of sectors (u64 LE), then 1 if read-only.
    pub const INFO: u8 = 1;
    /// Payload: the sector.
    pub const READ: u8 = 2;
    pub const WRITE: u8 = 3;

    pub const OK: u8 = 0;
    /// The disk reported an error.
    pub const IO_ERROR: u8 = 1;
    /// No such sector.
    pub const OUT_OF_RANGE: u8 = 2;
    pub const READ_ONLY: u8 = 3;
    pub const BAD_REQUEST: u8 = 4;

    pub const REQUEST_SIZE: usize = 1 + 8;

    pub fn describe(status: u8) -> &'static str {
        match status {
            OK => "ok",
            IO_ERROR => "the disk reported an error",
            OUT_OF_RANGE => "no such sector",
            READ_ONLY => "the disk is read-only",
            _ => "the disk service did not understand",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::init::{Run, BUILT_IN, IMAGE};

    #[test]
    fn a_run_request_round_trips() {
        let run = Run { kind: IMAGE, flags: 5, name: "cat", arguments: b"notes welcome" };
        let mut message = [0u8; 64];
        let length = run.encode(&mut message).unwrap();
        assert_eq!(length, 3 + 3 + 13);
        let back = Run::decode(&message[..length]).unwrap();
        assert_eq!((back.kind, back.flags, back.name, back.arguments), (IMAGE, 5, "cat", &b"notes welcome"[..]));
        // No arguments is not the same as no request.
        let length = Run { kind: BUILT_IN, flags: 0, name: "hello", arguments: b"" }.encode(&mut message).unwrap();
        let back = Run::decode(&message[..length]).unwrap();
        assert_eq!((back.name, back.arguments.len()), ("hello", 0));
    }

    #[test]
    fn a_run_request_that_does_not_fit_is_refused() {
        let run = Run { kind: IMAGE, flags: 0, name: "cat", arguments: b"notes" };
        assert_eq!(run.encode(&mut [0u8; 10]), None);
        assert_eq!(run.encode(&mut [0u8; 11]), Some(11));
        let long = "n".repeat(256);
        assert_eq!(Run { kind: IMAGE, flags: 0, name: &long, arguments: b"" }.encode(&mut [0u8; 512]), None);
    }

    #[test]
    fn bad_run_requests_decode_to_nothing() {
        // Whatever arrives on the channel: never a panic, never a
        // request with a name that is not there.
        for bad in [
            &b""[..],
            b"\x01\x00",                // cut short
            b"\x01\x00\x00",            // no name
            b"\x01\x00\x05cat",         // the name claims more than there is
            b"\x03\x00\x03cat",         // no such kind
            b"\x01\x00\x02\xff\xfe",    // a name that is not text
            b"run hello",               // the old, text request
        ] {
            assert!(Run::decode(bad).is_none(), "{bad:?}");
        }
        assert!(Run::decode(b"\x01\x00\x03cat").is_some());
    }
}
