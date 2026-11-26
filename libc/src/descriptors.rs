//! File descriptors: open, read, write, lseek, close, unlink - and
//! rename and isatty, which take a path or a descriptor too. A
//! descriptor can also be a socket (sockets.rs); read, write and close
//! work on those here as well.
//!
//! A descriptor is a small number a C program holds in place of "the
//! thing I am reading or writing". 0, 1 and 2 are there from the
//! start: standard input, output and error. `open` hands out the rest.
//!
//! On Unix the kernel keeps the table those numbers index, and for an
//! open file it remembers where the program has got to. Here neither
//! the kernel nor the file service knows any of that. The file service
//! remembers nothing between requests: each one says the whole path
//! and the offset - "512 bytes of /docs/notes from 1024". So the table
//! is here, in the program's own memory, and an open file is no more
//! than
//!
//!     the path    as text, from the root
//!     an offset   where the next read or write starts
//!     the flags   it was opened with
//!
//! `read` asks the service for bytes of that path from that offset,
//! and moves the offset on. `lseek` only changes a number. `close`
//! forgets the three; the service is not even told.
//!
//! That has consequences a Unix program would not expect, because the
//! descriptor is tied to the *name* and not to the file: rename a file
//! while it is open and the descriptor reads whatever has that name
//! now, or fails; remove it and the next write makes a new one.

use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::ffi::{c_char, c_int, c_long, c_void, CStr};
use core::ptr;
use core::sync::atomic::Ordering;

use taaj::sync::{Lock, Mutex};

use taaj::files::{FileError, Files, Kind};
use taaj::Error;

use crate::errno::*;

// The flags of open, with the values Linux gives them (fcntl.h).
// Reading, writing or both is a number in the lowest two bits, not a
// bit each: 0 to read, 1 to write, 2 (O_RDWR) for both. 3 is nothing.
const O_RDONLY: c_int = 0;
const O_WRONLY: c_int = 1;
const O_ACCMODE: c_int = 3;
const O_CREAT: c_int = 0o100;
const O_EXCL: c_int = 0o200;
const O_TRUNC: c_int = 0o1000;
const O_APPEND: c_int = 0o2000;
pub(crate) const O_DIRECTORY: c_int = 0o200000;

const SEEK_SET: c_int = 0;
const SEEK_CUR: c_int = 1;
const SEEK_END: c_int = 2;

/// Most descriptors a program may have open at once, the three it
/// starts with among them.
const OPEN_MAX: usize = 32;

/// What a descriptor stands for. Cloning it is what `dup` does: the
/// copy shares the file, socket or pipe end - and so, between threads,
/// one thread may read a pipe while another writes to it (see `read`).
#[derive(Clone)]
pub(crate) enum Open {
    Input,
    Output,
    Error,
    /// Shared with the descriptors `dup` made of it: one offset, moved
    /// by a read through any of them - POSIX's "open file description".
    File(SharedFile),
    /// Shared with the descriptors `dup` made of it, unread bytes and
    /// all.
    Socket(Shared<crate::sockets::Socket>),
    /// One end of a stream channel: a pipe's (pipes.rs). Shared with
    /// the descriptors `dup` made of it.
    PipeRead(crate::pipes::ReadEnd),
    PipeWrite(crate::pipes::WriteEnd),
}

/// Something the descriptors that `dup` made share, with a lock of its
/// own: a thread uses it with only that lock held, not the table's.
pub(crate) type Shared<T> = Arc<Mutex<T>>;

pub(crate) fn shared<T>(value: T) -> Shared<T> {
    Arc::new(Mutex::new(value))
}

/// A file open, as its descriptors share it.
pub(crate) type SharedFile = Shared<File>;

/// A descriptor for a file newly open.
fn file(file: File) -> Open {
    Open::File(shared(file))
}

pub(crate) struct File {
    /// From the root, with the '/' in front: what it meant when it was
    /// opened, whatever directory that was said in.
    pub(crate) path: String,
    pub(crate) offset: u64,
    pub(crate) flags: c_int,
}

impl File {
    fn reads(&self) -> bool {
        self.flags & O_ACCMODE != O_WRONLY
    }

    pub(crate) fn writes(&self) -> bool {
        self.flags & O_ACCMODE != O_RDONLY
    }
}

