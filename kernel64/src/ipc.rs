//! Inter-process communication: channels as kernel objects, and the
//! system calls on them.
//!
//! The rules (the channel's state, the handle table's checks) are in the
//! library - `kernel64::channel` and `kernel64::cap` - tested on the
//! host. This adds what only the kernel can: locks, waiting for a
//! message, user memory, and getting the lifetimes right.
//!
//! Lifetimes: a channel end lives as long as some handle - in a table or
//! in a message in flight - refers to it. When the last one goes, the
//! end closes, its peer is told, and the messages waiting in its inbox
//! are destroyed - with any handles they carry, which may close further
//! channels, and so on.
//!
//! Lock order, always: a process's handle table, then a channel's state,
//! then a process's page tables. Never the other way round.
//!
//! A known limit, shared with other capability kernels: channels can
//! keep each other alive in a cycle - end A in B's inbox, B in A's - once
//! their owners close every handle. Sending an end through its own
//! channel is refused (the one-step cycle); longer cycles leak until a
//! cycle collector exists.

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::ops::Range;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use abi::{rights, Error};
use kernel64::cap::{Handle, Rights};
use kernel64::channel::{peer, Channel, Message};

use crate::interrupts::InterruptLine;
use crate::memory_object::MemoryObject;
use crate::irqlock::IrqLock;
use crate::{arch, process};
use crate::threads::WaitQueue;

