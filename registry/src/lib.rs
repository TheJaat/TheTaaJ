//! The service registry: who answers to which name, and who may ask.
//!
//! A program that wants the file service needs a channel to it. Until
//! now init handed every program its channels in a fixed order, and the
//! program knew "the fourth one is the files" - a number agreed between
//! two source files and checked by nobody. The registry replaces the
//! number with a name: a program is given one channel, to the registry,
//! and asks on it for "files".
//!
//! A registry of names could undo everything capabilities are for: if
//! any program could ask for any service by name, a name would be all
//! it took to reach the disk. So there is not one list of names. Each
//! client talks to the registry over its own channel, and each channel
//! has its own list - its *namespace* - of the names that exist for
//! that client. A name outside it is not refused; it is not there. The
//! channel is the capability, as everywhere else: what you can ask for
//! was decided by whoever gave you the channel.
//!
//! Two kinds of channel reach the registry:
//!
//!   - one *admin* channel, held by init: "this handle answers to this
//!     name" (REGISTER) and "here is a new client channel, and the
//!     names it may ask for" (NAMESPACE);
//!   - any number of client channels, one per namespace: "give me the
//!     service called this" (LOOKUP), "what can I ask for?" (LIST),
//!     "make me a namespace with these of my names in it" (NARROW).
//!
//! A client can also ask for a channel of its own making: a *narrower*
//! namespace (NARROW), to hand to a program it starts. Every name in
//! it must be one the client can find itself - so nothing is gained by
//! asking - but it may be fewer, and they may be called something
//! else: the shell gives `ls` a namespace with the one name "files",
//! which means what the shell knows as "files.look". `ls` asks for
//! "files", like every program that reads files, and gets the channel
//! that cannot change anything. What a program is allowed is decided by
//! what its names mean, not by which names it knows to ask for.
//!
//! The registry holds a copy of every service's handle, and answers a
//! lookup with a fresh copy. It keeps nothing that cannot be given
//! again: if it crashes, init starts another and tells it everything
//! once more, and clients keep the channels they have.
//!
//! This crate is the part with no system calls in it - the messages
//! (`protocol`) and the table (`Table`) - so it is tested on the host.
//! The program around it is `user/src/bin/registry.rs`.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod protocol;
mod table;

pub use table::{Miss, Table, NAMESPACES_MAX, NAMES_MAX, SERVICES_MAX};