pub(crate) struct State {
    /// The descriptors: the number is the index.
    pub(crate) table: Vec<Option<Open>>,
    /// Close-on-exec, one bit per descriptor (OPEN_MAX is 32): those a
    /// program this one starts does not get (spawn.rs). Every new
    /// descriptor starts without it.
    pub(crate) cloexec: u32,
    /// The file service: not asked for until a file is first opened,
    /// and then once (the outer Option) - a program may have been
    /// given none (the inner). Shared, so that a thread can read a file
    /// with the table let go.
    pub(crate) files: Option<Option<Arc<Files>>>,
    /// The network service, the same way: asked for at the first
    /// socket.
    pub(crate) net: Option<Option<Arc<taaj::net::Net>>>,
}

/// Standard input, behind its own lock: a thread waiting for a key
/// holds only this one.
static INPUT: Mutex<Option<taaj::Input>> = Mutex::new(None);

fn read_input(out: &mut [u8]) -> usize {
    INPUT.lock().get_or_insert_with(taaj::input).read(out)
}

/// Whether reading standard input would not wait - `(bytes, ended)` -
/// and the channel to wait on until it would. None while another thread
/// is reading it (and holds it): then only its channel is known.
pub(crate) fn input_ready() -> Option<((bool, bool), Option<taaj::Handle>)> {
    let mut input = INPUT.try_lock()?;
    let input = input.get_or_insert_with(taaj::input);
    Some((input.ready(), input.handle()))
}

/// What descriptor `fd` stands for now, if anything.
pub(crate) fn open_at(fd: c_int) -> Option<Open> {
    with(|state| usize::try_from(fd).ok().and_then(|fd| state.table.get(fd)).and_then(Option::clone))
}

impl State {
    /// The file service, asked for the first time it is needed; None if
    /// this program was not given one.
    pub(crate) fn files(&mut self) -> Option<&Arc<Files>> {
        self.files.get_or_insert_with(|| taaj::file_service().map(Arc::new)).as_ref()
    }

    /// What descriptor `fd` stands for, and the file service: a copy
    /// to use once the table is let go.
    fn entry(&mut self, fd: c_int) -> Option<(Open, Option<Arc<Files>>)> {
        let open = usize::try_from(fd).ok().and_then(|fd| self.table.get(fd)).and_then(Option::as_ref)?.clone();
        let files = match open {
            Open::File(_) => self.files().cloned(),
            _ => None,
        };
        Some((open, files))
    }

    fn new() -> State {
        let mut table = Vec::with_capacity(OPEN_MAX);
        table.extend([Some(Open::Input), Some(Open::Output), Some(Open::Error)]);
        let mut state = State { table, cloexec: 0, files: None, net: None };
        state.inherit();
        state
    }

    /// The descriptors beyond 0, 1 and 2 whoever started this program
    /// handed down (abi::startup::DESCRIPTORS, abi::descriptors): a
    /// file as its name, place and flags; a pipe's end, or another
    /// stream, as the channel itself. The parent filled the channel and
    /// closed its end before this program started, so reading stops at
    /// the last message instead of waiting.
    fn inherit(&mut self) {
        use abi::descriptors::Entry;
        let Some(channel) = taaj::descriptors() else { return };
        let mut message = [0u8; abi::MESSAGE_MAX as usize];
        let mut handles = [0u64; 1];
        while let Ok((length, count)) = taaj::receive(channel, &mut message, &mut handles) {
            let handle = (count == 1).then_some(handles[0]);
            let entry = Entry::decode(&message[..length]).filter(|entry| entry.has_handle() == handle.is_some());
            let slot = entry.as_ref().map(|entry| usize::from(entry.descriptor())).filter(|&slot| (3..OPEN_MAX).contains(&slot));
            let (Some(entry), Some(slot)) = (entry, slot) else {
                // Not one this library can use: dropped, its handle closed.
                if let Some(handle) = handle {
                    let _ = taaj::close(handle);
                }
                continue;
            };
            let open = match (entry, handle) {
                // read and write find the file service already asked
                // for, as open leaves it: asked for here, then. A
                // program given no file service cannot use the file.
                (Entry::File { .. }, _) if self.files().is_none() => continue,
                (Entry::File { flags, offset, path, .. }, _) => file(File { path: String::from(path), offset, flags }),
                (Entry::Read { .. }, Some(handle)) => Open::PipeRead(Arc::new(crate::pipes::Reader::new(handle))),
                (_, Some(handle)) => Open::PipeWrite(Arc::new(crate::pipes::Channel(handle))),
                (_, None) => continue, // ruled out above
            };
            while self.table.len() <= slot {
                self.table.push(None);
            }
            self.table[slot] = Some(open);
        }
        let _ = taaj::close(channel);
    }

