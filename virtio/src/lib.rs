//! virtio: the standard interface of virtual devices - disks, network
//! cards, consoles - that QEMU, KVM, cloud hypervisors and others offer.
//!
//! A virtio device and its driver share memory. The driver puts buffers
//! in a ring the device reads, the *available* ring: descriptors, each
//! naming a piece of physical memory. The device does the work, reading
//! and writing that memory itself (DMA), and puts the buffers back in a
//! second ring, the *used* ring. Together they are a virtqueue. The
//! registers are only for setting up and for saying "there is something
//! new" (a notify); the data never goes through them.
//!
//! This is the "legacy" interface (virtio 0.9.5) over PCI I/O ports:
//! fixed register offsets in BAR 0, one physically contiguous block of
//! memory per queue. QEMU's virtio devices offer it alongside the modern
//! one ("transitional" devices). The modern interface - registers in
//! memory BARs found through PCI capabilities - can come later; the
//! virtqueue itself is the same.
//!
//! Pure logic over a pointer to the queue's memory, so it is tested on
//! the host, against a simulated device.

#![cfg_attr(not(test), no_std)]

use core::sync::atomic::{fence, Ordering};

/// Vendor id of every virtio PCI device; and the transitional device
/// ids, 0x1000 + the device type.
pub const PCI_VENDOR: u16 = 0x1AF4;
pub const PCI_DEVICE_BLOCK: u16 = 0x1001;
/// And a network card that speaks the legacy interface.
pub const PCI_DEVICE_NETWORK: u16 = 0x1000;

/// The legacy registers, as offsets into the I/O BAR.
pub mod legacy {
    pub const HOST_FEATURES: u16 = 0x00; // u32: what the device offers
    pub const GUEST_FEATURES: u16 = 0x04; // u32: what the driver accepts
    pub const QUEUE_PFN: u16 = 0x08; // u32: queue memory, physical / 4096
    pub const QUEUE_SIZE: u16 = 0x0C; // u16: entries, fixed by the device
    pub const QUEUE_SELECT: u16 = 0x0E; // u16: which queue the above mean
    pub const QUEUE_NOTIFY: u16 = 0x10; // u16: "queue n has new buffers"
    pub const STATUS: u16 = 0x12; // u8: the steps of setting up
    pub const ISR: u16 = 0x13; // u8: why it interrupted (pin interrupts)
    /// With MSI-X on, two more registers: which MSI-X entry each kind
    /// of interrupt uses - and the device's own configuration moves
    /// down to make room.
    pub const MSIX_CONFIG_VECTOR: u16 = 0x14; // u16
    pub const MSIX_QUEUE_VECTOR: u16 = 0x16; // u16, for QUEUE_SELECT's queue
    pub const NO_VECTOR: u16 = 0xFFFF;

    pub const fn device_config(msix: bool) -> u16 {
        if msix {
            0x18
        } else {
            0x14
        }
    }

    /// Status bits, set one after another as setting up progresses.
    pub const ACKNOWLEDGE: u8 = 1; // a driver noticed the device
    pub const DRIVER: u8 = 2; // and knows how to drive it
    pub const DRIVER_OK: u8 = 4; // and is ready
    pub const FAILED: u8 = 128; // and gave up
}

/// virtio over memory-mapped registers ("virtio-mmio"): what machines
/// without a PCI bus have - QEMU's `virt` board among them. A device is
/// 512 bytes of registers at an address the machine's description
/// gives (the device tree), and one interrupt.
///
/// PCI says what a device is before any driver looks at it (vendor and
/// device ids in its configuration space). Here there is no such thing:
/// the description names only *places*, most of them empty, and what is
/// in a place is read from the place itself - the first four registers.
pub mod mmio {
    pub const MAGIC: u32 = 0x000; // "virt", so that nothing else is mistaken for one
    pub const VERSION: u32 = 0x004; // 1: the legacy interface; 2: the modern one
    pub const DEVICE_ID: u32 = 0x008; // what it is; 0 means the place is empty
    pub const VENDOR_ID: u32 = 0x00C;
    pub const HOST_FEATURES: u32 = 0x010; // what the device offers...
    pub const HOST_FEATURES_SELECT: u32 = 0x014; // ...32 bits at a time: which 32
    pub const GUEST_FEATURES: u32 = 0x020; // what the driver accepts
    pub const GUEST_FEATURES_SELECT: u32 = 0x024;
    pub const GUEST_PAGE_SIZE: u32 = 0x028; // legacy: the unit QUEUE_PFN counts in
    pub const QUEUE_SELECT: u32 = 0x030; // which queue the next five mean
    pub const QUEUE_SIZE_MAX: u32 = 0x034; // the most entries the device takes
    pub const QUEUE_SIZE: u32 = 0x038; // how many the driver uses
    pub const QUEUE_ALIGN: u32 = 0x03C; // legacy: alignment of the used ring
    pub const QUEUE_PFN: u32 = 0x040; // legacy: queue memory, physical / page size
    pub const QUEUE_NOTIFY: u32 = 0x050; // "queue n has new buffers"
    pub const INTERRUPT_STATUS: u32 = 0x060; // why it interrupted
    pub const INTERRUPT_ACK: u32 = 0x064; // "seen": the device lets go of the line
    pub const STATUS: u32 = 0x070; // the steps of setting up (as `legacy`)
    pub const CONFIG: u32 = 0x100; // the device's own configuration

