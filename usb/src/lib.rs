//! USB, from the host's side: the xHCI host controller, finding out what
//! is plugged into it, and setting up keyboards and mice.
//!
//! USB is a tree with the host at its root. The **host controller** is a
//! PCI device; on every PC of the last ten years it speaks **xHCI** (the
//! "eXtensible Host Controller Interface"), which handles every speed of
//! USB - 1.5 Mbit/s keyboards to 10 Gbit/s disks - through one set of
//! registers and rings in memory. Its **root ports** are the sockets on
//! the machine.
//!
//! A device plugged into a port knows nothing at first: not its address
//! on the bus, not how it will be used. The host **enumerates** it:
//!
//!   1. resets the port (USB 2) - the device answers at address 0;
//!   2. gives it an address and a *slot* in the controller;
//!   3. asks for its descriptors: the device descriptor (who it is),
//!      then its configuration (what it can do: interfaces, each of a
//!      class - keyboard, mouse, disk - and their endpoints);
//!   4. chooses a configuration, sets up the endpoints it will use, and
//!      starts using them.
//!
//! Every device has **endpoint 0**, the control endpoint, for requests
//! like "give me your descriptor". Data flows through the other
//! endpoints: a keyboard has one *interrupt IN* endpoint, which the host
//! asks every few milliseconds "anything new?" and which answers with a
//! report when a key changes.
//!
//! The modules:
//!   - `xhci`: the controller - its registers, its rings, its commands,
//!     control transfers and reading from an endpoint;
//!   - `descriptor`: reading what a device says about itself;
//!   - `device`: enumeration, from a port to a configured keyboard or
//!     mouse (the HID "boot protocol", which every one of them speaks),
//!     or a USB stick;
//!   - `storage`: a stick's SCSI commands over the Bulk-Only Transport.
//!
//! All of it is written against `xhci::Bus` and tested on the host
//! against a simulated controller with simulated devices (`tests/`).
//! The program that drives a real controller is `user/src/bin/usb.rs`.

#![cfg_attr(not(test), no_std)]

pub mod descriptor;
pub mod device;
pub mod storage;
pub mod xhci;

#[cfg(test)]
mod tests;
