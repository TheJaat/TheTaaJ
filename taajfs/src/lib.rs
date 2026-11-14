//! TaajFS, in Rust: the format and the volume, with no operating system
//! underneath.
//!
//!   format  - the on-disk structures, bytes <-> values; no I/O
//!   device  - where blocks come from: the BlockDevice trait, and a disk
//!             in memory for tests (which can pretend to lose power)
//!   volume  - mount, read, and copy-on-write changes
//!
//! Only `core` and `alloc`, so one implementation serves two places:
//! the host tool (tools/tfstool-rs, which adds an image-file device, a
//! dumper and a command line) and the file service inside TheTaaJ
//! (which adds a device made of virtio disk requests). A format read and
//! written by the same code on both sides cannot drift.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod device;
pub mod format;
pub mod volume;

pub use device::{BlockDevice, DeviceError, MemDevice};
pub use volume::{format as format_volume, Error, FileInfo, Volume};
