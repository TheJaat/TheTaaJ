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

/// The block device: requests, and its configuration.
pub mod block {
    pub const SECTOR: usize = 512;
    /// Feature bit: the disk is read-only.
    pub const FEATURE_READ_ONLY: u32 = 1 << 5;

    /// Request types.
    pub const READ: u32 = 0;
    pub const WRITE: u32 = 1;
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
}
