//! Windows, with no machine in them.
//!
//! - `protocol`: what a program and the window server say to each other,
//!   as bytes - opening a window, damage, keys, clicks, focus, closing.
//! - `desktop`: the server's bookkeeping - where each window is, which
//!   is on top, which has the keys, what a press, a drag or a click on
//!   a close box means.
//! - `toolkit`: the colours, the window frame, and a label and a button
//!   for programs to put in their windows.
//! - `text`: text being edited - lines, a cursor, the view onto them,
//!   and the keys that change them - for the editor.
//!
//! The window server (`display`) and the programs that open windows
//! (through `taaj::window`) do the system calls; everything they decide
//! is decided here, and tested on the host.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod desktop;
pub mod protocol;
pub mod text;
pub mod toolkit;