    /// The lowest number not in use: what C promises `open` returns.
    /// Without close-on-exec, as every new descriptor starts.
    pub(crate) fn free_slot(&mut self) -> Option<usize> {
        let slot = match self.table.iter().position(Option::is_none) {
            Some(slot) => slot,
            None if self.table.len() < OPEN_MAX => {
                self.table.push(None);
                self.table.len() - 1
            }
            None => return None,
        };
        self.set_cloexec(slot, false);
        Some(slot)
    }

    /// Marks descriptor `slot` close-on-exec, or not.
    pub(crate) fn set_cloexec(&mut self, slot: usize, on: bool) {
        if slot < OPEN_MAX {
            if on {
                self.cloexec |= 1 << slot;
            } else {
                self.cloexec &= !(1 << slot);
            }
        }
    }

    pub(crate) fn is_cloexec(&self, slot: usize) -> bool {
        slot < OPEN_MAX && self.cloexec & (1 << slot) != 0
    }
}

/// The one State - the table of descriptors - behind a futex lock
/// (taaj::sync): one thread at a time changes it. Nothing that may wait
/// - for input, a pipe, a socket, the file service - is done with it
/// held: `read` and `write` take what the descriptor stands for out of
/// the table, let the table go, and wait holding only that thing's own
/// lock. Otherwise a thread waiting to read a pipe would keep the
/// thread that writes to it out of the table, and both would wait for
/// ever.
struct StateLock {
    locked: Lock,
    state: UnsafeCell<Option<State>>,
}

// SAFETY: `state` is reached only while `locked` is held.
unsafe impl Sync for StateLock {}

static STATE: StateLock = StateLock { locked: Lock::new(), state: UnsafeCell::new(None) };

pub(crate) fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    STATE.locked.lock();
    // SAFETY: the lock is held, so this is the only reference.
    let state = unsafe { &mut *STATE.state.get() };
    let result = f(state.get_or_insert_with(State::new));
    STATE.locked.unlock();
    result
}

/// Leaves the reason in errno and gives the -1 that says "look there".
pub(crate) fn fail<T: From<i8>>(code: c_int) -> T {
    errno.store(code, Ordering::Relaxed);
    T::from(-1)
}

/// The file service's reasons, as errno numbers.
pub(crate) fn reason(error: FileError) -> c_int {
    use taaj::files::Status as No;
    match error {
        FileError::Refused(No::NotFound) => ENOENT,
        FileError::Refused(No::BadName | No::IntoItself) => EINVAL,
        FileError::Refused(No::TooBig) => EFBIG,
        FileError::Refused(No::Full) => ENOSPC,
        FileError::Refused(No::ReadOnly) => EROFS,
        FileError::Refused(No::NotDirectory) => ENOTDIR,
        FileError::Refused(No::IsDirectory) => EISDIR,
        FileError::Refused(No::NotEmpty) => ENOTEMPTY,
        FileError::Refused(No::Exists) => EEXIST,
        FileError::Refused(No::Busy) => ENFILE,
        FileError::Refused(No::BadRequest | No::DiskError | No::Ok) => EIO,
        FileError::Unreachable(Error::TimedOut) => ETIMEDOUT,
        FileError::Unreachable(_) => EIO,
    }
}

/// A path from C: text ending in a zero. EINVAL if it is not UTF-8,
/// which no name on this system is; ENAMETOOLONG, as POSIX says, if a
/// name in it is longer than NAME_MAX (limits.h) or the whole is
/// PATH_MAX or longer.
///
/// # Safety
/// `path` must be null or point to a string that ends in a zero byte.
pub(crate) unsafe fn text<'a>(path: *const c_char) -> Result<&'a str, c_int> {
    if path.is_null() {
        return Err(EFAULT);
    }
    // SAFETY: not null, and the caller's promise.
    let path = unsafe { CStr::from_ptr(path) }.to_str().map_err(|_| EINVAL)?;
    if path.len() >= PATH_MAX || path.split('/').any(|name| name.len() > NAME_MAX) {
        return Err(ENAMETOOLONG);
    }
    Ok(path)
}

/// limits.h's NAME_MAX: the longest name in a path. The file service's
/// limit, which TaajFS (since version 5) and its memory store share.
pub const NAME_MAX: usize = taaj::files::COMPONENT_MAX;

