//! hid - the driver of virtio input devices: keyboards, mice and
//! tablets ("human interface devices").
//!
//! Startup handles, from init:
//!   1  the device channel: a message per device - its MmioDeviceInfo,
//!      with its registers, its DMA memory and (if it has one) its
//!      interrupt as the message's handles
//!   2  the keys' channel: terminal bytes, as the PC's keyboard driver
//!      sends them (send only)
//!   3  the display's channel: pointer reports (send only;
//!      abi::display::POINTER)
//!   4  pings from init (abi::PING)
//!
//! One driver for every input device, however many: init sends each as
//! a message carrying its handles, the way the kernel sent them to init,
//! so this program has no fixed place for any one device. A restarted
//! driver is sent them all again.
//!
//! A virtio input device speaks first, like a network card: an event
//! happens when someone presses a key. So its one queue is kept full of
//! empty eight-byte buffers; the device fills one per event (Linux's
//! input event: type, code, value) and hands it back, and the driver
//! reads it and offers the buffer again.
//!
//! What a device is, it says itself: asked "which keys do you send?",
//! a keyboard's answer includes 'A'; a tablet's "which absolute axes?"
//! includes X and Y. Keys go through the same decoder as the PS/2
//! keyboard's - a virtio key code is turned into the set-1 scancode for
//! the same key (`input::evdev`) - so Shift, Ctrl and the layout are
//! decided in one place for both. A pointer's events are gathered into
//! reports (`input::pointer::Events`) and sent to the display.
//!
//! The DMA memory of each device: the queue in its first two pages,
//! the event buffers in the third.

#![no_std]
#![no_main]

use abi::display::Pointer;
use abi::MmioDeviceInfo;
use input::pointer::{Events, Report};
use input::{evdev, terminal_bytes, Decoder};
use taaj::{println, Error, Handle};
use virtio::input::{self as vinput, Kind, EVENT_SIZE};
use virtio::{legacy, mmio, Buffer, Layout, Queue};

taaj::program!(main);

const DEVICES_MAX: usize = 4;
/// The queue's share of the DMA memory, and where the events begin.
const QUEUE_AREA: usize = 0x2000;
const EVENTS_AT: usize = QUEUE_AREA;
/// Buffers offered at once: events a device can send before the
/// driver has looked. Each is eight bytes.
const BUFFERS_MAX: usize = 64;
const DMA_NEEDED: u64 = (EVENTS_AT + BUFFERS_MAX * EVENT_SIZE) as u64;
/// The most descriptors a queue in QUEUE_AREA can have.
const DESCRIPTORS_MAX: usize = 256;

/// One device, started.
struct Device {
    registers: *mut u32,
    memory: *mut u8,
    physical: u64,
    queue: Queue,
    interrupt: Option<Handle>,
    /// Which buffer each descriptor carries: the queue hands out
    /// descriptors as it pleases, and the device names a finished
    /// buffer by its descriptor.
    carried: [u8; DESCRIPTORS_MAX],
    kind: Kind,
    keys: Decoder,
    pointer: Events,
}

/// A register of a device's, by its offset.
///
/// # Safety
/// `registers` must be a mapping of a virtio-mmio device's registers.
unsafe fn read(registers: *mut u32, offset: u32) -> u32 {
    // SAFETY: the caller's promise; volatile, as every device register.
    unsafe { registers.byte_add(offset as usize).read_volatile() }
}

/// # Safety
/// As for `read`.
unsafe fn write(registers: *mut u32, offset: u32, value: u32) {
    // SAFETY: as for `read`.
    unsafe { registers.byte_add(offset as usize).write_volatile(value) }
}

/// A question to the device's configuration: `select` and `subsel`
/// written, the answer's size and bytes read - a byte at a time, as
/// the specification asks of fields narrower than a register.
///
/// # Safety
/// As for `read`; the registers must reach CONFIG + 8 + ANSWER_MAX.
unsafe fn ask(registers: *mut u32, select: u8, subsel: u8, answer: &mut [u8; vinput::ANSWER_MAX]) -> usize {
    let config = registers.cast::<u8>();
    // SAFETY: the caller's promise.
    unsafe {
        config.add((mmio::CONFIG + vinput::CONFIG_SELECT) as usize).write_volatile(select);
        config.add((mmio::CONFIG + vinput::CONFIG_SUBSEL) as usize).write_volatile(subsel);
        let size = usize::from(config.add((mmio::CONFIG + vinput::CONFIG_SIZE) as usize).read_volatile()).min(vinput::ANSWER_MAX);
        for (n, byte) in answer.iter_mut().enumerate().take(size) {
            *byte = config.add((mmio::CONFIG + vinput::CONFIG_ANSWER) as usize + n).read_volatile();
        }
        size
    }
}