    /// What MAGIC reads on a virtio device: the letters "virt".
    pub const MAGIC_VALUE: u32 = 0x7472_6976;

    /// Device kinds (DEVICE_ID).
    pub const DEVICE_NETWORK: u32 = 1;
    pub const DEVICE_BLOCK: u32 = 2;
    pub const DEVICE_CONSOLE: u32 = 3;
    pub const DEVICE_ENTROPY: u32 = 4;
    pub const DEVICE_GPU: u32 = 16;
    pub const DEVICE_INPUT: u32 = 18;

    /// INTERRUPT_STATUS: the device used a buffer.
    pub const INTERRUPT_USED: u32 = 1;
    /// INTERRUPT_STATUS: its configuration changed.
    pub const INTERRUPT_CONFIG: u32 = 2;

    /// How many entries to give a legacy queue: the most the device
    /// takes (`max`, what QUEUE_SIZE_MAX reads) that is a power of two
    /// and whose rings fit in `room` bytes. None if the queue does not
    /// exist (`max` 0) or not even two entries fit.
    ///
    /// On a PCI bus a legacy device fixes the size and the driver must
    /// find the memory. Here the driver chooses, so it can start from
    /// the memory it has.
    pub fn queue_size(max: u32, room: usize) -> Option<u16> {
        // 32768 is the largest size virtio allows.
        let mut size = max.min(32768).checked_ilog2().map(|log| 1u32 << log)?;
        while size >= 2 {
            if super::Layout::legacy(size as u16).total <= room {
                return Some(size as u16);
            }
            size /= 2;
        }
        None
    }

    /// What is in one place.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Identity {
        /// 1 (legacy) or 2 (modern).
        pub version: u32,
        /// The kind of device.
        pub device: u32,
        pub vendor: u32,
    }

    /// Why a place holds no device a driver could be given.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Nothing {
        /// A virtio place with nothing plugged into it: the usual case.
        Empty,
        /// The magic number is not there: these are not virtio
        /// registers at all, whatever the description said.
        NotVirtio { magic: u32 },
        /// A version of the interface this code has not heard of.
        UnknownVersion { version: u32 },
    }

    /// Reads what is at one place, through `read` (a register offset in,
    /// its value out). The order matters: nothing but the magic number
    /// may be believed until the magic number is right.
    pub fn identify(read: impl Fn(u32) -> u32) -> Result<Identity, Nothing> {
        let magic = read(MAGIC);
        if magic != MAGIC_VALUE {
            return Err(Nothing::NotVirtio { magic });
        }
        let version = read(VERSION);
        if version != 1 && version != 2 {
            return Err(Nothing::UnknownVersion { version });
        }
        let device = read(DEVICE_ID);
        if device == 0 {
            return Err(Nothing::Empty);
        }
        Ok(Identity { version, device, vendor: read(VENDOR_ID) })
    }

    impl Identity {
        /// The kind of device, in words.
        pub const fn kind(&self) -> &'static str {
            match self.device {
                DEVICE_NETWORK => "network card",
                DEVICE_BLOCK => "block device",
                DEVICE_CONSOLE => "console",
                DEVICE_ENTROPY => "source of random numbers",
                DEVICE_GPU => "display",
                DEVICE_INPUT => "input device",
                _ => "device of a kind not known here",
            }
        }

        /// Which of the two interfaces it speaks, in words.
        pub const fn interface(&self) -> &'static str {
            if self.version == 1 {
                "legacy"
            } else {
                "modern"
            }
        }
    }
}

/// The block device: requests, and its configuration.
pub mod block {
    pub const SECTOR: usize = 512;
    /// Feature bit: the disk is read-only.
    pub const FEATURE_READ_ONLY: u32 = 1 << 5;
    /// Feature bit: the disk may keep written sectors in a cache, and
    /// takes FLUSH requests to put them on the medium. A driver that
    /// does not accept it gets a disk that writes through - each write
    /// on the medium before it is answered - which is safe and slow:
    /// QEMU then has its host write every request out at once.
    pub const FEATURE_FLUSH: u32 = 1 << 9;

    /// Request types.
    pub const READ: u32 = 0;
    pub const WRITE: u32 = 1;
    /// Everything written so far onto the medium. No data buffer: the
    /// header and the status byte only.
    pub const FLUSH: u32 = 4;
    /// Status the device writes in the request's last byte.
    pub const OK: u8 = 0;
    pub const IO_ERROR: u8 = 1;
    pub const UNSUPPORTED: u8 = 2;