/// `open(path, flags)`: what the C function `open` calls (libc/c/fcntl.c
/// - it takes `...`, for a third argument nothing here uses).
///
/// # Safety
/// `path` must be null or point to a string that ends in a zero byte.
#[no_mangle]
pub unsafe extern "C" fn __taaj_open(path: *const c_char, flags: c_int) -> c_int {
    // SAFETY: the caller's promise.
    let name = match unsafe { text(path) } {
        Ok(name) => name,
        Err(code) => return fail(code),
    };
    if flags & O_ACCMODE == O_ACCMODE {
        return fail(EINVAL);
    }
    with(|state| {
        let Some(slot) = state.free_slot() else { return fail(EMFILE) };
        let Some(files) = state.files() else {
            return fail(EACCES);
        };
        let file = File { path: format!("/{}", files.resolve(name)), offset: 0, flags };
        // To write, the channel must be one that can change files. Asked
        // now, so that the program hears at `open` - where it expects to
        // - and not at its first `write`.
        if file.writes() {
            match files.read_only() {
                Ok(false) => {}
                Ok(true) => return fail(EROFS),
                Err(error) => return fail(reason(error)),
            }
        }
        let made_ready = match files.stat(&file.path) {
            // A directory opens only when asked for as one, to read:
            // a descriptor for fchdir, fdopendir and the like (its read
            // gives EISDIR). Without O_DIRECTORY, EISDIR at once, as
            // TheTaaJ always said.
            Ok((Kind::Directory, _)) if flags & O_DIRECTORY != 0 && !file.writes() => Ok(()),
            Ok((Kind::Directory, _)) => return fail(EISDIR),
            Ok((Kind::File, _)) if flags & O_DIRECTORY != 0 => return fail(ENOTDIR),
            Ok((Kind::File, _)) if flags & (O_CREAT | O_EXCL) == O_CREAT | O_EXCL => return fail(EEXIST),
            // Writing nothing over a file leaves it empty.
            Ok((Kind::File, _)) if flags & O_TRUNC != 0 && file.writes() => files.write(&file.path, &[]).map(|_| ()),
            Ok((Kind::File, _)) => Ok(()),
            // Adding nothing to a file that is not there makes it.
            Err(FileError::Refused(taaj::files::Status::NotFound)) if flags & O_CREAT != 0 => {
                files.append(&file.path, &[]).map(|_| ())
            }
            Err(error) => Err(error),
        };
        if let Err(error) = made_ready {
            return fail(reason(error));
        }
        let cloexec = file.flags & O_CLOEXEC != 0;
        state.table[slot] = Some(self::file(File { flags: file.flags & !O_CLOEXEC, ..file }));
        state.set_cloexec(slot, cloexec);
        slot as c_int
    })
}

/// `close(fd)`: the number is free to be given out again. (A socket's
/// connection is closed with it: what was written is still sent. A
/// pipe's end is closed with its last descriptor.)
#[no_mangle]
pub extern "C" fn close(fd: c_int) -> c_int {
    with(|state| match usize::try_from(fd).ok().and_then(|fd| state.table.get_mut(fd)).and_then(Option::take) {
        Some(_) => {
            state.set_cloexec(fd as usize, false);
            0
        }
        None => fail(EBADF),
    })
}

/// `read(fd, buffer, count)`: up to `count` bytes, from where the
/// descriptor has got to. Returns how many - which may be fewer than
/// asked for, and is 0 only at the end - or -1.
///
/// # Safety
/// `buffer` must point to `count` writable bytes.
#[no_mangle]
pub unsafe extern "C" fn read(fd: c_int, buffer: *mut c_void, count: usize) -> isize {
    if buffer.is_null() && count != 0 {
        return fail(EFAULT);
    }
    let Some((open, files)) = with(|state| state.entry(fd)) else { return fail(EBADF) };
    if count == 0 {
        return 0;
    }
    // SAFETY: the caller's promise, and `buffer` is not null.
    let out = unsafe { core::slice::from_raw_parts_mut(buffer.cast::<u8>(), count) };
    // The table is let go: what is read now may wait (see STATE).
    match open {
        Open::Input => read_input(out) as isize,
        Open::Socket(socket) => socket.lock().read(out),
        Open::PipeRead(end) => end.read(out) as isize,
        Open::File(file) => {
            let mut file = file.lock();
            if !file.reads() {
                return fail(EBADF);
            }
            // A file can only be in the table if the service was there
            // to open it.
            let Some(files) = files else { return fail(EIO) };
            let mut taken = 0;
            let read = files.read_from(&file.path, file.offset, |piece| {
                taken = piece.len().min(out.len());
                out[..taken].copy_from_slice(&piece[..taken]);
            });
            match read {
                Ok(_) => {
                    file.offset += taken as u64;
                    taken as isize
                }
                Err(error) => fail(reason(error)),
            }
        }
        _ => fail(EBADF),
    }
}