fn map(memory: Handle, what: &str) -> Option<*mut u8> {
    match taaj::memory_map(memory, true) {
        Ok(base) => Some(base),
        Err(error) => {
            println!("[hid] cannot map {what}: {error:?}");
            None
        }
    }
}

impl Device {
    /// Starts the device the message describes: checks the description
    /// against what the kernel says the handles are, maps them, asks
    /// the device what it is, sets up its queue and fills it.
    fn start(info: &MmioDeviceInfo, registers: Handle, dma: Handle, interrupt: Option<Handle>) -> Result<Device, &'static str> {
        let end = info.offset_in_page() as u64 + u64::from(info.registers_size);
        let fits = taaj::memory_size(registers).is_ok_and(|size| end <= size);
        let room = u64::from(mmio::CONFIG) + u64::from(vinput::CONFIG_ANSWER) + vinput::ANSWER_MAX as u64;
        if info.dma_size < DMA_NEEDED || u64::from(info.registers_size) < room || !fits {
            return Err("no usable device description");
        }
        let page = map(registers, "the device's registers").ok_or("its registers cannot be mapped")?;
        let memory = map(dma, "the DMA memory").ok_or("its DMA memory cannot be mapped")?;
        // SAFETY: inside the mapping (checked above).
        let registers = unsafe { page.add(info.offset_in_page()) }.cast::<u32>();
        if !registers.is_aligned() {
            return Err("no usable device description");
        }
        // SAFETY (every register access below): a mapping of the device's
        // registers, checked to reach as far as the configuration's end.
        unsafe {
            let status = |bits: u8| write(registers, mmio::STATUS, u32::from(bits));
            if read(registers, mmio::MAGIC) != mmio::MAGIC_VALUE || read(registers, mmio::DEVICE_ID) != mmio::DEVICE_INPUT {
                return Err("these are not a virtio input device's registers");
            }
            if read(registers, mmio::VERSION) != 1 {
                return Err("only the legacy register interface is spoken here");
            }
            status(0); // reset: forget any old queue
            status(legacy::ACKNOWLEDGE);
            status(legacy::ACKNOWLEDGE | legacy::DRIVER);
            // No features: there are none this driver needs.
            write(registers, mmio::HOST_FEATURES_SELECT, 0);
            write(registers, mmio::GUEST_FEATURES_SELECT, 0);
            write(registers, mmio::GUEST_FEATURES, 0);

            // What it is.
            let mut keys = [0u8; vinput::ANSWER_MAX];
            let mut relative = [0u8; vinput::ANSWER_MAX];
            let mut absolute = [0u8; vinput::ANSWER_MAX];
            ask(registers, vinput::SELECT_EVENT_BITS, vinput::EV_KEY, &mut keys);
            ask(registers, vinput::SELECT_EVENT_BITS, vinput::EV_REL, &mut relative);
            ask(registers, vinput::SELECT_EVENT_BITS, vinput::EV_ABS, &mut absolute);
            let kind = vinput::kind(&keys, &relative, &absolute);
            let range = |axis: u8| {
                let mut answer = [0u8; vinput::ANSWER_MAX];
                let size = ask(registers, vinput::SELECT_ABS_INFO, axis, &mut answer);
                vinput::abs_range(&answer[..size]).unwrap_or((0, 0))
            };
            let (x_range, y_range) = (range(0), range(1));
            let mut name = [0u8; vinput::ANSWER_MAX];
            let length = ask(registers, vinput::SELECT_NAME, 0, &mut name);
            // Some devices count the zero that ends the name; it is not
            // part of it.
            let name = core::str::from_utf8(&name[..length]).unwrap_or("?").trim_end_matches('\0');

            // The queue: as many entries as the device takes and the
            // memory holds, and a buffer offered for each, up to the
            // number there is room for.
            write(registers, mmio::GUEST_PAGE_SIZE, 4096);
            write(registers, mmio::QUEUE_SELECT, u32::from(vinput::QUEUE_EVENTS));
            let size = mmio::queue_size(read(registers, mmio::QUEUE_SIZE_MAX), QUEUE_AREA)
                .filter(|&size| usize::from(size) <= DESCRIPTORS_MAX)
                .ok_or("no queue, or none that fits")?;
            // SAFETY: the first QUEUE_AREA bytes of the DMA memory, which
            // is page-aligned and this driver's alone.
            let queue = Queue::new(memory, Layout::legacy(size));
            write(registers, mmio::QUEUE_SIZE, u32::from(size));
            write(registers, mmio::QUEUE_ALIGN, 4096);
            write(registers, mmio::QUEUE_PFN, (info.dma_physical / 4096) as u32);
            let mut device = Device {
                registers,
                memory,
                physical: info.dma_physical,
                queue,
                interrupt,
                carried: [0; DESCRIPTORS_MAX],
                kind,
                keys: Decoder::new(),
                pointer: Events::new(x_range, y_range),
            };
            for n in 0..BUFFERS_MAX.min(usize::from(size)) {
                device.offer(n);
            }
            status(legacy::ACKNOWLEDGE | legacy::DRIVER | legacy::DRIVER_OK);
            device.notify();
            let what = match kind {
                Kind::Keyboard => "keyboard",
                Kind::Mouse => "mouse",
                Kind::Tablet => "tablet",
                Kind::Other => "device of no kind this driver uses",
            };
            println!(
                "[hid] virtio {what} at {:#x}: {name:?}, {} buffers, {}",
                info.registers,
                BUFFERS_MAX.min(usize::from(size)),
                if interrupt.is_some() { "interrupts" } else { "polled" }
            );
            Ok(device)
        }
    }

    /// Buffer `n` offered to the device, empty, for its next event.
    fn offer(&mut self, n: usize) {
        let buffer = Buffer { physical: self.physical + (EVENTS_AT + n * EVENT_SIZE) as u64, length: EVENT_SIZE as u32, device_writes: true };
        // The queue has a descriptor for each buffer: never full.
        if let Some(descriptor) = self.queue.submit(&[buffer]) {
            self.carried[usize::from(descriptor) % DESCRIPTORS_MAX] = n as u8;
        }
    }

    fn notify(&self) {
        // SAFETY: the device's notify register.
        unsafe { write(self.registers, mmio::QUEUE_NOTIFY, u32::from(vinput::QUEUE_EVENTS)) };
    }

    /// After an interrupt: tells the device it was seen, before the
    /// queue is looked at - an event that comes in between then
    /// interrupts again instead of waiting unseen.
    fn interrupt_seen(&self) {
        // SAFETY: the two interrupt registers.
        unsafe {
            let why = read(self.registers, mmio::INTERRUPT_STATUS);
            write(self.registers, mmio::INTERRUPT_ACK, why);
        }
    }

    /// Every event the device has written since the last look: passed
    /// on, and its buffer offered again.
    fn take_events(&mut self, keys_out: Handle, pointer_out: Handle) {
        let mut any = false;
        while let Some((descriptor, _)) = self.queue.take_used() {
            any = true;
            let n = usize::from(self.carried[usize::from(descriptor) % DESCRIPTORS_MAX]) % BUFFERS_MAX;
            let mut bytes = [0u8; EVENT_SIZE];
            // SAFETY: buffer n, inside the DMA memory; the device has
            // handed it back and will not write it until offered again.
            unsafe { self.memory.add(EVENTS_AT + n * EVENT_SIZE).copy_to_nonoverlapping(bytes.as_mut_ptr(), EVENT_SIZE) };
            let (kind, code, value) = vinput::event(&bytes);
            self.event(kind, code, value, keys_out, pointer_out);
            self.offer(n);
        }
        if any {
            self.notify();
        }
    }

    fn event(&mut self, kind: u16, code: u16, value: u32, keys_out: Handle, pointer_out: Handle) {
        use input::pointer::codes::{BTN_MISC, EV_KEY};
        if self.kind == Kind::Keyboard && kind == EV_KEY && code < BTN_MISC {
            // 1 down, 2 held (repeating), 0 up.
            let Some((bytes, length)) = evdev::scancode(code, value != 0) else { return };
            for &byte in &bytes[..length] {
                let Some(event) = self.keys.feed(byte) else { continue };
                if let Some(text) = terminal_bytes(event, self.keys.control_held()) {
                    // A reader that lags loses keys, not the driver.
                    let _ = taaj::send(keys_out, text.as_bytes(), &[]);
                }
            }
            return;
        }
        if matches!(self.kind, Kind::Mouse | Kind::Tablet) {
            let message = match self.pointer.feed(kind, code, value) {
                Some(Report::Relative { dx, dy, buttons }) => Pointer { absolute: false, x: dx, y: dy, buttons },
                Some(Report::Absolute { x, y, buttons }) => Pointer { absolute: true, x, y, buttons },
                None => return,
            };
            let _ = taaj::send(pointer_out, &message.to_message(), &[]);
        }
    }
}

