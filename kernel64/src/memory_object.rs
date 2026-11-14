//! Memory objects: physical pages as a kernel object.
//!
//! A memory object is a list of physical frames. A handle to one lets a
//! process map those frames into its address space - and two processes
//! mapping the same object see the same bytes, with no copying: what
//! one writes, the other reads. That is how large data moves between
//! processes (pixels, file contents, network packets), where copying it
//! through channel messages would cost too much.
//!
//! Lifetime: the frames are freed when the last reference goes - and a
//! mapping is a reference, not just a handle. Closing the handle of an
//! object that is still mapped leaves the mapping working; the pages go
//! once the mapping is gone too. So no process can ever be left with a
//! mapping of a page that was freed and reused for something else.
//!
//! Most objects own their frames. A device object instead names fixed
//! physical memory - the framebuffer - and frees nothing.

use alloc::vec::Vec;

use kernel64::memory::addr::{phys_to_virt, PhysAddr, PAGE_SIZE};
use kernel64::paging::Flags;

use crate::mm;

pub struct MemoryObject {
    frames: Vec<PhysAddr>,
    owned: bool,
    /// Extra page flags for every mapping - caching, for devices.
    pub cache: Flags,
}

impl MemoryObject {
    /// `pages` fresh, zeroed frames. None if memory runs out.
    pub fn new(pages: u64) -> Option<MemoryObject> {
        let mut object = MemoryObject { frames: Vec::new(), owned: true, cache: Flags::NONE };
        mm::with_frames(|frames| {
            for _ in 0..pages {
                let frame = frames.allocate()?;
                // SAFETY: a fresh frame, through the direct map. Zeroed:
                // whatever the last owner left there must not leak.
                unsafe { phys_to_virt(frame).as_mut_ptr::<u8>().write_bytes(0, PAGE_SIZE as usize) };
                object.frames.push(frame);
            }
            Some(())
        })?;
        Some(object)
    }

    /// Fixed physical memory - a device's - starting at `physical`.
    pub fn device(physical: u64, bytes: u64, cache: Flags) -> MemoryObject {
        let first = physical & !(PAGE_SIZE - 1);
        let end = (physical + bytes + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
        let frames = (first..end).step_by(PAGE_SIZE as usize).map(PhysAddr::new).collect();
        MemoryObject { frames, owned: false, cache }
    }

    pub fn frames(&self) -> &[PhysAddr] {
        &self.frames
    }

    pub fn size(&self) -> u64 {
        self.frames.len() as u64 * PAGE_SIZE
    }
}

impl Drop for MemoryObject {
    fn drop(&mut self) {
        if self.owned {
            mm::with_frames(|frames| {
                for &frame in &self.frames {
                    frames.free(frame);
                }
            });
        }
    }
}