/// `write(fd, buffer, count)`: writes `count` bytes where the
/// descriptor has got to - or at the end, if it was opened with
/// O_APPEND. Returns how many were written, or -1.
///
/// # Safety
/// `buffer` must point to `count` readable bytes, as C's `write` asks.
#[no_mangle]
pub unsafe extern "C" fn write(fd: c_int, buffer: *const c_void, count: usize) -> isize {
    if buffer.is_null() && count != 0 {
        return fail(EFAULT);
    }
    let Some((open, files)) = with(|state| state.entry(fd)) else { return fail(EBADF) };
    if count == 0 {
        return 0;
    }
    // SAFETY: the caller's promise: `count` readable bytes at `buffer`,
    // which is not null.
    let bytes = unsafe { core::slice::from_raw_parts(buffer.cast::<u8>(), count) };
    // The table is let go: a write may wait for room (see STATE).
    match open {
        Open::Output => taaj::write(bytes).map_or_else(|_| fail(EIO), |_| count as isize),
        Open::Error => taaj::write_error(bytes).map_or_else(|_| fail(EIO), |_| count as isize),
        Open::Socket(socket) => socket.lock().write(bytes),
        Open::PipeWrite(end) => crate::pipes::write(&end, bytes),
        Open::File(file) => {
            let mut file = file.lock();
            if !file.writes() {
                return fail(EBADF);
            }
            let Some(files) = files else { return fail(EIO) };
            let written = if file.flags & O_APPEND != 0 {
                // The service finds the end itself, at the moment it
                // writes: two programs appending to one file cannot
                // write over each other.
                files.append(&file.path, bytes).map(|size| file.offset = size)
            } else {
                files.write_from(&file.path, file.offset, bytes).map(|_| file.offset += count as u64)
            };
            match written {
                Ok(()) => count as isize,
                Err(error) => fail(reason(error)),
            }
        }
        _ => fail(EBADF),
    }
}

/// `pread(fd, buffer, count, offset)`: as read, from `offset` in the
/// file, without moving where the next read or write starts. ESPIPE
/// for a descriptor that is not a file, EINVAL for a negative offset.
///
/// # Safety
/// `buffer` must be null with `count` 0, or valid for `count` bytes.
#[no_mangle]
pub unsafe extern "C" fn pread(fd: c_int, buffer: *mut c_void, count: usize, offset: i64) -> isize {
    if buffer.is_null() && count != 0 {
        return fail(EFAULT);
    }
    let Some((open, files)) = with(|state| state.entry(fd)) else { return fail(EBADF) };
    let Open::File(file) = open else { return fail(ESPIPE) };
    let Ok(offset) = u64::try_from(offset) else { return fail(EINVAL) };
    if count == 0 {
        return 0;
    }
    // SAFETY: the caller's promise, and `buffer` is not null.
    let out = unsafe { core::slice::from_raw_parts_mut(buffer.cast::<u8>(), count) };
    let file = file.lock();
    if !file.reads() {
        return fail(EBADF);
    }
    let Some(files) = files else { return fail(EIO) };
    let mut taken = 0;
    let read = files.read_from(&file.path, offset, |piece| {
        taken = piece.len().min(out.len());
        out[..taken].copy_from_slice(&piece[..taken]);
    });
    match read {
        Ok(_) => taken as isize,
        Err(error) => fail(reason(error)),
    }
}

/// `pwrite(fd, buffer, count, offset)`: as write, at `offset` in the
/// file, without moving where the next read or write starts - even
/// with O_APPEND, as POSIX says (Linux appends there instead). ESPIPE
/// for a descriptor that is not a file, EINVAL for a negative offset.
///
/// # Safety
/// `buffer` must be null with `count` 0, or valid for `count` bytes.
#[no_mangle]
pub unsafe extern "C" fn pwrite(fd: c_int, buffer: *const c_void, count: usize, offset: i64) -> isize {
    if buffer.is_null() && count != 0 {
        return fail(EFAULT);
    }
    let Some((open, files)) = with(|state| state.entry(fd)) else { return fail(EBADF) };
    let Open::File(file) = open else { return fail(ESPIPE) };
    let Ok(offset) = u64::try_from(offset) else { return fail(EINVAL) };
    if count == 0 {
        return 0;
    }
    // SAFETY: the caller's promise: `count` readable bytes at `buffer`.
    let bytes = unsafe { core::slice::from_raw_parts(buffer.cast::<u8>(), count) };
    let file = file.lock();
    if !file.writes() {
        return fail(EBADF);
    }
    let Some(files) = files else { return fail(EIO) };
    match files.write_from(&file.path, offset, bytes) {
        Ok(_) => count as isize,
        Err(error) => fail(reason(error)),
    }
}