    /// A request is three buffers: this header (device reads), the data
    /// (device reads for a write, writes for a read), and one status
    /// byte (device writes).
    pub const HEADER_SIZE: usize = 16;

    pub fn header(kind: u32, sector: u64) -> [u8; HEADER_SIZE] {
        let mut header = [0u8; HEADER_SIZE];
        header[0..4].copy_from_slice(&kind.to_le_bytes());
        // 4..8 reserved
        header[8..16].copy_from_slice(&sector.to_le_bytes());
        header
    }
}

/// The network card: its queues, what goes before each frame, and its
/// configuration.
pub mod net {
    /// Frames the card received come back through queue 0; frames to
    /// send go into queue 1.
    pub const QUEUE_RECEIVE: u16 = 0;
    pub const QUEUE_TRANSMIT: u16 = 1;

    /// Feature bit: the card has an address of its own, in its
    /// configuration.
    pub const FEATURE_MAC: u32 = 1 << 5;
    /// Feature bit: the configuration says whether the link is up.
    pub const FEATURE_STATUS: u32 = 1 << 16;
    /// What this driver asks for, of what a card offers. Nothing else:
    /// no checksums left to the card, no frames merged or split - each
    /// buffer is one whole frame, exactly as it is on the wire.
    pub const fn accepted(offered: u32) -> u32 {
        offered & (FEATURE_MAC | FEATURE_STATUS)
    }

    /// In the configuration: the address, six bytes from here...
    pub const CONFIG_MAC: u32 = 0;
    /// ...and the status, 16 bits, whose lowest bit is "the link is up".
    pub const CONFIG_STATUS: u32 = 6;
    pub const STATUS_LINK_UP: u16 = 1;

    /// Every frame, in both directions, has this many bytes in front
    /// of it that are not on the wire: a note between driver and card
    /// about checksums and oversized frames. With none of those
    /// features accepted it is all zeroes going out, and ignored
    /// coming in.
    pub const HEADER_SIZE: usize = 10;

    /// Whether the link is up, from what was accepted and the status
    /// in the configuration. A card that does not say is taken to be
    /// connected.
    pub const fn link_up(accepted: u32, status: u16) -> bool {
        accepted & FEATURE_STATUS == 0 || status & STATUS_LINK_UP != 0
    }
}

/// The input device (virtio-input): keyboards, mice and tablets. One
/// queue of events from the device - eight bytes each, Linux's input
/// events - and a configuration that answers questions: write what is
/// asked into `select` and `subsel`, then read `size` and that many
/// bytes of the answer.
pub mod input {
    /// Events come back through queue 0. (Queue 1 is for the driver to
    /// send to the device - a keyboard's lights - and is not used.)
    pub const QUEUE_EVENTS: u16 = 0;
    /// One event: type (u16), code (u16), value (u32), little-endian.
    pub const EVENT_SIZE: usize = 8;

    /// The configuration's fields, from its start.
    pub const CONFIG_SELECT: u32 = 0;
    pub const CONFIG_SUBSEL: u32 = 1;
    pub const CONFIG_SIZE: u32 = 2;
    /// Where the answer starts; at most 128 bytes of it.
    pub const CONFIG_ANSWER: u32 = 8;
    pub const ANSWER_MAX: usize = 128;

    /// Questions (`select`): the device's name, as text...
    pub const SELECT_NAME: u8 = 0x01;
    /// ...which codes of event type `subsel` it sends, a bit each...
    pub const SELECT_EVENT_BITS: u8 = 0x11;
    /// ...and the range of absolute axis `subsel`.
    pub const SELECT_ABS_INFO: u8 = 0x12;

    /// Event types, as the bits question names them.
    pub const EV_KEY: u8 = 0x01;
    pub const EV_REL: u8 = 0x02;
    pub const EV_ABS: u8 = 0x03;
    /// A key every keyboard has: 'A' (Linux's KEY_A).
    pub const KEY_A: u16 = 30;
    pub const BTN_LEFT: u16 = 0x110;

    /// One event, from its eight bytes.
    pub fn event(bytes: &[u8; EVENT_SIZE]) -> (u16, u16, u32) {
        (
            u16::from_le_bytes([bytes[0], bytes[1]]),
            u16::from_le_bytes([bytes[2], bytes[3]]),
            u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
        )
    }

    /// Whether bit `n` is set in a bits answer (bit 0 is the low bit of
    /// the first byte).
    pub fn has_bit(bits: &[u8], n: u16) -> bool {
        bits.get(usize::from(n / 8)).is_some_and(|byte| byte & (1 << (n % 8)) != 0)
    }