/// Something a handle can refer to.
#[derive(Clone)]
pub enum Object {
    Channel(Arc<ChannelEnd>),
    /// A device's interrupt line: lets its holder wait for it.
    Interrupt(&'static InterruptLine),
    /// A range of I/O ports its holder may enable for itself.
    IoPorts(Range<u16>),
    /// Physical pages that can be mapped, and shared.
    Memory(Arc<MemoryObject>),
    /// The authority to start programs. Only init has one.
    Launcher,
    /// A process: lets its holder wait for it to end. (A child cannot
    /// get a handle to its parent unless the parent sends one - and
    /// spawn hands these out without TRANSFER - so processes do not
    /// keep each other alive in cycles.)
    Process(Arc<process::Process>),
}

/// A handle in flight: the object and the rights it travels with.
type Capability = (Object, Rights);

struct Shared {
    state: IrqLock<Channel<Capability>>,
    /// readable[n]: threads waiting for a message at end n.
    readable: [WaitQueue; 2],
}

pub struct ChannelEnd {
    shared: Arc<Shared>,
    end: usize,
}

/// Channels in existence, for leak checks.
static LIVE: AtomicUsize = AtomicUsize::new(0);

pub fn live_channels() -> usize {
    LIVE.load(Ordering::Relaxed)
}

impl Drop for Shared {
    fn drop(&mut self) {
        LIVE.fetch_sub(1, Ordering::Relaxed);
    }
}

/// A new channel: its two ends.
pub fn channel() -> (Object, Object) {
    LIVE.fetch_add(1, Ordering::Relaxed);
    let shared = Arc::new(Shared {
        state: IrqLock::new(Channel::new(abi::CHANNEL_QUEUE_MAX)),
        readable: [WaitQueue::new(), WaitQueue::new()],
    });
    let end = |n| Object::Channel(Arc::new(ChannelEnd { shared: Arc::clone(&shared), end: n }));
    (end(0), end(1))
}

impl Object {
    fn channel(&self) -> Result<&Arc<ChannelEnd>, Error> {
        match self {
            Object::Channel(end) => Ok(end),
            _ => Err(Error::WrongType),
        }
    }
}

impl ChannelEnd {
    fn same_channel(&self, other: &Object) -> bool {
        other.channel().is_ok_and(|end| Arc::ptr_eq(&self.shared, &end.shared))
    }
}

impl Drop for ChannelEnd {
    fn drop(&mut self) {
        let unread = self.shared.state.lock().close(self.end);
        self.shared.readable[peer(self.end)].wake_all();
        bury(unread);
    }
}

/// Messages waiting to be destroyed.
static GRAVEYARD: IrqLock<Vec<Message<Capability>>> = IrqLock::new(Vec::new());
static DRAINING: AtomicBool = AtomicBool::new(false);

/// Destroys messages - iteratively, not recursively.
///
/// Destroying a message drops its handles; dropping the last handle to a
/// channel end destroys *its* inbox, and so on. Done by plain recursion,
/// a chain of channels nested in each other's inboxes - which a process
/// can build as deep as it likes while holding a single handle - would
/// recurse once per link and overflow the kernel stack. So the messages
/// go into a graveyard, and only the outermost call empties it: inner
/// drops just add to the pile. The stack stays a few frames deep however
/// long the chain.
fn bury(messages: impl IntoIterator<Item = Message<Capability>>) {
    GRAVEYARD.lock().extend(messages);
    loop {
        if DRAINING.swap(true, Ordering::Acquire) {
            return; // someone further out is draining; they will get these
        }
        loop {
            let next = GRAVEYARD.lock().pop();
            match next {
                Some(message) => drop(message), // may bury more
                None => break,
            }
        }
        DRAINING.store(false, Ordering::Release);
        // Something buried between the last pop and the store above saw
        // DRAINING still set and left it to us.
        if GRAVEYARD.lock().is_empty() {
            return;
        }
    }
}

/// The current process, which every call below needs.
fn me() -> Result<Arc<process::Process>, Error> {
    process::current().ok_or(Error::InvalidHandle)
}

fn handles_from_user(pointer: u64, count: u64) -> Result<Vec<Handle>, Error> {
    let bytes = process::copy_from_user(pointer, count * 8)?;
    Ok(bytes.as_chunks::<8>().0.iter().map(|b| u64::from_le_bytes(*b)).collect())
}

fn handles_to_bytes(handles: &[Handle]) -> Vec<u8> {
    handles.iter().flat_map(|h| h.to_le_bytes()).collect()
}

pub fn create(out: u64) -> Result<u64, Error> {
    let process = me()?;
    process::check_writable(out, 16)?;
    let handles = {
        let mut table = process.handles.lock();
        if table.room() < 2 {
            return Err(Error::TooManyHandles);
        }
        let (a, b) = channel();
        [table.insert(a, Rights::ALL)?, table.insert(b, Rights::ALL)?]
    };
    process::copy_to_user(out, &handles_to_bytes(&handles))?;
    Ok(0)
}

pub fn send(handle: Handle, data: u64, length: u64, handles: u64, count: u64) -> Result<u64, Error> {
    if length > abi::MESSAGE_MAX || count > abi::MESSAGE_HANDLES_MAX {
        return Err(Error::InvalidArgument);
    }
    // Copy everything in first: user memory is read once, so the process
    // cannot change it while the kernel is looking.
    let data = process::copy_from_user(data, length)?;
    let to_move = handles_from_user(handles, count)?;

    let process = me()?;
    let mut table = process.handles.lock();
    let end = Arc::clone(table.get(handle, Rights(rights::SEND))?.channel()?);
    let mut state = end.shared.state.lock();
    // Every check before anything moves: after this point nothing fails.
    state.can_send(end.end)?;
    let moved = table.remove_all(&to_move, Rights(rights::TRANSFER), |o| !end.same_channel(o))?;
    if state.send(end.end, Message { data, handles: moved }).is_err() {
        unreachable!("can_send said yes");
    }
    drop(state);
    drop(table);
    end.shared.readable[peer(end.end)].wake_all();
    Ok(0)
}

pub fn receive(handle: Handle, buffer: u64, capacity: u64, handles: u64, handle_capacity: u64) -> Result<u64, Error> {
    let process = me()?;
    let end = Arc::clone(process.handles.lock().get(handle, Rights(rights::RECEIVE))?.channel()?);
    loop {
        // Wait for a message (or a closed peer: peek's error).
        end.shared.readable[end.end].wait_while(|| matches!(end.shared.state.lock().peek(end.end), Ok(None)));

        let mut table = process.handles.lock();
        let mut state = end.shared.state.lock();
        // Someone else holding this end may have taken it meanwhile.
        let Some((length, count)) = state.peek(end.end)? else { continue };
        if length as u64 > capacity || count as u64 > handle_capacity {
            return Err(Error::BufferTooSmall);
        }
        process::check_writable(buffer, length as u64)?;
        process::check_writable(handles, count as u64 * 8)?;
        if table.room() < count {
            return Err(Error::TooManyHandles);
        }
        // Nothing can fail now: take the message, install its handles.
        let message = state.receive(end.end).expect("peeked");
        drop(state);
        let installed: Vec<Handle> = message
            .handles
            .into_iter()
            .map(|(object, rights)| table.insert(object, rights).expect("room checked"))
            .collect();
        drop(table);
        process::copy_to_user(buffer, &message.data)?;
        process::copy_to_user(handles, &handles_to_bytes(&installed))?;
        return Ok(length as u64 | (count as u64) << 32);
    }
}

pub fn close(handle: Handle) -> Result<u64, Error> {
    let process = me()?;
    let (object, _) = process.handles.lock().remove(handle)?;
    drop(object); // outside the table's lock: closing may wake and bury
    Ok(0)
}

pub fn duplicate(handle: Handle, rights: u64) -> Result<u64, Error> {
    let rights = u32::try_from(rights).ok().filter(|r| r & !abi::rights::ALL == 0).ok_or(Error::InvalidArgument)?;
    me()?.handles.lock().duplicate(handle, Rights(rights))
}

pub fn interrupt_wait(handle: Handle) -> Result<u64, Error> {
    let line = match process::current().ok_or(Error::InvalidHandle)?.handles.lock().get(handle, Rights(rights::RECEIVE))? {
        Object::Interrupt(line) => *line,
        _ => return Err(Error::WrongType),
    };
    Ok(line.wait())
}

pub fn ioports_enable(handle: Handle) -> Result<u64, Error> {
    let process = me()?;
    let ports = match process.handles.lock().get(handle, Rights(0))? {
        Object::IoPorts(ports) => ports.clone(),
        _ => return Err(Error::WrongType),
    };
    let mut map = process.io_ports.lock();
    if !map.allow(ports) {
        return Err(Error::InvalidArgument);
    }
    // This process is running: load the change now, not at the next switch.
    arch::set_io_bitmap(&map);
    Ok(0)
}

/// Starts a program, giving it startup handles taken from the caller.
pub fn spawn(launcher: Handle, name: u64, name_length: u64, pairs: u64, count: u64) -> Result<u64, Error> {
    if name_length > abi::PROGRAM_NAME_MAX || count > abi::SPAWN_HANDLES_MAX {
        return Err(Error::InvalidArgument);
    }
    let name = process::copy_from_user(name, name_length)?;
    let pairs: Vec<(Handle, Rights)> = handles_from_user(pairs, count * 2)?
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u32::try_from(pair[1]).map(|rights| (pair[0], Rights(rights))))
        .collect::<Result<_, _>>()
        .map_err(|_| Error::InvalidArgument)?;
    let process = me()?;