/// `lseek(fd, offset, whence)`: moves where the next read or write
/// starts - to `offset` from the start (SEEK_SET), from where it is
/// (SEEK_CUR) or from the end (SEEK_END). Returns the new place, from
/// the start, or -1. Past the end is allowed: a write there leaves
/// zeros in between.
#[no_mangle]
pub extern "C" fn lseek(fd: c_int, offset: i64, whence: c_int) -> i64 {
    let (file, files) = match with(|state| state.entry(fd)) {
        Some((Open::File(file), files)) => (file, files),
        Some(_) => return fail(ESPIPE),
        None => return fail(EBADF),
    };
    {
        let mut file = file.lock();
        let from = match whence {
            SEEK_SET => 0,
            SEEK_CUR => file.offset,
            SEEK_END => {
                let Some(files) = files else { return fail(EIO) };
                match files.size(&file.path) {
                    Ok(size) => size,
                    Err(error) => return fail(reason(error)),
                }
            }
            _ => return fail(EINVAL),
        };
        // Before the start is nowhere; and the place must fit the
        // signed number it is returned as.
        match from.checked_add_signed(offset).filter(|&place| i64::try_from(place).is_ok()) {
            Some(place) => {
                file.offset = place;
                place as i64
            }
            None => fail(EINVAL),
        }
    }
}

/// Copies `text` and a zero byte into `buffer` (`size` bytes), or - if
/// `buffer` is null - into memory from malloc, which the caller frees.
/// Null, with errno ERANGE, if it does not fit.
///
/// # Safety
/// `buffer` must be null or valid for writing `size` bytes.
unsafe fn give(text: &str, buffer: *mut c_char, size: usize) -> *mut c_char {
    let (buffer, size) = if buffer.is_null() {
        let memory = crate::memory::malloc(text.len() + 1).cast::<c_char>();
        if memory.is_null() {
            return ptr::null_mut(); // malloc said why
        }
        (memory, text.len() + 1)
    } else {
        (buffer, size)
    };
    if text.len() + 1 > size {
        errno.store(ERANGE, Ordering::Relaxed);
        return ptr::null_mut();
    }
    // SAFETY: `size` bytes are writable, and text and its zero fit.
    unsafe {
        ptr::copy_nonoverlapping(text.as_ptr().cast::<c_char>(), buffer, text.len());
        buffer.add(text.len()).write(0);
    }
    buffer
}

/// `getcwd(buffer, size)`: the directory this program was started in,
/// as a path from the root ("/", "/docs") - names that do not start
/// with '/' are taken from there. TheTaaJ has no `chdir` for a program
/// yet, so it never changes. With a null `buffer`, the memory comes
/// from malloc (as glibc does it).
///
/// # Safety
/// `buffer` must be null or valid for writing `size` bytes.
#[no_mangle]
pub unsafe extern "C" fn getcwd(buffer: *mut c_char, size: usize) -> *mut c_char {
    let directory = with(|state| match state.files() {
        Some(files) => format!("/{}", files.directory()),
        // Not given the files: names mean nothing, and "/" is as true
        // as anything.
        None => String::from("/"),
    });
    // SAFETY: the caller's promise.
    unsafe { give(&directory, buffer, size) }
}

/// `realpath(path, resolved)`: the whole path from the root that `path`
/// names - ".", ".." and the starting directory worked out - if
/// something is there. TheTaaJ's files have no symbolic links, so that
/// is all there is to it. With a null `resolved`, the memory comes from
/// malloc; otherwise `resolved` must have room for PATH_MAX bytes.
///
/// # Safety
/// `path` must be null or point to a string that ends in a zero byte;
/// `resolved` null or valid for writing PATH_MAX bytes.
#[no_mangle]
pub unsafe extern "C" fn realpath(path: *const c_char, resolved: *mut c_char) -> *mut c_char {
    // SAFETY: the caller's promise.
    let name = match unsafe { text(path) } {
        Ok(name) => name,
        Err(code) => return fail_null(code),
    };
    let whole = with(|state| {
        let files = state.files().ok_or(EACCES)?;
        files.stat(name).map_err(reason)?;
        Ok(format!("/{}", files.resolve(name)))
    });
    match whole {
        // SAFETY: the caller's promise.
        Ok(whole) => unsafe { give(&whole, resolved, PATH_MAX) },
        Err(code) => fail_null(code),
    }
}