    /// An absolute axis's range, from the first eight bytes of its
    /// answer: (min, max), each a u32 that holds an i32.
    pub fn abs_range(answer: &[u8]) -> Option<(i32, i32)> {
        let min = answer.get(0..4)?;
        let max = answer.get(4..8)?;
        Some((i32::from_le_bytes(min.try_into().ok()?), i32::from_le_bytes(max.try_into().ok()?)))
    }

    /// What a device is, from what it says it sends.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum Kind {
        Keyboard,
        /// Says how far it moved.
        Mouse,
        /// Says where it is.
        Tablet,
        /// Nothing this system uses.
        Other,
    }

    /// The kind, from the answers to "which keys?", "which relative
    /// axes?" and "which absolute axes?". A keyboard has an 'A'; a
    /// pointer has a left button and axes.
    pub fn kind(keys: &[u8], relative: &[u8], absolute: &[u8]) -> Kind {
        if has_bit(keys, KEY_A) {
            Kind::Keyboard
        } else if has_bit(absolute, 0) && has_bit(absolute, 1) {
            Kind::Tablet
        } else if has_bit(relative, 0) && has_bit(relative, 1) {
            Kind::Mouse
        } else {
            Kind::Other
        }
    }
}

/// Descriptor flags.
pub const DESC_NEXT: u16 = 1;
pub const DESC_WRITE: u16 = 2; // the device writes this buffer

const DESC_SIZE: usize = 16;
const PAGE: usize = 4096;

/// Where each part of a legacy queue of `size` entries lies, from the
/// start of its memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    pub size: u16,
    pub available: usize,
    pub used: usize,
    pub total: usize,
}

impl Layout {
    /// Descriptors, then the available ring, then - on the next page -
    /// the used ring. Legacy devices compute the same from the size, so
    /// only the start is ever told to them.
    pub const fn legacy(size: u16) -> Layout {
        let n = size as usize;
        let available = DESC_SIZE * n;
        let available_end = available + 2 + 2 + 2 * n + 2; // flags, idx, ring, used_event
        let used = available_end.next_multiple_of(PAGE);
        let used_end = used + 2 + 2 + 8 * n + 2; // flags, idx, ring of (id, len), avail_event
        Layout { size, available, used, total: used_end.next_multiple_of(PAGE) }
    }
}

/// One buffer of a request: physical address, length, and whether the
/// device writes it (else reads it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Buffer {
    pub physical: u64,
    pub length: u32,
    pub device_writes: bool,
}

/// The driver's side of one virtqueue.
///
/// Free descriptors form a list threaded through their own `next`
/// fields, so the queue needs no memory beyond the ring itself.
pub struct Queue {
    base: *mut u8,
    layout: Layout,
    free_head: u16,
    free_count: u16,
    /// The next entry of the available ring to fill.
    next_available: u16,
    /// The used-ring index already looked at.
    last_used: u16,
}

// SAFETY: the pointer is to memory the queue owns; it can move between
// threads with it.
unsafe impl Send for Queue {}

impl Queue {
    /// Sets up a queue in the `layout.total` bytes at `base`, zeroing
    /// them.
    ///
    /// # Safety
    /// `base` must be valid for `layout.total` bytes, 4096-aligned, and
    /// used for nothing else while the queue (or the device) uses it.
    pub unsafe fn new(base: *mut u8, layout: Layout) -> Queue {
        base.write_bytes(0, layout.total);
        let mut queue = Queue { base, layout, free_head: 0, free_count: layout.size, next_available: 0, last_used: 0 };
        for n in 0..layout.size {
            queue.write16(n as usize * DESC_SIZE + 14, n.wrapping_add(1)); // next: the one after
        }
        queue
    }

    pub fn size(&self) -> u16 {
        self.layout.size
    }

    /// Asks the device not to interrupt when it has finished with a
    /// buffer of this queue (or, with false, to do so again). A hint,
    /// which a device may ignore: for a queue the driver looks at when
    /// it wants to anyway. (The first 16 bits of the available ring;
    /// bit 0 is "no interrupt, please".)
    pub fn quiet(&mut self, quiet: bool) {
        self.write16(self.layout.available, u16::from(quiet));
    }

    pub fn free(&self) -> u16 {
        self.free_count
    }

    // Volatile: the device reads and writes this memory behind the
    // compiler's back.
    fn write16(&mut self, offset: usize, value: u16) {
        // SAFETY: `new`'s contract; every offset is inside the layout.
        unsafe { (self.base.add(offset) as *mut u16).write_volatile(value) }
    }

    fn read16(&self, offset: usize) -> u16 {
        // SAFETY: as for write16.
        unsafe { (self.base.add(offset) as *const u16).read_volatile() }
    }

    fn read32(&self, offset: usize) -> u32 {
        // SAFETY: as for write16.
        unsafe { (self.base.add(offset) as *const u32).read_volatile() }
    }

