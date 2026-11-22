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
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::ffi::{c_char, c_int, c_void, CStr};
use core::hint;
use core::ptr;
use core::sync::atomic::{AtomicBool, Ordering};

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

const SEEK_SET: c_int = 0;
const SEEK_CUR: c_int = 1;
const SEEK_END: c_int = 2;

/// Most descriptors a program may have open at once, the three it
/// starts with among them.
const OPEN_MAX: usize = 32;

/// What a descriptor stands for.
pub(crate) enum Open {
    Input,
    Output,
    Error,
    File(File),
    /// (Boxed: a connection holds a message's worth of bytes that
    /// arrived and were not read yet.)
    Socket(alloc::boxed::Box<crate::sockets::Socket>),
}

pub(crate) struct File {
    /// From the root, with the '/' in front: what it meant when it was
    /// opened, whatever directory that was said in.
    path: String,
    offset: u64,
    flags: c_int,
}

impl File {
    fn reads(&self) -> bool {
        self.flags & O_ACCMODE != O_WRONLY
    }

    fn writes(&self) -> bool {
        self.flags & O_ACCMODE != O_RDONLY
    }
}

pub(crate) struct State {
    /// The descriptors: the number is the index.
    pub(crate) table: Vec<Option<Open>>,
    input: taaj::Input,
    /// The file service: not asked for until a file is first opened,
    /// and then once (the outer Option) - a program may have been
    /// given none (the inner).
    files: Option<Option<Files>>,
    /// The network service, the same way: asked for at the first
    /// socket.
    pub(crate) net: Option<Option<taaj::net::Net>>,
}

impl State {
    fn new() -> State {
        let mut table = Vec::with_capacity(OPEN_MAX);
        table.extend([Some(Open::Input), Some(Open::Output), Some(Open::Error)]);
        State { table, input: taaj::input(), files: None, net: None }
    }

    /// The lowest number not in use: what C promises `open` returns.
    pub(crate) fn free_slot(&mut self) -> Option<usize> {
        if let Some(slot) = self.table.iter().position(Option::is_none) {
            return Some(slot);
        }
        if self.table.len() < OPEN_MAX {
            self.table.push(None);
            return Some(self.table.len() - 1);
        }
        None
    }
}

/// The one State, behind a spin lock. A C program here has one thread,
/// so the lock is never waited for; it is what makes handing out a
/// `&mut State` from a `static` sound, and costs one atomic swap.
struct Lock {
    locked: AtomicBool,
    state: UnsafeCell<Option<State>>,
}

// SAFETY: `state` is reached only while `locked` is held.
unsafe impl Sync for Lock {}

static STATE: Lock = Lock { locked: AtomicBool::new(false), state: UnsafeCell::new(None) };

pub(crate) fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    while STATE.locked.swap(true, Ordering::Acquire) {
        hint::spin_loop();
    }
    // SAFETY: the lock is held, so this is the only reference.
    let state = unsafe { &mut *STATE.state.get() };
    let result = f(state.get_or_insert_with(State::new));
    STATE.locked.store(false, Ordering::Release);
    result
}

/// Leaves the reason in errno and gives the -1 that says "look there".
pub(crate) fn fail<T: From<i8>>(code: c_int) -> T {
    errno.store(code, Ordering::Relaxed);
    T::from(-1)
}

/// The file service's reasons, as errno numbers.
fn reason(error: FileError) -> c_int {
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
        FileError::Refused(No::BadRequest | No::DiskError | No::Ok) => EIO,
        FileError::Unreachable(Error::TimedOut) => ETIMEDOUT,
        FileError::Unreachable(_) => EIO,
    }
}

/// A path from C: text ending in a zero. None if it is not UTF-8,
/// which no name on this system is.
///
/// # Safety
/// `path` must be null or point to a string that ends in a zero byte.
unsafe fn text<'a>(path: *const c_char) -> Result<&'a str, c_int> {
    if path.is_null() {
        return Err(EFAULT);
    }
    // SAFETY: not null, and the caller's promise.
    unsafe { CStr::from_ptr(path) }.to_str().map_err(|_| EINVAL)
}

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
        let Some(files) = state.files.get_or_insert_with(taaj::file_service).as_ref() else {
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
            Ok((Kind::Directory, _)) => return fail(EISDIR),
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
        state.table[slot] = Some(Open::File(file));
        slot as c_int
    })
}