/// Sets errno and returns null: failing, for functions that return a
/// pointer.
fn fail_null(code: c_int) -> *mut c_char {
    errno.store(code, Ordering::Relaxed);
    ptr::null_mut()
}

/// The room a path needs, its zero byte included: limits.h's PATH_MAX
/// (libc/include/limits.h), which must say the same.
const PATH_MAX: usize = 4096;

/// `unlink(path)`: removes a file. (A directory is `rmdir`'s to remove.)
///
/// # Safety
/// `path` must be null or point to a string that ends in a zero byte.
#[no_mangle]
pub unsafe extern "C" fn unlink(path: *const c_char) -> c_int {
    // SAFETY: the caller's promise.
    let name = match unsafe { text(path) } {
        Ok(name) => name,
        Err(code) => return fail(code),
    };
    with(|state| {
        let Some(files) = state.files() else {
            return fail(EACCES);
        };
        match files.stat(name) {
            Ok((Kind::Directory, _)) => return fail(EISDIR),
            Ok((Kind::File, _)) => {}
            Err(error) => return fail(reason(error)),
        }
        match files.remove(name) {
            Ok(()) => 0,
            Err(error) => fail(reason(error)),
        }
    })
}

/// `rename(from, to)`: gives a file or a directory a new name, a new
/// directory, or both. As on Unix, what is already called `to` is
/// replaced, in the same step (TaajFS version 6): a file by a file, an
/// empty directory by a directory. A directory with things in it is
/// ENOTEMPTY; a directory onto a file ENOTDIR, a file onto a directory
/// EISDIR.
///
/// # Safety
/// Both must be null or point to strings that end in a zero byte.
#[no_mangle]
pub unsafe extern "C" fn rename(from: *const c_char, to: *const c_char) -> c_int {
    // SAFETY: the caller's promise, for each.
    let (from, to) = match unsafe { (text(from), text(to)) } {
        (Ok(from), Ok(to)) => (from, to),
        (Err(code), _) | (_, Err(code)) => return fail(code),
    };
    with(|state| {
        let Some(files) = state.files() else {
            return fail(EACCES);
        };
        match files.rename(from, to) {
            Ok(()) => 0,
            Err(error) => fail(reason(error)),
        }
    })
}

/// `isatty(fd)`: 1 if the descriptor is the screen - somebody is
/// reading it as it comes - and 0, with ENOTTY, if it is a file or
/// another program. (The name is Unix's: a "tty" was a teletype.)
///
/// The library asks this once, about standard output, to decide how
/// long text may wait in a buffer (libc/c/stdio.c). Standard error is
/// always the screen here. Standard input is the keyboard when the
/// program was started with neither a file nor another program to
/// read from.
#[no_mangle]
pub extern "C" fn isatty(fd: c_int) -> c_int {
    // 0 for "it is", or why it is not.
    let no = with(|state| match usize::try_from(fd).ok().and_then(|fd| state.table.get(fd)).and_then(Option::as_ref) {
        Some(Open::Output) if taaj::output_is_screen() => 0,
        Some(Open::Input) if taaj::input_is_keyboard() => 0,
        Some(Open::Error) => 0,
        Some(_) => ENOTTY,
        None => EBADF,
    });
    if no == 0 {
        return 1;
    }
    // "No" is 0 here, not -1: the question has an answer either way,
    // and errno says which kind of no it was.
    errno.store(no, Ordering::Relaxed);
    0
}

/// A second entry for what `fd` is open on, sharing it: a file's place
/// in it, a socket's unread bytes, a pipe's end.
fn copy_of(state: &State, fd: c_int) -> Result<Open, c_int> {
    usize::try_from(fd).ok().and_then(|fd| state.table.get(fd)).and_then(Option::as_ref).cloned().ok_or(EBADF)
}

/// `dup(fd)`: a second descriptor for the same thing, the lowest number
/// free.
#[no_mangle]
pub extern "C" fn dup(fd: c_int) -> c_int {
    with(|state| {
        let copy = match copy_of(state, fd) {
            Ok(copy) => copy,
            Err(code) => return fail(code),
        };
        let Some(slot) = state.free_slot() else { return fail(EMFILE) };
        state.table[slot] = Some(copy);
        slot as c_int
    })
}

