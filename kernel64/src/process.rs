//! Processes: an address space, and (for now) one thread running in it.
//!
//! The lower half of the address space is the process's own; the upper
//! half is the kernel, shared by every process but inaccessible from
//! ring 3 (no USER bit). A process's frames are marked OWNED, so tearing
//! down its page tables returns every one of them.
//!
//! A process also has a handle table (cap.rs): the only way it can reach
//! any kernel object. When it ends - by exiting or being killed - every
//! handle is closed at once, so whoever was talking to it finds out.

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};

use abi::Error;
use kernel64::cap::{CapTable, Rights};
use kernel64::elf::{self, ElfError};
use kernel64::ioports::PortBitmap;
use kernel64::regions::Regions;
use kernel64::memory::addr::{phys_to_virt, PhysAddr, VirtAddr, PAGE_SIZE};
use kernel64::paging::{Flags, PageSize, PageTables};
use kernel64::sched::ThreadId;

use crate::arch::{mmu, syscall};
use crate::irqlock::IrqLock;
use crate::ipc::Object;
use crate::memory_object::MemoryObject;
use crate::{mm, threads};

/// The user stack: 64 KiB just below this address. The page below it is
/// left unmapped, so overflowing the stack faults.
const USER_STACK_TOP: u64 = 0x0000_7FFF_FFFF_0000;
const USER_STACK_PAGES: u64 = 16;

/// Where memory objects are mapped: far from the program (which sits low,
/// near 4 MiB) and the stack (at the very top).
const MAPPING_AREA: core::ops::Range<u64> = 0x0000_1000_0000_0000..0x0000_7000_0000_0000;

const RUNNING: i64 = i64::MIN;

pub struct Process {
    pub pid: u64,
    pub name: String,
    /// Physical address of the PML4 - what CR3 holds while this runs.
    root: u64,
    tables: IrqLock<Option<PageTables>>,
    exit_code: AtomicI64,
    /// Set by `kill`: the process ends when it next heads for user mode.
    kill_requested: AtomicBool,
    /// Its thread, once started (0 before): whom a kill wakes.
    thread: AtomicU64,
    pub handles: IrqLock<CapTable<Object>>,
    /// The I/O ports it may use directly; loaded into the TSS whenever
    /// it runs. Granted for the life of the process.
    pub io_ports: IrqLock<PortBitmap>,
    /// Mapped memory objects. Each region holds a reference to its
    /// object, so the frames outlive any handle while they are mapped.
    /// Taken before `tables`.
    mappings: IrqLock<Regions<Arc<MemoryObject>>>,
}

#[derive(Debug)]
pub enum LoadError {
    /// Read when the error is printed (Debug), which the compiler's
    /// dead-code check does not count.
    #[allow(dead_code)]
    Elf(ElfError),
    OutOfMemory,
}

impl Process {
    pub fn root(&self) -> u64 {
        self.root
    }

    /// The exit code, once the process has ended.
    pub fn exit_code(&self) -> Option<i64> {
        let code = self.exit_code.load(Ordering::Acquire);
        (code != RUNNING).then_some(code)
    }

    /// Whether someone has asked for this process to end.
    pub fn kill_requested(&self) -> bool {
        self.kill_requested.load(Ordering::Acquire)
    }
}

impl Process {
    /// Closes every handle - now, not when the last reference to the
    /// Process goes, which may be much later (a parent still holding it
    /// to read the exit code).
    fn close_all_handles(&self) {
        let objects = self.handles.lock().take_all();
        drop(objects); // outside the lock: closing channels wakes threads
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        // The tables go first: they free the process's own (OWNED) frames
        // and leave mapped objects' frames alone. The mappings - the
        // references keeping those objects alive - are dropped after,
        // with the rest of the fields.
        if let Some(tables) = self.tables.lock().take() {
            mm::with_frames(|frames| tables.destroy(frames));
        }
    }
}

