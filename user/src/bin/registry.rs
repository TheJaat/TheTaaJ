//! registry: the service that knows where the services are.
//!
//! Startup handle, from init:
//!   1  the admin channel (receive only): init's registrations
//!
//! init tells it two kinds of thing (registry::protocol). "This handle
//! answers to this name" - and the registry keeps the handle. "Here is
//! a client's channel, and the names that exist on it" - and from then
//! on the registry answers that channel: a lookup of one of those names
//! gets a fresh copy of the handle, with the rights init said a client
//! is to have; a lookup of anything else gets "not found".
//!
//! A client may also ask for a narrower namespace, to give to a
//! program it starts: some of its own names, perhaps called something
//! else. The registry makes the channel, answers it like any other,
//! and forgets it when the last holder of its other end has gone.
//!
//! So the registry holds a handle to every service, which makes it as
//! trusted as init. It is kept small for that reason: the deciding is
//! in the `registry` crate, tested on the host, and what is here is
//! receiving, copying a handle, and sending.
//!
//! It keeps nothing of init's that cannot be told again. When it
//! ends, init starts another and repeats every registration; clients
//! keep their channels, and a lookup that was waiting is answered by
//! the new one. The narrowed namespaces are the exception: nobody can
//! repeat those, so a program that was running across the crash finds
//! its channel to the registry closed. What it had already asked for,
//! it keeps.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;
use core::fmt::Write;

use registry::protocol::{Admin, Request, BAD_REQUEST, NOT_FOUND, OK};
use registry::{Miss, Table};
use taaj::rights::TRANSFER;
use taaj::{println, Buffer, Handle};

taaj::program!(main);

const MESSAGE_MAX: usize = abi::MESSAGE_MAX as usize;

fn close_all(handles: &[Handle]) {
    for &handle in handles {
        let _ = taaj::close(handle);
    }
}

/// Takes in one message from init. Whatever handle is not kept - the
/// message was not understood, the table is full, a name was replaced -
/// is closed: a handle nobody uses must not be left open, or the
/// channel behind it never looks closed to its other end.
fn admin(table: &mut Table<Handle>, message: &[u8], handles: &[Handle]) {
    let (Some(request), &[handle]) = (Admin::decode(message), handles) else {
        println!("[registry] a message from init was not understood");
        return close_all(handles);
    };
    let kept = match request {
        Admin::Register { name, rights } => table.register(name, handle, rights),
        Admin::Namespace { label, names } => {
            let kept = table.add_namespace(label, &names, handle);
            if kept.is_ok() {
                let mut line = Buffer::<400>::new();
                for name in &names {
                    let _ = write!(line, " {name}");
                }
                println!("[registry] {label} may ask for:{}", core::str::from_utf8(line.as_bytes()).unwrap_or(" ?"));
            }
            kept
        }
    };
    match kept {
        Ok(None) => {}
        Ok(Some(replaced)) => close_all(&[replaced]),
        Err(refused) => {
            println!("[registry] no room, or a bad name: a registration was dropped");
            close_all(&[refused]);
        }
    }
}