/// `dup2(fd, to)`: makes `to` a second descriptor for what `fd` is open
/// on, closing whatever `to` was first. dup2(fd, fd) changes nothing.
#[no_mangle]
pub extern "C" fn dup2(fd: c_int, to: c_int) -> c_int {
    with(|state| {
        let copy = match copy_of(state, fd) {
            Ok(copy) => copy,
            Err(code) => return fail(code),
        };
        let Some(slot) = usize::try_from(to).ok().filter(|&slot| slot < OPEN_MAX) else { return fail(EBADF) };
        if fd == to {
            return to;
        }
        while state.table.len() <= slot {
            state.table.push(None);
        }
        state.table[slot] = Some(copy);
        // A copy made by dup2 is handed on, whatever the first is.
        state.set_cloexec(slot, false);
        to
    })
}

/// `fsync(fd)`: every write was committed by the file service before it
/// returned - and a commit flushes the disk's cache, before and after
/// its superblock (TaajFS's Volume::commit) - so there is nothing left
/// to write out: it is on the medium already. 0 for an open descriptor;
/// EBADF otherwise.
#[no_mangle]
pub extern "C" fn fsync(fd: c_int) -> c_int {
    with(|state| match usize::try_from(fd).ok().and_then(|fd| state.table.get(fd)).and_then(Option::as_ref) {
        Some(_) => 0,
        None => fail(EBADF),
    })
}

// fcntl's commands and flags (fcntl.h).
const F_DUPFD: c_int = 0;
const F_GETFD: c_int = 1;
const F_SETFD: c_int = 2;
const F_GETFL: c_int = 3;
const F_SETFL: c_int = 4;
const F_GETLK: c_int = 5;
const F_SETLK: c_int = 6;
const F_SETLKW: c_int = 7;
const F_DUPFD_CLOEXEC: c_int = 1030;
const FD_CLOEXEC: c_long = 1;
const O_NONBLOCK: c_int = 0o4000;
const O_CLOEXEC: c_int = 0o2000000;

/// What the C function `fcntl` (libc/c/fcntl.c) calls, its argument
/// already taken out: fcntl.h says what each command does here.
#[no_mangle]
pub extern "C" fn __taaj_fcntl(fd: c_int, command: c_int, argument: c_long) -> c_int {
    if command == F_DUPFD || command == F_DUPFD_CLOEXEC {
        return with(|state| {
            let copy = match copy_of(state, fd) {
                Ok(copy) => copy,
                Err(code) => return fail(code),
            };
            let from = match usize::try_from(argument) {
                Ok(from) if from < OPEN_MAX => from,
                _ => return fail(EINVAL),
            };
            // A number past the end of the table is free too.
            let free = |slot: usize| state.table.get(slot).is_none_or(Option::is_none);
            let Some(slot) = (from..OPEN_MAX).find(|&slot| free(slot)) else { return fail(EMFILE) };
            while state.table.len() <= slot {
                state.table.push(None);
            }
            state.table[slot] = Some(copy);
            state.set_cloexec(slot, command == F_DUPFD_CLOEXEC);
            slot as c_int
        });
    }
    with(|state| {
        let slot = fd as usize;
        let cloexec = state.is_cloexec(slot);
        let open = match usize::try_from(fd).ok().and_then(|fd| state.table.get_mut(fd)).and_then(Option::as_mut) {
            Some(open) => open,
            None => return fail(EBADF),
        };
        match (command, open) {
            // Close-on-exec: whether a program this one starts gets the
            // descriptor (spawn.rs).
            (F_GETFD, _) => c_int::from(cloexec),
            (F_SETFD, _) => {
                state.set_cloexec(slot, argument & FD_CLOEXEC != 0);
                0
            }
            (F_GETFL, Open::File(file)) => file.lock().flags,
            (F_GETFL, Open::Input) => O_RDONLY,
            (F_GETFL, Open::Output | Open::Error) => O_WRONLY,
            (F_GETFL, Open::Socket(_)) => 2, // O_RDWR
            (F_SETFL, Open::File(file)) => {
                let mut file = file.lock();
                let changeable = O_APPEND | O_NONBLOCK;
                file.flags = (file.flags & !changeable) | (argument as c_int & changeable);
                0
            }
            (F_SETFL, _) => 0,
            (F_GETLK | F_SETLK | F_SETLKW, _) => fail(ENOLCK),
            _ => fail(EINVAL),
        }
    })
}