    // 1. Every check, before anything changes.
    {
        let table = process.handles.lock();
        if !matches!(table.get(launcher, Rights(0))?, Object::Launcher) {
            return Err(Error::WrongType);
        }
        for &(handle, wanted) in &pairs {
            let held = table.rights(handle)?;
            if !held.contains(Rights(rights::TRANSFER)) || !held.contains(wanted) {
                return Err(Error::AccessDenied);
            }
        }
        if table.room() + pairs.len() < 1 {
            return Err(Error::TooManyHandles);
        }
    }
    let (program, file) =
        core::str::from_utf8(&name).ok().and_then(crate::programs::find).ok_or(Error::NotFound)?;

    // 2. Loading can still fail - out of memory - so it comes before the
    //    handles move. The table is not locked meanwhile: only this
    //    process's own thread changes it, and it is here.
    let loaded = process::load(program, file).map_err(|_| Error::OutOfMemory)?;

    // 3. Nothing fails from here on.
    let mut table = process.handles.lock();
    let handles: Vec<Handle> = pairs.iter().map(|&(h, _)| h).collect();
    let moved = table.remove_all(&handles, Rights(rights::TRANSFER), |_| true)?;
    let given = moved.into_iter().zip(&pairs).map(|((object, _), &(_, wanted))| (object, wanted)).collect();
    process::start(&loaded, given);
    table.insert(Object::Process(Arc::clone(&loaded)), Rights(0))
}

/// Waits for any of several processes to end.
pub fn wait(handles: u64, count: u64, code: u64) -> Result<u64, Error> {
    if count == 0 || count > abi::SPAWN_HANDLES_MAX {
        return Err(Error::InvalidArgument);
    }
    let handles = handles_from_user(handles, count)?;
    process::check_writable(code, 8)?;
    let process = me()?;
    let watched: Vec<Arc<process::Process>> = {
        let table = process.handles.lock();
        handles
            .iter()
            .map(|&h| match table.get(h, Rights(0))? {
                Object::Process(p) => Ok(Arc::clone(p)),
                _ => Err(Error::WrongType),
            })
            .collect::<Result<_, _>>()?
    };
    process::EXITS.wait_while(|| watched.iter().all(|p| p.exit_code().is_none()));
    let (index, exit_code) = watched
        .iter()
        .enumerate()
        .find_map(|(n, p)| p.exit_code().map(|c| (n, c)))
        .expect("one has ended");
    process::copy_to_user(code, &exit_code.to_le_bytes())?;
    Ok(index as u64)
}

/// Sends a message from the kernel itself, on a channel end it holds.
pub fn kernel_send(end: &Object, data: &[u8], handles: Vec<Capability>) -> Result<(), Error> {
    let end = end.channel()?;
    end.shared.state.lock().send(end.end, Message { data: data.to_vec(), handles }).map_err(|(error, rejected)| {
        bury([rejected]);
        error
    })?;
    end.shared.readable[peer(end.end)].wake_all();
    Ok(())
}

pub fn memory_create(size: u64) -> Result<u64, Error> {
    if size == 0 || size > abi::MEMORY_MAX {
        return Err(Error::InvalidArgument);
    }
    let process = me()?;
    if process.handles.lock().room() == 0 {
        return Err(Error::TooManyHandles);
    }
    let object = MemoryObject::new(size.div_ceil(abi::PAGE_SIZE)).ok_or(Error::OutOfMemory)?;
    let handle = process.handles.lock().insert(Object::Memory(Arc::new(object)), Rights::ALL);
    handle
}

pub fn memory_map(handle: Handle, writable: u64) -> Result<u64, Error> {
    let writable = match writable {
        0 => false,
        1 => true,
        _ => return Err(Error::InvalidArgument),
    };
    let needed = if writable { rights::READ | rights::WRITE } else { rights::READ };
    let object = match me()?.handles.lock().get(handle, Rights(needed))? {
        Object::Memory(object) => Arc::clone(object),
        _ => return Err(Error::WrongType),
    };
    process::map_memory(&object, writable)
}