    fn write_descriptor(&mut self, index: u16, buffer: &Buffer, next: Option<u16>) {
        let at = index as usize * DESC_SIZE;
        // SAFETY: as for write16.
        unsafe {
            (self.base.add(at) as *mut u64).write_volatile(buffer.physical);
            (self.base.add(at + 8) as *mut u32).write_volatile(buffer.length);
        }
        let flags = if buffer.device_writes { DESC_WRITE } else { 0 } | if next.is_some() { DESC_NEXT } else { 0 };
        self.write16(at + 12, flags);
        self.write16(at + 14, next.unwrap_or(0));
    }

    /// Offers the device one request made of `buffers`, chained in that
    /// order. Returns the chain's first descriptor - its id when it
    /// comes back - or None if the queue has too few free descriptors.
    /// The device has to be notified afterwards.
    pub fn submit(&mut self, buffers: &[Buffer]) -> Option<u16> {
        if buffers.is_empty() || buffers.len() > self.free_count as usize {
            return None;
        }
        // Take descriptors off the free list, and fill them.
        let head = self.free_head;
        let mut index = head;
        for (n, buffer) in buffers.iter().enumerate() {
            let next_free = self.read16(index as usize * DESC_SIZE + 14);
            let last = n + 1 == buffers.len();
            self.write_descriptor(index, buffer, (!last).then_some(next_free));
            if last {
                self.free_head = next_free;
            }
            index = next_free;
        }
        self.free_count -= buffers.len() as u16;

        // Into the available ring - and only then make it visible by
        // moving the index: the device may look the moment it moves.
        let slot = self.layout.available + 4 + 2 * (self.next_available % self.layout.size) as usize;
        self.write16(slot, head);
        self.next_available = self.next_available.wrapping_add(1);
        fence(Ordering::SeqCst);
        self.write16(self.layout.available + 2, self.next_available);
        fence(Ordering::SeqCst);
        Some(head)
    }