/// `close(fd)`: the number is free to be given out again. (A socket's
/// connection is closed with it: what was written is still sent.)
#[no_mangle]
pub extern "C" fn close(fd: c_int) -> c_int {
    with(|state| match usize::try_from(fd).ok().and_then(|fd| state.table.get_mut(fd)).and_then(Option::take) {
        Some(_) => 0,
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
    with(|state| {
        let State { table, input, files, .. } = state;
        let open = usize::try_from(fd).ok().and_then(|fd| table.get_mut(fd)).and_then(Option::as_mut);
        if count == 0 {
            return if open.is_some() { 0 } else { fail(EBADF) };
        }
        // SAFETY: the caller's promise, and `buffer` is not null.
        let out = unsafe { core::slice::from_raw_parts_mut(buffer.cast::<u8>(), count) };
        match open {
            Some(Open::Input) => input.read(out) as isize,
            Some(Open::Socket(socket)) => socket.read(out),
            Some(Open::File(file)) if file.reads() => {
                // A file can only be in the table if the service was
                // there to open it.
                let Some(Some(files)) = files else { return fail(EIO) };
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
    })
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
    with(|state| {
        let State { table, files, .. } = state;
        let open = usize::try_from(fd).ok().and_then(|fd| table.get_mut(fd)).and_then(Option::as_mut);
        if count == 0 {
            return if open.is_some() { 0 } else { fail(EBADF) };
        }
        // SAFETY: the caller's promise: `count` readable bytes at
        // `buffer`, which is not null.
        let bytes = unsafe { core::slice::from_raw_parts(buffer.cast::<u8>(), count) };
        match open {
            Some(Open::Output) => taaj::write(bytes).map_or_else(|_| fail(EIO), |_| count as isize),
            Some(Open::Error) => taaj::write_error(bytes).map_or_else(|_| fail(EIO), |_| count as isize),
            Some(Open::Socket(socket)) => socket.write(bytes),
            Some(Open::File(file)) if file.writes() => {
                let Some(Some(files)) = files else { return fail(EIO) };
                let written = if file.flags & O_APPEND != 0 {
                    // The service finds the end itself, at the moment
                    // it writes: two programs appending to one file
                    // cannot write over each other.
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
    })
}

/// `lseek(fd, offset, whence)`: moves where the next read or write
/// starts - to `offset` from the start (SEEK_SET), from where it is
/// (SEEK_CUR) or from the end (SEEK_END). Returns the new place, from
/// the start, or -1. Past the end is allowed: a write there leaves
/// zeros in between.
#[no_mangle]
pub extern "C" fn lseek(fd: c_int, offset: i64, whence: c_int) -> i64 {
    with(|state| {
        let State { table, files, .. } = state;
        let file = match usize::try_from(fd).ok().and_then(|fd| table.get_mut(fd)).and_then(Option::as_mut) {
            Some(Open::File(file)) => file,
            Some(_) => return fail(ESPIPE),
            None => return fail(EBADF),
        };
        let from = match whence {
            SEEK_SET => 0,
            SEEK_CUR => file.offset,
            SEEK_END => {
                let Some(Some(files)) = files else { return fail(EIO) };
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
    })
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
    let directory = with(|state| match state.files.get_or_insert_with(taaj::file_service).as_ref() {
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
        let files = state.files.get_or_insert_with(taaj::file_service).as_ref().ok_or(EACCES)?;
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
        let Some(files) = state.files.get_or_insert_with(taaj::file_service).as_ref() else {
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
/// directory, or both. Unlike Unix, it replaces nothing: if `to` is
/// already there, that is the failure (EEXIST) - the file service will
/// not make a file vanish as a side effect of moving another.
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
        let Some(files) = state.files.get_or_insert_with(taaj::file_service).as_ref() else {
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
