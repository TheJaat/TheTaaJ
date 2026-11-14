//! TaajFS, in Rust - the host tool's library.
//!
//! The format and the volume are the `taajfs` crate, shared with the
//! file service inside TheTaaJ, so the tool and the OS read and write
//! disks with the same code. This adds what only the host needs:
//!
//!   device  - an image file as a block device
//!   dump    - an independent reader that reports what is on disk
//!
//! `format` and `volume` are re-exported, so the tool's code and tests
//! name them as before.

pub mod device;
pub mod dump;

pub use taajfs::{format, volume};
