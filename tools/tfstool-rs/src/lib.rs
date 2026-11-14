//! TaajFS, in Rust.
//!
//! A library plus a small command-line front end (`main.rs`). Keeping
//! the filesystem in a library means the tests exercise exactly the code
//! the tool runs - and later, the same modules can move into a `no_std`
//! file server inside TheTaaJ itself.
//!
//!   format  - the on-disk structures, bytes <-> values; no I/O
//!   device  - where blocks come from: an image file, or memory
//!   volume  - mount, read, and copy-on-write changes
//!   dump    - an independent reader that reports what is on disk

pub mod device;
pub mod dump;
pub mod format;
pub mod volume;