/// Maps all of `object` into the current process; returns the address.
pub fn map_memory(object: &Arc<MemoryObject>, writable: bool) -> Result<u64, Error> {
    let process = current().ok_or(Error::InvalidArgument)?;
    let mut mappings = process.mappings.lock();
    let start = mappings.allocate(object.size(), Arc::clone(object)).ok_or(Error::OutOfMemory)?;

    // Never executable: shared memory is data. A page another process
    // can write must not be one this process can run (W^X across
    // processes).
    let mut flags = Flags::USER | mm::no_execute() | object.cache;
    if writable {
        flags |= Flags::WRITABLE;
    }
    let mut tables = process.tables.lock();
    let tables = tables.as_mut().ok_or(Error::InvalidArgument)?;
    let mapped = mm::with_frames(|frames| {
        for (n, &frame) in object.frames().iter().enumerate() {
            let page = VirtAddr::new(start + n as u64 * PAGE_SIZE);
            if tables.map(page, frame, PageSize::Size4K, flags, frames).is_err() {
                return Err(n); // out of frames for page tables
            }
        }
        Ok(())
    });
    if let Err(done) = mapped {
        for n in 0..done as u64 {
            tables.unmap(VirtAddr::new(start + n * PAGE_SIZE));
        }
        mappings.remove(start);
        return Err(Error::OutOfMemory);
    }
    Ok(start)
}

/// Removes the current process's mapping that starts at `start`.
pub fn unmap_memory(start: u64) -> Result<u64, Error> {
    let process = current().ok_or(Error::InvalidArgument)?;
    let mut mappings = process.mappings.lock();
    let (range, object) = mappings.remove(start).ok_or(Error::InvalidArgument)?;
    {
        let mut tables = process.tables.lock();
        let tables = tables.as_mut().ok_or(Error::InvalidArgument)?;
        for page in range.step_by(PAGE_SIZE as usize) {
            tables.unmap(VirtAddr::new(page));
            // The CPU may still hold the old translation in its TLB; until
            // it is flushed the process could keep using a page that is
            // about to be freed. (This process is the one running, so its
            // tables are the active ones.)
            mmu::flush(page);
        }
    }
    drop(mappings);
    drop(object); // possibly the last reference: frees the frames
    Ok(0)
}

static NEXT_PID: AtomicU64 = AtomicU64::new(1);

/// Loads and starts a program: `load`, then `start`.
pub fn spawn(
    name: &str,
    file: &[u8],
    handles: Vec<(Object, Rights)>,
) -> Result<(Arc<Process>, ThreadId), LoadError> {
    let process = load(name, file)?;
    let thread = start(&process, handles, 0);
    Ok((process.process, thread))
}

/// A loaded program, not yet started.
pub struct Loaded {
    process: Arc<Process>,
    entry: u64,
}

impl core::ops::Deref for Loaded {
    type Target = Arc<Process>;
    fn deref(&self) -> &Arc<Process> {
        &self.process
    }
}

/// Every process that still exists, for `process_list`. Weak: listing a
/// process must not keep it alive.
static ALL: IrqLock<Vec<alloc::sync::Weak<Process>>> = IrqLock::new(Vec::new());

/// The processes that exist (running, or ended but still referenced).
pub fn all() -> Vec<Arc<Process>> {
    let mut all = ALL.lock();
    all.retain(|p| p.strong_count() > 0);
    all.iter().filter_map(|p| p.upgrade()).collect()
}

/// Wakes everyone in `process_wait` whenever any process ends.
pub static EXITS: threads::WaitQueue = threads::WaitQueue::new();