    /// A request the device has finished: (its id, bytes it wrote). Its
    /// descriptors are free again.
    pub fn take_used(&mut self) -> Option<(u16, u32)> {
        if self.read16(self.layout.used + 2) == self.last_used {
            return None;
        }
        fence(Ordering::SeqCst); // read the entry only after seeing the index
        let entry = self.layout.used + 4 + 8 * (self.last_used % self.layout.size) as usize;
        let (id, length) = (self.read32(entry) as u16, self.read32(entry + 4));
        self.last_used = self.last_used.wrapping_add(1);

        // Back onto the free list: walk to the chain's end, link it in.
        let mut tail = id;
        let mut count = 1;
        while self.read16(tail as usize * DESC_SIZE + 12) & DESC_NEXT != 0 {
            tail = self.read16(tail as usize * DESC_SIZE + 14);
            count += 1;
        }
        let free_head = self.free_head;
        self.write16(tail as usize * DESC_SIZE + 14, free_head);
        self.free_head = id;
        self.free_count += count;
        Some((id, length))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_input_device_is_known_by_what_it_sends() {
        use input::*;
        assert_eq!(event(&[1, 0, 30, 0, 1, 0, 0, 0]), (1, 30, 1));
        assert_eq!(event(&[3, 0, 1, 0, 0xFF, 0x7F, 0, 0]), (3, 1, 0x7FFF));
        assert_eq!(event(&[2, 0, 0, 0, 0xFE, 0xFF, 0xFF, 0xFF]).2 as i32, -2);
        // Bits: KEY_A is bit 30 - byte 3, bit 6.
        let mut keys = [0u8; 16];
        keys[3] = 1 << 6;
        assert!(has_bit(&keys, KEY_A) && !has_bit(&keys, 29) && !has_bit(&keys, 500));
        let mut buttons = [0u8; 40];
        buttons[(BTN_LEFT / 8) as usize] = 1 << (BTN_LEFT % 8);
        assert_eq!(kind(&keys, &[], &[]), Kind::Keyboard);
        assert_eq!(kind(&buttons, &[0b11], &[]), Kind::Mouse);
        assert_eq!(kind(&buttons, &[], &[0b11]), Kind::Tablet);
        assert_eq!(kind(&buttons, &[0b01], &[0b10]), Kind::Other);
        assert_eq!(kind(&[], &[], &[]), Kind::Other);
        assert_eq!(abs_range(&[0, 0, 0, 0, 0xFF, 0x7F, 0, 0, 9, 9]), Some((0, 32767)));
        assert_eq!(abs_range(&[0xFF, 0xFF, 0xFF, 0xFF, 1, 0, 0, 0]), Some((-1, 1)));
        assert_eq!(abs_range(&[1, 2, 3]), None);
    }

    /// A fake set of registers: the first four, as given.
    fn registers(values: [u32; 4]) -> impl Fn(u32) -> u32 {
        move |offset| {
            assert!(offset % 4 == 0 && offset < 16, "read of register {offset:#x}");
            values[offset as usize / 4]
        }
    }

    #[test]
    fn a_queue_is_as_big_as_the_device_and_the_memory_allow() {
        use mmio::queue_size;
        // QEMU's answer (1024) and the block driver's 32 KiB: all of
        // it fits, exactly.
        assert_eq!(Layout::legacy(1024).total, 0x8000);
        assert_eq!(queue_size(1024, 0x8000), Some(1024));
        // One byte less room: the next size down.
        assert_eq!(queue_size(1024, 0x7FFF), Some(512));
        // The device's limit, when that is the smaller.
        assert_eq!(queue_size(8, 0x8000), Some(8));
        // Not a power of two: the one below.
        assert_eq!(queue_size(100, 0x8000), Some(64));
        // More than virtio allows is not believed.
        assert_eq!(queue_size(u32::MAX, usize::MAX), Some(32768));
        // No queue; no room for any.
        assert_eq!(queue_size(0, 0x8000), None);
        assert_eq!(queue_size(1024, 0x1000), None);
        assert_eq!(queue_size(1, 0x8000), None);
        // Whatever is chosen fits.
        for room in [0x2000, 0x3000, 0x5000, 0x10000] {
            let size = queue_size(4096, room).unwrap();
            assert!(Layout::legacy(size).total <= room);
            assert!(Layout::legacy(size * 2).total > room || size == 4096);
        }
    }

    #[test]
    fn a_place_says_what_is_in_it() {
        use mmio::{identify, Identity, Nothing};
        let disk = identify(registers([0x7472_6976, 1, 2, 0x554D_4551])).unwrap();
        assert_eq!(disk, Identity { version: 1, device: 2, vendor: 0x554D_4551 });
        assert_eq!((disk.kind(), disk.interface()), ("block device", "legacy"));
        let modern = identify(registers([0x7472_6976, 2, 4, 0])).unwrap();
        assert_eq!((modern.kind(), modern.interface()), ("source of random numbers", "modern"));
        assert_eq!(identify(registers([0x7472_6976, 2, 99, 0])).unwrap().kind(), "device of a kind not known here");
        // Empty: the usual case, and not an error to report.
        assert_eq!(identify(registers([0x7472_6976, 1, 0, 0x554D_4551])), Err(Nothing::Empty));
        assert_eq!(identify(registers([0x7472_6976, 3, 2, 0])), Err(Nothing::UnknownVersion { version: 3 }));
    }

    #[test]
    fn nothing_is_believed_before_the_magic_number() {
        use mmio::{identify, Nothing, MAGIC};
        // Not virtio registers: only the first one may be read.
        let read = |offset| {
            assert_eq!(offset, MAGIC, "read past a wrong magic number");
            0xFFFF_FFFF
        };
        assert_eq!(identify(read), Err(Nothing::NotVirtio { magic: 0xFFFF_FFFF }));
        assert_eq!(&mmio::MAGIC_VALUE.to_le_bytes(), b"virt");
    }

    /// Page-aligned memory, with "physical" addresses = host addresses.
    struct Memory(*mut u8, usize);

    impl Memory {
        fn new(bytes: usize) -> Memory {
            let layout = std::alloc::Layout::from_size_align(bytes, PAGE).unwrap();
            // SAFETY: non-zero size; freed in Drop.
            Memory(unsafe { std::alloc::alloc_zeroed(layout) }, bytes)
        }
    }

    impl Drop for Memory {
        fn drop(&mut self) {
            // SAFETY: allocated in new with this layout.
            unsafe { std::alloc::dealloc(self.0, std::alloc::Layout::from_size_align(self.1, PAGE).unwrap()) }
        }
    }

    /// A simulated block device: reads the available ring the way the
    /// hardware does, carries out each request on a disk held in a Vec,
    /// and returns it on the used ring.
    struct Disk {
        base: *mut u8,
        layout: Layout,
        seen: u16,
        sectors: Vec<[u8; 512]>,
    }

    unsafe fn rd<T: Copy>(address: u64) -> T {
        (address as *const T).read_volatile()
    }

    unsafe fn wr<T>(address: u64, value: T) {
        (address as *mut T).write_volatile(value)
    }

    impl Disk {
        /// Handles everything new; returns how many requests.
        fn run(&mut self) -> usize {
            let base = self.base as u64;
            let available = base + self.layout.available as u64;
            let used = base + self.layout.used as u64;
            let mut handled = 0;
            // SAFETY: the test's queue memory and buffers.
            unsafe {
                while self.seen != rd::<u16>(available + 2) {
                    let head: u16 = rd(available + 4 + 2 * (self.seen % self.layout.size) as u64);
                    // Collect the chain.
                    let mut chain = Vec::new();
                    let mut index = head;
                    loop {
                        let d = base + 16 * index as u64;
                        let (address, length, flags, next) = (rd::<u64>(d), rd::<u32>(d + 8), rd::<u16>(d + 12), rd::<u16>(d + 14));
                        chain.push((address, length, flags));
                        if flags & DESC_NEXT == 0 {
                            break;
                        }
                        index = next;
                    }
                    let (header, data, status) = (chain[0], chain[1], chain[2]);
                    assert_eq!(header.2 & DESC_WRITE, 0, "the header is the device's to read");
                    assert_ne!(status.2 & DESC_WRITE, 0, "the status is the device's to write");
                    let kind: u32 = rd(header.0);
                    let sector: u64 = rd(header.0 + 8);
                    let mut written = 1;
                    let result = match (kind, self.sectors.get_mut(sector as usize)) {
                        (block::READ, Some(contents)) => {
                            assert_ne!(data.2 & DESC_WRITE, 0);
                            std::ptr::copy_nonoverlapping(contents.as_ptr(), data.0 as *mut u8, 512);
                            written += 512;
                            block::OK
                        }
                        (block::WRITE, Some(contents)) => {
                            assert_eq!(data.2 & DESC_WRITE, 0);
                            std::ptr::copy_nonoverlapping(data.0 as *const u8, contents.as_mut_ptr(), 512);
                            block::OK
                        }
                        (block::READ | block::WRITE, None) => block::IO_ERROR,
                        _ => block::UNSUPPORTED,
                    };
                    wr(status.0, result);
                    let used_index: u16 = rd(used + 2);
                    let entry = used + 4 + 8 * (used_index % self.layout.size) as u64;
                    wr(entry, u32::from(head));
                    wr(entry + 4, written as u32);
                    wr(used + 2, used_index.wrapping_add(1));
                    self.seen = self.seen.wrapping_add(1);
                    handled += 1;
                }
            }
            handled
        }
    }

    /// Aligned as the driver's DMA memory is: the device reads the
    /// header's sector as a u64.
    #[repr(C, align(16))]
    struct Request {
        header: [u8; 16],
        data: [u8; 512],
        status: u8,
    }

    fn buffers(r: &mut Request, read: bool) -> [Buffer; 3] {
        [
            Buffer { physical: r.header.as_ptr() as u64, length: 16, device_writes: false },
            Buffer { physical: r.data.as_mut_ptr() as u64, length: 512, device_writes: read },
            Buffer { physical: &mut r.status as *mut u8 as u64, length: 1, device_writes: true },
        ]
    }

    fn setup(size: u16) -> (Memory, Queue, Disk) {
        let layout = Layout::legacy(size);
        let memory = Memory::new(layout.total);
        // SAFETY: fresh, aligned, big enough, used only by these two.
        let queue = unsafe { Queue::new(memory.0, layout) };
        let disk = Disk { base: memory.0, layout, seen: 0, sectors: vec![[0; 512]; 64] };
        (memory, queue, disk)
    }

    #[test]
    fn layouts_match_the_specification() {
        // virtio 0.9.5, 2.3: for 256 entries, descriptors take 4096
        // bytes, the available ring 518, and the used ring starts on the
        // next page boundary.
        assert_eq!(Layout::legacy(256), Layout { size: 256, available: 4096, used: 8192, total: 12288 });
        assert_eq!(Layout::legacy(128), Layout { size: 128, available: 2048, used: 4096, total: 8192 });
        assert_eq!(Layout::legacy(1024), Layout { size: 1024, available: 16384, used: 20480, total: 32768 });
    }

    #[test]
    fn write_then_read_back() {
        let (_memory, mut queue, mut disk) = setup(8);
        let mut write = Request { header: block::header(block::WRITE, 5), data: [0xAB; 512], status: 0xFF };
        let id = queue.submit(&buffers(&mut write, false)).unwrap();
        assert_eq!(queue.free(), 5);
        assert_eq!(queue.take_used(), None, "not done before the device runs");
        assert_eq!(disk.run(), 1);
        assert_eq!(queue.take_used(), Some((id, 1)));
        assert_eq!((write.status, queue.free()), (block::OK, 8));
        assert_eq!(disk.sectors[5], [0xAB; 512]);

        let mut read = Request { header: block::header(block::READ, 5), data: [0; 512], status: 0xFF };
        queue.submit(&buffers(&mut read, true)).unwrap();
        disk.run();
        assert_eq!(queue.take_used().map(|(_, n)| n), Some(513));
        assert_eq!((read.status, read.data), (block::OK, [0xAB; 512]));
    }

    #[test]
    fn errors_come_back_in_the_status() {
        let (_memory, mut queue, mut disk) = setup(8);
        let mut read = Request { header: block::header(block::READ, 1000), data: [0; 512], status: 0xFF };
        queue.submit(&buffers(&mut read, true)).unwrap();
        disk.run();
        assert!(queue.take_used().is_some());
        assert_eq!(read.status, block::IO_ERROR, "past the end of the disk");
    }

    #[test]
    fn a_full_queue_refuses_and_recovers() {
        let (_memory, mut queue, mut disk) = setup(8);
        let mut requests: Vec<Request> =
            (0..3).map(|n| Request { header: block::header(block::WRITE, n), data: [n as u8; 512], status: 0xFF }).collect();
        // 8 descriptors: two requests of three fit, a third does not.
        let ids: Vec<Option<u16>> = requests.iter_mut().map(|r| queue.submit(&buffers(r, false))).collect();
        assert!(ids[0].is_some() && ids[1].is_some() && ids[2].is_none());
        assert_eq!(queue.submit(&[]), None, "an empty request");
        assert_eq!(disk.run(), 2);
        let done: Vec<u16> = std::iter::from_fn(|| queue.take_used().map(|(id, _)| id)).collect();
        assert_eq!(done, vec![ids[0].unwrap(), ids[1].unwrap()]);
        assert_eq!(queue.free(), 8, "every descriptor back");
        assert!(queue.submit(&buffers(&mut requests[2], false)).is_some());
    }

    #[test]
    fn indexes_wrap_around() {
        // The ring indexes are 16-bit and run forever: past 65535 they
        // wrap, and slot = index % size must keep working.
        let (_memory, mut queue, mut disk) = setup(4);
        let mut r = Request { header: [0; 16], data: [0; 512], status: 0 };
        for n in 0..70_000u32 {
            r.header = block::header(block::WRITE, u64::from(n % 64));
            r.data[0] = n as u8;
            queue.submit(&buffers(&mut r, false)).unwrap();
            assert_eq!(disk.run(), 1);
            assert!(queue.take_used().is_some(), "request {n}");
            assert_eq!(disk.sectors[(n % 64) as usize][0], n as u8);
        }
        assert_eq!(queue.free(), 4);
    }
    /// A card that hands buffers back in an order of its own: what a
    /// receive queue sees, where every buffer is offered ahead of time
    /// and comes back when a frame happens to arrive.
    #[test]
    fn single_buffers_come_back_in_any_order_and_are_reusable() {
        let layout = Layout::legacy(8);
        let memory = Memory::new(layout.total);
        // SAFETY: `memory` is page-aligned, zeroed, and big enough.
        let mut queue = unsafe { Queue::new(memory.0, layout) };
        let ids: Vec<u16> = (0..8u64)
            .map(|n| queue.submit(&[Buffer { physical: 0x1000 * (n + 1), length: 2048, device_writes: true }]).unwrap())
            .collect();
        assert_eq!(queue.free(), 0);
        assert!(queue.submit(&[Buffer { physical: 0x9000, length: 2048, device_writes: true }]).is_none());
        // Every buffer got a descriptor of its own.
        let mut seen = ids.clone();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), 8);

