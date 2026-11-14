//! Processes: an address space, and (for now) one thread running in it.
//!
//! The lower half of the address space is the process's own; the upper
//! half is the kernel, shared by every process but inaccessible from
//! ring 3 (no USER bit). A process's frames are marked OWNED, so tearing
//! down its page tables returns every one of them.

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicI64, AtomicU64, Ordering};

use abi::Error;
use kernel64::elf::{self, ElfError};
use kernel64::memory::addr::{phys_to_virt, PhysAddr, VirtAddr, PAGE_SIZE};
use kernel64::paging::{Flags, PageSize, PageTables};
use kernel64::sched::ThreadId;

use crate::arch::syscall;
use crate::irqlock::IrqLock;
use crate::{mm, threads};

/// The user stack: 64 KiB just below this address. The page below it is
/// left unmapped, so overflowing the stack faults.
const USER_STACK_TOP: u64 = 0x0000_7FFF_FFFF_0000;
const USER_STACK_PAGES: u64 = 16;

const RUNNING: i64 = i64::MIN;

pub struct Process {
    pub pid: u64,
    pub name: &'static str,
    /// Physical address of the PML4 - what CR3 holds while this runs.
    root: u64,
    tables: IrqLock<Option<PageTables>>,
    exit_code: AtomicI64,
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
}

impl Drop for Process {
    fn drop(&mut self) {
        if let Some(tables) = self.tables.lock().take() {
            mm::with_frames(|frames| tables.destroy(frames));
        }
    }
}

static NEXT_PID: AtomicU64 = AtomicU64::new(1);

/// Loads the ELF executable `file` into a new address space and starts
/// it on a new thread.
pub fn spawn(name: &'static str, file: &[u8]) -> Result<(Arc<Process>, ThreadId), LoadError> {
    let image = elf::parse(file, abi::USER_END).map_err(LoadError::Elf)?;

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
            tables.map(VirtAddr::new(virt), frame, PageSize::Size4K, flags, frames).map_err(|_| LoadError::OutOfMemory)
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
        name,
        root: tables.root().as_u64(),
        tables: IrqLock::new(Some(tables)),
        exit_code: AtomicI64::new(RUNNING),
    });
    let entry = image.entry;
    // RSP as if `_start` had just been called: 16-byte aligned, minus the
    // return address a call would have pushed.
    let stack = USER_STACK_TOP - 8;
    let thread = threads::spawn_in(Some(Arc::clone(&process)), name, move || {
        // SAFETY: this thread runs in `process`'s address space (the
        // switch loaded its CR3 and kernel stack), which maps both.
        unsafe { syscall::enter_user(entry, stack) }
    });
    Ok((process, thread))
}

/// The process the running thread belongs to.
pub fn current() -> Option<Arc<Process>> {
    threads::current_process()
}

/// Ends the current process with `code`.
pub fn exit_current(code: i64) -> ! {
    if let Some(process) = current() {
        process.exit_code.store(code, Ordering::Release);
        kprintln!("[process] {} {:?} exited with code {code}", process.pid, process.name);
    }
    threads::exit()
}

/// Ends the current process for breaking the rules.
pub fn kill_current(reason: &str) -> ! {
    if let Some(process) = current() {
        process.exit_code.store(abi::KILLED, Ordering::Release);
        kprintln!("[process] {} {:?} killed: {reason}", process.pid, process.name);
    }
    threads::exit()
}

/// Copies `length` bytes from the current process's memory at `pointer`.
///
/// The kernel never dereferences a user pointer directly. Every page of
/// the range is looked up in the process's own page tables first, and
/// must be mapped with the USER bit - so a pointer into the kernel half,
/// into unmapped memory, or wrapping around the address space is an
/// error, not a fault and not a leak.
pub fn copy_from_user(pointer: u64, length: u64) -> Result<Vec<u8>, Error> {
    let end = pointer.checked_add(length).ok_or(Error::BadAddress)?;
    if end > abi::USER_END {
        return Err(Error::BadAddress);
    }
    let process = current().ok_or(Error::BadAddress)?;
    let tables = process.tables.lock();
    let tables = tables.as_ref().ok_or(Error::BadAddress)?;

    let mut bytes = Vec::with_capacity(length as usize);
    let mut address = pointer;
    while address < end {
        let translation = tables.translate(VirtAddr::new(address)).ok_or(Error::BadAddress)?;
        if !translation.flags.contains(Flags::USER) {
            return Err(Error::BadAddress);
        }
        let chunk = (PAGE_SIZE - address % PAGE_SIZE).min(end - address);
        let source = phys_to_virt(PhysAddr::new(translation.physical.as_u64())).as_ptr::<u8>();
        // SAFETY: the page is mapped (checked), and read through the
        // direct map, so it stays valid whatever the process does.
        bytes.extend_from_slice(unsafe { core::slice::from_raw_parts(source, chunk as usize) });
        address += chunk;
    }
    Ok(bytes)
}