/// Answers one request from the client whose namespace is at `index`.
/// `reply` is where the answer goes; it is closed by the caller.
fn serve(table: &mut Table<Handle>, index: usize, message: &[u8], reply: Handle) {
    match Request::decode(message) {
        Some(Request::Lookup(name)) => {
            let found = table.lookup(index, name).map_err(|miss| match miss {
                Miss::NotInNamespace => "it is not in its namespace",
                Miss::NotRegistered => "nothing is registered under that name",
            });
            // A copy for this client: the rights init said, and the
            // right to be sent - without which it could not leave here.
            let copy = found.and_then(|(&handle, rights)| taaj::duplicate(handle, rights | TRANSFER).map_err(|_| "it could not be copied"));
            match copy {
                Ok(copy) => {
                    if taaj::send(reply, &[OK], &[copy]).is_err() {
                        let _ = taaj::close(copy); // not sent: still ours
                    }
                }
                Err(why) => {
                    println!("[registry] {} asked for {name:?}: {why}", table.label(index));
                    let _ = taaj::send(reply, &[NOT_FOUND], &[]);
                }
            }
        }
        Some(Request::List) => {
            let mut answer = [0u8; MESSAGE_MAX];
            answer[0] = OK;
            // The names always fit: 16 of 33 bytes at most.
            let length = abi::arguments::encode(table.list(index).into_iter(), &mut answer[1..]).unwrap_or(0);
            let _ = taaj::send(reply, &answer[..=length], &[]);
        }
        Some(Request::Narrow { label, names }) => {
            // A new channel: one end kept here and answered like any
            // client's, the other sent to the asker - to pass on.
            let made = taaj::channel_create().ok().and_then(|(ours, theirs)| match table.narrow(index, label, &names, ours) {
                Ok(()) => Some(theirs),
                Err(ours) => {
                    close_all(&[ours, theirs]);
                    None
                }
            });
            let mut line = Buffer::<400>::new();
            for (new, old) in &names {
                let _ = if new == old { write!(line, " {new}") } else { write!(line, " {new}={old}") };
            }
            let line = core::str::from_utf8(line.as_bytes()).unwrap_or(" ?");
            match made {
                Some(theirs) => {
                    println!("[registry] {} gave {label}:{line}", table.label(index));
                    if taaj::send(reply, &[OK], &[theirs]).is_err() {
                        // Not sent: closing it leaves the new
                        // namespace with no client, and it is
                        // forgotten like any other whose client left.
                        let _ = taaj::close(theirs);
                    }
                }
                None => {
                    println!("[registry] {} could not give {label}:{line}", table.label(index));
                    let _ = taaj::send(reply, &[NOT_FOUND], &[]);
                }
            }
        }
        Some(Request::Crash) => {
            println!("[registry] asked to crash (a test)");
            taaj::exit(1);
        }
        None => {
            let _ = taaj::send(reply, &[BAD_REQUEST], &[]);
        }
    }
}

fn main() -> i64 {
    let Some(init) = taaj::startup_handle(0) else {
        println!("[registry] started without its channel from init");
        return 1;
    };
    println!("[registry] ready");
    let mut table: Table<Handle> = Table::new();
    let mut message = [0u8; MESSAGE_MAX];
    // Room for the most handles a message can carry: one that did not
    // fit would stay at the head of its queue for ever.
    let mut handles = [0u64; abi::MESSAGE_HANDLES_MAX as usize];
    loop {
        // What init has said comes first, all of it: a client must not
        // be answered from a table that is only half filled. init
        // sends the services before the namespaces, so by the time a
        // client's channel is known here, so is everything it may find.
        while let Ok(Some(_)) = taaj::poll(&[init]) {
            match taaj::receive(init, &mut message, &mut handles) {
                Ok((length, count)) => admin(&mut table, &message[..length], &handles[..count]),
                Err(_) => return 0, // init is gone: so is the system
            }
        }
        // `once` and `chain`: init's channel, then every client's.
        let watched: Vec<Handle> = core::iter::once(init).chain(table.channels().copied()).collect();
        let ready = match taaj::object_wait(&watched) {
            Ok(ready) => ready,
            Err(error) => {
                println!("[registry] cannot wait: {error:?}");
                return 2;
            }
        };
        // init's turn is the top of the loop.
        let Some(index) = ready.checked_sub(1) else { continue };
        match taaj::receive(watched[ready], &mut message, &mut handles) {
            // A request brings exactly one handle: where to answer.
            Ok((length, 1)) => {
                serve(&mut table, index, &message[..length], handles[0]);
                let _ = taaj::close(handles[0]);
            }
            Ok((length, count)) => {
                // None: nowhere to answer - though a crash needs none.
                if count == 0 && Request::decode(&message[..length]) == Some(Request::Crash) {
                    println!("[registry] asked to crash (a test)");
                    return 1;
                }
                close_all(&handles[..count]);
            }
            // Every sender has gone: the namespace has no client.
            Err(_) => {
                if let Some(channel) = table.remove_namespace(index) {
                    let _ = taaj::close(channel);
                }
            }
        }
    }
}