        // The device uses them last first, writing a different length
        // into each.
        // SAFETY: the test's queue memory.
        unsafe {
            for (n, &id) in ids.iter().rev().enumerate() {
                let entry = memory.0.add(layout.used + 4 + 8 * n);
                (entry as *mut u32).write(u32::from(id));
                (entry.add(4) as *mut u32).write(100 + n as u32);
            }
            (memory.0.add(layout.used + 2) as *mut u16).write(8);
        }
        for (n, &id) in ids.iter().rev().enumerate() {
            assert_eq!(queue.take_used(), Some((id, 100 + n as u32)));
        }
        assert_eq!(queue.take_used(), None);
        assert_eq!(queue.free(), 8);
        // And all eight can be offered again.
        for n in 0..8u64 {
            assert!(queue.submit(&[Buffer { physical: 0x1000 * (n + 1), length: 2048, device_writes: true }]).is_some());
        }
    }

    #[test]
    fn a_queue_can_ask_for_quiet() {
        let layout = Layout::legacy(8);
        let memory = Memory::new(layout.total);
        // SAFETY: as above.
        let mut queue = unsafe { Queue::new(memory.0, layout) };
        // SAFETY: the available ring's first 16 bits.
        let flags = || unsafe { (memory.0.add(layout.available) as *const u16).read() };
        assert_eq!(flags(), 0);
        queue.quiet(true);
        assert_eq!(flags(), 1);
        queue.quiet(false);
        assert_eq!(flags(), 0);
    }

    #[test]
    fn a_network_card_is_asked_for_its_address_and_its_link_and_nothing_more() {
        // QEMU offers a long list (checksums, merged buffers, a control
        // queue...): only two of them are taken.
        let offered = 0x79bf_ffe7;
        assert_eq!(net::accepted(offered), net::FEATURE_MAC | net::FEATURE_STATUS);
        assert_eq!(net::accepted(net::FEATURE_MAC), net::FEATURE_MAC);
        assert_eq!(net::accepted(0), 0);
        // The link: what the status says, if the card says at all.
        assert!(net::link_up(net::FEATURE_STATUS, 1));
        assert!(!net::link_up(net::FEATURE_STATUS, 0));
        assert!(net::link_up(0, 0));
    }
}