fn main() -> i64 {
    let (Some(arrivals), Some(keys_out), Some(pointer_out)) = (taaj::startup_handle(0), taaj::startup_handle(1), taaj::startup_handle(2)) else {
        println!("[hid] started without its channels");
        return 1;
    };
    let mut pings = taaj::startup_handle(3);

    // The devices: every message already waiting. init sends them all
    // before starting this driver, so none comes later.
    let mut devices: [Option<Device>; DEVICES_MAX] = [const { None }; DEVICES_MAX];
    let mut count = 0;
    let mut message = [0u8; abi::MESSAGE_MAX as usize];
    while let Ok(Some(_)) = taaj::poll(&[arrivals]) {
        let mut handles = [0u64; abi::MESSAGE_HANDLES_MAX as usize];
        let Ok((n, given)) = taaj::receive(arrivals, &mut message, &mut handles) else { break };
        let started = match (MmioDeviceInfo::from_bytes(&message[..n]), given) {
            (Some(info), 2 | 3) if count < DEVICES_MAX => {
                Device::start(&info, handles[0], handles[1], (given == 3).then_some(handles[2])).map_err(|why| (info.registers, why))
            }
            _ => Err((0, "not a device description, or one too many")),
        };
        match started {
            Ok(device) => {
                devices[count] = Some(device);
                count += 1;
            }
            Err((at, why)) => {
                println!("[hid] the device at {at:#x} would not start: {why}");
                for &handle in &handles[..given] {
                    let _ = taaj::close(handle);
                }
            }
        }
    }
    if count == 0 {
        println!("[hid] no input devices to drive");
        return 2;
    }

    loop {
        for device in devices.iter_mut().flatten() {
            device.take_events(keys_out, pointer_out);
        }
        // What to wait for: every device's interrupt, and init's pings.
        // A device without an interrupt is looked at every tick.
        let mut waiting = [0u64; DEVICES_MAX + 1];
        let mut length = 0;
        let mut polled = false;
        for device in devices.iter().flatten() {
            match device.interrupt {
                Some(line) => {
                    waiting[length] = line;
                    length += 1;
                }
                None => polled = true,
            }
        }
        let ping_at = length;
        if let Some(channel) = pings {
            waiting[length] = channel;
            length += 1;
        }
        let ready = match (length, polled) {
            (0, _) => {
                taaj::sleep(1);
                continue;
            }
            (_, true) => taaj::object_wait_until(&waiting[..length], taaj::ticks() + 1),
            (_, false) => taaj::object_wait(&waiting[..length]).map(Some),
        };
        match ready {
            Ok(Some(n)) if n == ping_at && pings.is_some() => {
                if let Some(channel) = pings {
                    if !taaj::answer_pings(channel) {
                        pings = None;
                    }
                }
            }
            Ok(Some(n)) => {
                // The n-th interrupt: taken, so the next one can come;
                // answered, so the device lets go of the line. Its queue
                // is looked at at the top.
                let line = waiting[n];
                if let Some(device) = devices.iter().flatten().find(|device| device.interrupt == Some(line)) {
                    if taaj::interrupt_wait(line).is_err() {
                        return 3;
                    }
                    device.interrupt_seen();
                }
            }
            Ok(None) => {}
            Err(Error::PeerClosed) => return 0,
            Err(_) => return 3,
        }
    }
}