/// Loads the ELF executable `file` into a new address space. Nothing
/// runs yet; this is the step that can fail.
///
/// `file` may be anything - a program from the disk is whatever someone
/// put there. `elf::parse` checks all of it; a program must also lie
/// below the mapping area, so no segment can sit where the kernel will
/// later place a memory object, or on the stack.
pub fn load(name: &str, file: &[u8]) -> Result<Loaded, LoadError> {
    let image = elf::parse(file, MAPPING_AREA.start).map_err(LoadError::Elf)?;

    // SAFETY: frames are reached through the direct map.
    let mut tables = mm::with_memory(|kernel, frames| unsafe {
        PageTables::new_user(frames, mm::DIRECT_MAP, kernel)
    })
    .ok_or(LoadError::OutOfMemory)?;

    let loaded = mm::with_frames(|frames| -> Result<(), LoadError> {
        let mut map_page = |virt: u64, flags: Flags, contents: &[(usize, &[u8])]| {
            let frame = frames.allocate().ok_or(LoadError::OutOfMemory)?;
            let page = phys_to_virt(frame).as_mut_ptr::<u8>();
            // SAFETY: a fresh frame, reached through the direct map.
            unsafe {
                page.write_bytes(0, PAGE_SIZE as usize);
                for (offset, bytes) in contents {
                    page.add(*offset).copy_from_nonoverlapping(bytes.as_ptr(), bytes.len());
                }
            }
            let flags = flags | Flags::USER | Flags::OWNED;
            tables.map(VirtAddr::new(virt), frame, PageSize::Size4K, flags, frames).map_err(|_| {
                // Not mapped, so `destroy` will not find it: free it here.
                frames.free(frame);
                LoadError::OutOfMemory
            })
        };

        for segment in &image.segments {
            let mut flags = Flags::NONE;
            if segment.writable {
                flags |= Flags::WRITABLE;
            }
            if !segment.executable {
                flags |= mm::no_execute();
            }
            // The file's bytes occupy virtual [start, start + file length).
            let start = segment.virtual_range.start;
            let data = &file[segment.file_range.clone()];
            let first = start & !(PAGE_SIZE - 1);
            for page in (first..segment.virtual_range.end).step_by(PAGE_SIZE as usize) {
                // The slice of `data` that lands in this page, and where.
                let from = page.max(start);
                let to = (page + PAGE_SIZE).min(start + data.len() as u64);
                let piece: &[u8] = if from < to { &data[(from - start) as usize..(to - start) as usize] } else { &[] };
                map_page(page, flags, &[((from - page) as usize, piece)])?;
            }
        }
        for n in 1..=USER_STACK_PAGES {
            map_page(USER_STACK_TOP - n * PAGE_SIZE, Flags::WRITABLE | mm::no_execute(), &[])?;
        }
        Ok(())
    });
    if let Err(error) = loaded {
        mm::with_frames(|frames| tables.destroy(frames));
        return Err(error);
    }

    let process = Arc::new(Process {
        pid: NEXT_PID.fetch_add(1, Ordering::Relaxed),
        name: String::from(name),
        root: tables.root().as_u64(),
        tables: IrqLock::new(Some(tables)),
        exit_code: AtomicI64::new(RUNNING),
        kill_requested: AtomicBool::new(false),
        thread: AtomicU64::new(0),
        handles: IrqLock::new(CapTable::new(abi::HANDLES_MAX)),
        io_ports: IrqLock::new(PortBitmap::NONE),
        mappings: IrqLock::new(Regions::new(MAPPING_AREA)),
    });
    ALL.lock().push(Arc::downgrade(&process));
    Ok(Loaded { process, entry: image.entry })
}

/// Starts a loaded program on a new thread, with `handles` as its startup
/// handles: numbered 1..=n in this order, n passed to `_start` in RDI,
/// and `argument` in RSI.
pub fn start(loaded: &Loaded, handles: Vec<(Object, Rights)>, argument: u64) -> ThreadId {
    let process = &loaded.process;
    let count = handles.len() as u64;
    {
        let mut table = process.handles.lock();
        for (n, (object, rights)) in handles.into_iter().enumerate() {
            let handle = table.insert(object, rights).expect("a fresh table has room");
            debug_assert_eq!(handle, n as u64 + 1, "startup handles are 1..=n");
        }
    }
    let entry = loaded.entry;
    // RSP as if `_start` had just been called: 16-byte aligned, minus the
    // return address a call would have pushed.
    let stack = USER_STACK_TOP - 8;
    let thread = threads::spawn_in(Some(Arc::clone(process)), "user", move || {
        // SAFETY: this thread runs in `process`'s address space (the
        // switch loaded its CR3 and kernel stack), which maps both.
        unsafe { syscall::enter_user(entry, stack, count, argument) }
    });
    process.thread.store(thread, Ordering::Release);
    thread
}

/// The process the running thread belongs to.
pub fn current() -> Option<Arc<Process>> {
    threads::current_process()
}

/// Ends the current process: records `code`, closes its handles, tells
/// whoever waits for it, and ends its thread.
fn end_current(code: i64, how: core::fmt::Arguments) -> ! {
    if let Some(process) = current() {
        process.exit_code.store(code, Ordering::Release);
        kprintln!("[process] {} {:?} {how}", process.pid, process.name);
        process.close_all_handles();
    }
    EXITS.wake_all();
    threads::exit()
}

/// Ends the current process with `code`.
pub fn exit_current(code: i64) -> ! {
    end_current(code, format_args!("exited with code {code}"))
}

/// Ends the current process for breaking the rules.
pub fn kill_current(reason: &str) -> ! {
    end_current(abi::KILLED, format_args!("killed: {reason}"))
}

/// Asks for `target` to be ended. It returns at once; the process ends
/// itself, soon.
///
/// A process cannot simply be taken apart from outside. Its thread may
/// be in the middle of a system call - holding a lock, halfway through
/// moving a handle - and a thread stopped there would leave the kernel's
/// own data half-changed. So a kill is a request: a flag, which the
/// thread checks at the two places where it is about to return to user
/// mode and so holds nothing of the kernel's - the end of a system
/// call, and the end of an interrupt that arrived in user mode
/// (`stop_if_killed`). There it ends as if it had called exit.
///
/// A thread that is waiting - for a message, an interrupt, a time -
/// would not get there, so the kill also wakes it: the wait returns
/// early (threads::wait_any), the call gives up, and the exit path
/// finds the flag.
///
/// How long it takes: a waiting thread ends the next time it runs; one
/// computing in user mode, at the next timer tick - 10 ms at most.
pub fn kill(target: &Arc<Process>) {
    if target.exit_code().is_some() {
        return; // already ended
    }
    // The flag first: a thread woken before it was set would only go
    // back to waiting.
    target.kill_requested.store(true, Ordering::Release);
    let thread = target.thread.load(Ordering::Acquire);
    if thread != 0 {
        threads::wake(thread); // if it is blocked or sleeping; else nothing
    }
}

/// Ends the current process if a kill was asked for. Called only where
/// the thread is about to return to user mode.
pub fn stop_if_killed() {
    if current().is_some_and(|process| process.kill_requested()) {
        end_current(abi::STOPPED, format_args!("stopped: killed by request"));
    }
}

/// The physical address behind each page of `pointer..pointer+length`
/// in the current process, checked: every page mapped for user mode, and
/// writable if `write`.
///
/// The kernel never dereferences a user pointer directly. Every page of
/// the range is looked up in the process's own page tables first, and
/// must be mapped with the USER bit - so a pointer into the kernel half,
/// into unmapped memory, or wrapping around the address space is an
/// error, not a fault and not a leak.
fn user_pages(pointer: u64, length: u64, write: bool) -> Result<Vec<(u64, usize)>, Error> {
    let end = pointer.checked_add(length).ok_or(Error::BadAddress)?;
    if end > abi::USER_END {
        return Err(Error::BadAddress);
    }
    let process = current().ok_or(Error::BadAddress)?;
    let tables = process.tables.lock();
    let tables = tables.as_ref().ok_or(Error::BadAddress)?;

    let mut needed = Flags::USER;
    if write {
        needed |= Flags::WRITABLE;
    }
    let mut pieces = Vec::new();
    let mut address = pointer;
    while address < end {
        let translation = tables.translate(VirtAddr::new(address)).ok_or(Error::BadAddress)?;
        if !translation.flags.contains(needed) {
            return Err(Error::BadAddress);
        }
        let chunk = (PAGE_SIZE - address % PAGE_SIZE).min(end - address);
        pieces.push((translation.physical.as_u64(), chunk as usize));
        address += chunk;
    }
    Ok(pieces)
}

/// Copies `length` bytes from the current process's memory at `pointer`.
pub fn copy_from_user(pointer: u64, length: u64) -> Result<Vec<u8>, Error> {
    let mut bytes = Vec::with_capacity(length as usize);
    for (physical, length) in user_pages(pointer, length, false)? {
        let source = phys_to_virt(PhysAddr::new(physical)).as_ptr::<u8>();
        // SAFETY: the page is mapped (checked), and read through the
        // direct map, so it stays valid whatever the process does.
        bytes.extend_from_slice(unsafe { core::slice::from_raw_parts(source, length) });
    }
    Ok(bytes)
}

/// Whether the current process may have `length` bytes at `pointer`
/// written - checked before a call commits to anything, so a bad buffer
/// fails the call rather than losing, say, a received message.
pub fn check_writable(pointer: u64, length: u64) -> Result<(), Error> {
    user_pages(pointer, length, true).map(|_| ())
}

/// Copies `bytes` into the current process's memory at `pointer`: all of
/// them, or (if any page is not writable user memory) none.
pub fn copy_to_user(pointer: u64, bytes: &[u8]) -> Result<(), Error> {
    let mut offset = 0;
    for (physical, length) in user_pages(pointer, bytes.len() as u64, true)? {
        let target = phys_to_virt(PhysAddr::new(physical)).as_mut_ptr::<u8>();
        // SAFETY: as above, and the page is user memory - never the
        // kernel's own - so writing it cannot corrupt the kernel.
        unsafe { target.copy_from_nonoverlapping(bytes[offset..].as_ptr(), length) };
        offset += length;
    }
    Ok(())
}
