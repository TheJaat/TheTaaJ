//! What files are, and directories: stat, fstat, lstat; mkdir, rmdir,
//! access; opendir, readdir, closedir, rewinddir; utimensat, futimens,
//! utime, utimes, which mark a file changed. And the calls about
//! permissions, which TheTaaJ's files do not have (chmod, fchmod,
//! umask): sys/stat.h says what each field of `struct stat` can and
//! cannot say here.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::ffi::{c_char, c_int, c_long, c_uint};
use core::ptr;

use taaj::files::Kind;

use crate::descriptors::{fail, reason, text, with, Open};
use crate::errno::*;

// The kinds and permissions of st_mode (sys/stat.h).
const S_IFREG: c_uint = 0o100000;
const S_IFDIR: c_uint = 0o040000;
const S_IFCHR: c_uint = 0o020000;
const S_IFIFO: c_uint = 0o010000;

/// sys/stat.h's `struct timespec` fields: seconds and nanoseconds.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Time {
    seconds: c_long,
    nanoseconds: c_long,
}

/// sys/stat.h's `struct stat`, field for field.
#[repr(C)]
#[derive(Default)]
pub struct Stat {
    st_dev: u64,
    st_ino: u64,
    st_nlink: u64,
    st_mode: c_uint,
    st_uid: c_uint,
    st_gid: c_uint,
    st_rdev: u64,
    st_size: i64,
    st_blksize: i64,
    st_blocks: i64,
    st_atim: Time,
    st_mtim: Time,
    st_ctim: Time,
}

/// A number for the file at `path` (from the root): 64-bit FNV-1a of
/// the path. A path is a file here - there are no hard links - so the
/// same path always gives the same number, and different files
/// different numbers unless two hashes collide. Never 0, which some
/// programs take to mean "no file".
pub fn inode(path: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in path.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash.max(1)
}

/// The time a file's change stamp is told as: the stamp in seconds,
/// and never less than 1 - `make` takes 0 to mean there is no file. A
/// file not changed since stamping began (stamp 0) is 1, older than any
/// stamped one: a commit's stamp is at least 2 (sys/stat.h).
fn time_of(stamp: u64) -> Time {
    Time { seconds: c_long::try_from(stamp.max(1)).unwrap_or(c_long::MAX), nanoseconds: 0 }
}

fn describe(path: &str, kind: Kind, size: u64, stamp: u64) -> Stat {
    let size = i64::try_from(size).unwrap_or(i64::MAX);
    Stat {
        st_atim: time_of(stamp),
        st_mtim: time_of(stamp),
        st_ctim: time_of(stamp),
        st_dev: 1,
        st_ino: inode(path),
        st_nlink: 1,
        st_mode: match kind {
            Kind::File => S_IFREG | 0o644,
            Kind::Directory => S_IFDIR | 0o755,
        },
        st_size: size,
        st_blksize: 4096,
        st_blocks: (size + 511) / 512,
        ..Stat::default()
    }
}

/// `stat(path, out)`: what is at `path`. 0, or -1 with errno.
///
/// # Safety
/// `path` must be null or a string ending in a zero byte; `out` valid
/// for writing.
#[no_mangle]
pub unsafe extern "C" fn stat(path: *const c_char, out: *mut Stat) -> c_int {
    // SAFETY: the caller's promise.
    let name = match unsafe { text(path) } {
        Ok(name) => name,
        Err(code) => return fail(code),
    };
    let found = with(|state| {
        let files = state.files().ok_or(EACCES)?;
        let (kind, size, stamp) = files.stat_stamped(name).map_err(reason)?;
        Ok(describe(&format!("/{}", files.resolve(name)), kind, size, stamp))
    });
    match found {
        Ok(status) => {
            // SAFETY: the caller's promise.
            unsafe { out.write(status) };
            0
        }
        Err(code) => fail(code),
    }
}

/// `lstat(path, out)`: stat; there are no symbolic links to tell apart.
///
/// # Safety
/// As for `stat`.
#[no_mangle]
pub unsafe extern "C" fn lstat(path: *const c_char, out: *mut Stat) -> c_int {
    // SAFETY: the caller's promise, passed on.
    unsafe { stat(path, out) }
}

/// `fstat(fd, out)`: what an open descriptor is. A file is asked about
/// by its path; the three standard ones are character devices, and a
/// pipe's end is a FIFO.
///
/// # Safety
/// `out` must be valid for writing.
#[no_mangle]
pub unsafe extern "C" fn fstat(fd: c_int, out: *mut Stat) -> c_int {
    let found = with(|state| {
        let path = match usize::try_from(fd).ok().and_then(|fd| state.table.get(fd)).and_then(Option::as_ref) {
            Some(Open::File(file)) => file.lock().path.clone(),
            Some(Open::PipeRead(_) | Open::PipeWrite(_)) => {
                return Ok(Stat { st_dev: 1, st_ino: 1 + fd as u64, st_nlink: 1, st_mode: S_IFIFO | 0o600, st_blksize: 4096, ..Stat::default() })
            }
            Some(Open::Input | Open::Output | Open::Error | Open::Socket(_)) => {
                return Ok(Stat {
                    st_dev: 1,
                    st_ino: 1 + fd as u64,
                    st_nlink: 1,
                    st_mode: S_IFCHR | 0o620,
                    st_blksize: 4096,
                    ..Stat::default()
                })
            }
            None => return Err(EBADF),
        };
        let files = state.files().ok_or(EBADF)?;
        let (kind, size, stamp) = files.stat_stamped(&path).map_err(reason)?;
        Ok(describe(&path, kind, size, stamp))
    });
    match found {
        Ok(status) => {
            // SAFETY: the caller's promise.
            unsafe { out.write(status) };
            0
        }
        Err(code) => fail(code),
    }
}

/// `mkdir(path, mode)`: a new, empty directory. The mode is not kept.
///
/// # Safety
/// `path` must be null or a string ending in a zero byte.
#[no_mangle]
pub unsafe extern "C" fn mkdir(path: *const c_char, _mode: c_uint) -> c_int {
    // SAFETY: the caller's promise.
    let name = match unsafe { text(path) } {
        Ok(name) => name,
        Err(code) => return fail(code),
    };
    with(|state| {
        let Some(files) = state.files() else { return fail(EACCES) };
        match files.make_directory(name) {
            Ok(()) => 0,
            Err(error) => fail(reason(error)),
        }
    })
}

/// `rmdir(path)`: removes an empty directory.
///
/// # Safety
/// `path` must be null or a string ending in a zero byte.
#[no_mangle]
pub unsafe extern "C" fn rmdir(path: *const c_char) -> c_int {
    // SAFETY: the caller's promise.
    let name = match unsafe { text(path) } {
        Ok(name) => name,
        Err(code) => return fail(code),
    };
    with(|state| {
        let Some(files) = state.files() else { return fail(EACCES) };
        match files.stat(name) {
            Ok((Kind::Directory, _)) => {}
            Ok((Kind::File, _)) => return fail(ENOTDIR),
            Err(error) => return fail(reason(error)),
        }
        match files.remove(name) {
            Ok(()) => 0,
            Err(error) => fail(reason(error)),
        }
    })
}

// access's questions (unistd.h).
const W_OK: c_int = 2;

/// `access(path, mode)`: may this program use what is at `path` so?
/// F_OK: is it there. R_OK and X_OK: yes, if it is there - the service
/// lets a program that was given the files read them all. W_OK: only if
/// this program's channel to the files is one that may change them.
///
/// # Safety
/// `path` must be null or a string ending in a zero byte.
#[no_mangle]
pub unsafe extern "C" fn access(path: *const c_char, mode: c_int) -> c_int {
    // SAFETY: the caller's promise.
    let name = match unsafe { text(path) } {
        Ok(name) => name,
        Err(code) => return fail(code),
    };
    with(|state| {
        let Some(files) = state.files() else { return fail(EACCES) };
        if let Err(error) = files.stat(name) {
            return fail(reason(error));
        }
        if mode & W_OK != 0 {
            match files.read_only() {
                Ok(false) => {}
                Ok(true) => return fail(EROFS),
                Err(error) => return fail(reason(error)),
            }
        }
        0
    })
}

/// `chmod(path, mode)`: files have no permissions to change. 0 if the
/// file is there, so that a program setting them carries on.
///
/// # Safety
/// `path` must be null or a string ending in a zero byte.
#[no_mangle]
pub unsafe extern "C" fn chmod(path: *const c_char, _mode: c_uint) -> c_int {
    // SAFETY: the caller's promise.
    unsafe { access(path, 0) }
}

/// `fchmod(fd, mode)`: as chmod, for an open descriptor.
#[no_mangle]
pub extern "C" fn fchmod(fd: c_int, _mode: c_uint) -> c_int {
    let mut status = Stat::default();
    // SAFETY: `status` is writable.
    unsafe { fstat(fd, &mut status) }
}

/// `umask(mask)`: the permissions new files do not get. There are none
/// to take away; 022 is what a Unix program expects to hear.
#[no_mangle]
pub extern "C" fn umask(_mask: c_uint) -> c_uint {
    0o022
}

// ---- directories ----

// readdir's kinds (dirent.h).
const DT_DIR: u8 = 4;
const DT_REG: u8 = 8;

/// dirent.h's `struct dirent`.
#[repr(C)]
pub struct Dirent {
    d_ino: u64,
    d_off: i64,
    d_reclen: u16,
    d_type: u8,
    d_name: [c_char; 256],
}

/// dirent.h's `DIR`: the directory's names, read all at once when it is
/// opened (the file service lists a directory in one request), and
/// where readdir has got to.
pub struct Dir {
    path: String,
    entries: Vec<(String, u8)>,
    next: usize,
    entry: Dirent,
}

/// `opendir(path)`: a directory to read the names of with readdir, or
/// null with errno. "." and ".." come first, as on Unix.
///
/// # Safety
/// `path` must be null or a string ending in a zero byte.
#[no_mangle]
pub unsafe extern "C" fn opendir(path: *const c_char) -> *mut Dir {
    // SAFETY: the caller's promise.
    let name = match unsafe { text(path) } {
        Ok(name) => name,
        Err(code) => return fail_null(code),
    };
    let listed = with(|state| {
        let files = state.files().ok_or(EACCES)?;
        match files.stat(name).map_err(reason)? {
            (Kind::Directory, _) => {}
            (Kind::File, _) => return Err(ENOTDIR),
        }
        let mut entries = Vec::from([(String::from("."), DT_DIR), (String::from(".."), DT_DIR)]);
        files
            .list(name, |entry, kind, _size| {
                entries.push((String::from(entry), if matches!(kind, Kind::Directory) { DT_DIR } else { DT_REG }));
            })
            .map_err(reason)?;
        Ok((format!("/{}", files.resolve(name)), entries))
    });
    match listed {
        Ok((path, entries)) => Box::into_raw(Box::new(Dir {
            path,
            entries,
            next: 0,
            entry: Dirent { d_ino: 0, d_off: 0, d_reclen: 0, d_type: 0, d_name: [0; 256] },
        })),
        Err(code) => fail_null(code),
    }
}

/// `readdir(dir)`: the next name, or null at the end. The entry is the
/// DIR's own, overwritten by the next call.
///
/// # Safety
/// `dir` must come from opendir and not have been closed.
#[no_mangle]
pub unsafe extern "C" fn readdir(dir: *mut Dir) -> *mut Dirent {
    // SAFETY: the caller's promise.
    let dir = unsafe { &mut *dir };
    let Some((name, kind)) = dir.entries.get(dir.next) else {
        return ptr::null_mut();
    };
    dir.next += 1;
    let whole = match name.as_str() {
        "." => dir.path.clone(),
        ".." => String::from("/"), // only its number is affected, and nothing uses it
        _ if dir.path == "/" => format!("/{name}"),
        _ => format!("{}/{name}", dir.path),
    };
    let entry = &mut dir.entry;
    entry.d_ino = inode(&whole);
    entry.d_off = dir.next as i64;
    entry.d_reclen = core::mem::size_of::<Dirent>() as u16;
    entry.d_type = *kind;
    let n = name.len().min(255);
    for (to, from) in entry.d_name.iter_mut().zip(name.bytes().take(n)) {
        *to = from as c_char;
    }
    entry.d_name[n] = 0;
    entry
}

/// `rewinddir(dir)`: readdir starts again from the first name (the
/// names read when it was opened).
///
/// # Safety
/// `dir` must come from opendir and not have been closed.
#[no_mangle]
pub unsafe extern "C" fn rewinddir(dir: *mut Dir) {
    // SAFETY: the caller's promise.
    unsafe { (*dir).next = 0 };
}

/// `closedir(dir)`: done with it.
///
/// # Safety
/// `dir` must come from opendir and not have been closed.
#[no_mangle]
pub unsafe extern "C" fn closedir(dir: *mut Dir) -> c_int {
    if dir.is_null() {
        return fail(EBADF);
    }
    // SAFETY: the caller's promise: it came from Box::into_raw in opendir.
    drop(unsafe { Box::from_raw(dir) });
    0
}

// ---- marking a file changed -------------------------------------------

/// `UTIME_NOW` and `UTIME_OMIT` (sys/stat.h), in a timespec's
/// nanoseconds: now, and leave as it is.
const UTIME_NOW: c_long = (1 << 30) - 1;
const UTIME_OMIT: c_long = (1 << 30) - 2;
/// `AT_FDCWD` (fcntl.h): a path from the current directory.
const AT_FDCWD: c_int = -100;

/// What to do with a file's times, given `times[0]` (access) and
/// `times[1]` (change), or none: Ok(true) touch, Ok(false) nothing.
/// Only the change time is kept, as a stamp, and a stamp can be made
/// newer but not set to a time: a given one is refused (EPERM).
///
/// # Safety
/// `times` null or valid for reading two.
unsafe fn what_to_do(times: *const Time) -> Result<bool, c_int> {
    if times.is_null() {
        return Ok(true);
    }
    // SAFETY: the caller's promise.
    let change = unsafe { &*times.add(1) };
    match change.nanoseconds {
        UTIME_NOW => Ok(true),
        UTIME_OMIT => Ok(false),
        _ => Err(EPERM),
    }
}

/// Touches the file `name` (from the current directory).
fn touch(name: &str) -> Result<(), c_int> {
    with(|state| {
        let files = state.files().ok_or(EACCES)?;
        files.touch(name).map_err(reason)
    })
}

/// `utimensat(dirfd, path, times, flags)`: marks the file changed now
/// (times null, or `times[1]` UTIME_NOW). 0, or -1 with errno: EPERM
/// for a change time given as a time, which TheTaaJ cannot keep;
/// EINVAL for a `dirfd` other than AT_FDCWD (directories are not opened
/// as descriptors here) unless the path is from the root. `flags`
/// (AT_SYMLINK_NOFOLLOW) means nothing: there are no links.
///
/// # Safety
/// `path` null or a string ending in a zero byte; `times` null or valid
/// for reading two.
#[no_mangle]
pub unsafe extern "C" fn utimensat(dirfd: c_int, path: *const c_char, times: *const Time, _flags: c_int) -> c_int {
    // SAFETY: the caller's promise.
    let name = match unsafe { text(path) } {
        Ok(name) => name,
        Err(code) => return fail(code),
    };
    if dirfd != AT_FDCWD && !name.starts_with('/') {
        return fail(EINVAL);
    }
    // SAFETY: the caller's promise.
    match unsafe { what_to_do(times) }.and_then(|now| if now { touch(name) } else { Ok(()) }) {
        Ok(()) => 0,
        Err(code) => fail(code),
    }
}

/// `futimens(fd, times)`: as utimensat, for an open file.
///
/// # Safety
/// `times` null or valid for reading two.
#[no_mangle]
pub unsafe extern "C" fn futimens(fd: c_int, times: *const Time) -> c_int {
    let path = with(|state| match usize::try_from(fd).ok().and_then(|fd| state.table.get(fd)).and_then(Option::as_ref) {
        Some(Open::File(file)) => Ok(file.lock().path.clone()),
        Some(_) => Err(EINVAL),
        None => Err(EBADF),
    });
    // SAFETY: the caller's promise.
    let done = path.and_then(|path| unsafe { what_to_do(times) }.and_then(|now| if now { touch(&path) } else { Ok(()) }));
    match done {
        Ok(()) => 0,
        Err(code) => fail(code),
    }
}

/// `utime(path, times)` (utime.h) and `utimes(path, times)` (sys/time.h):
/// with no times, marks the file changed now; with times, EPERM, as
/// utimensat with a time.
///
/// # Safety
/// `path` null or a string ending in a zero byte.
#[no_mangle]
pub unsafe extern "C" fn utime(path: *const c_char, times: *const core::ffi::c_void) -> c_int {
    // SAFETY: the caller's promise.
    let name = match unsafe { text(path) } {
        Ok(name) => name,
        Err(code) => return fail(code),
    };
    if !times.is_null() {
        return fail(EPERM);
    }
    match touch(name) {
        Ok(()) => 0,
        Err(code) => fail(code),
    }
}

/// `utimes(path, times)`: as utime.
///
/// # Safety
/// As for utime.
#[no_mangle]
pub unsafe extern "C" fn utimes(path: *const c_char, times: *const core::ffi::c_void) -> c_int {
    // SAFETY: the caller's promise, passed on.
    unsafe { utime(path, times) }
}

fn fail_null<T>(code: c_int) -> *mut T {
    errno.store(code, core::sync::atomic::Ordering::Relaxed);
    ptr::null_mut()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_layout_is_what_sys_stat_h_says() {
        // dev, ino, nlink (8 each), mode, uid, gid (4 each), a hole to
        // the next 8, rdev, size, blksize, blocks, then three timespecs.
        assert_eq!(core::mem::offset_of!(Stat, st_mode), 24);
        assert_eq!(core::mem::offset_of!(Stat, st_rdev), 40);
        assert_eq!(core::mem::offset_of!(Stat, st_size), 48);
        assert_eq!(core::mem::offset_of!(Stat, st_mtim), 88);
        assert_eq!(core::mem::size_of::<Stat>(), 120);
        assert_eq!(core::mem::offset_of!(Dirent, d_type), 18);
        assert_eq!(core::mem::offset_of!(Dirent, d_name), 19);
    }

    #[test]
    fn a_path_has_one_number_and_different_paths_different_ones() {
        assert_eq!(inode("/docs/notes"), inode("/docs/notes"));
        assert_ne!(inode("/docs/notes"), inode("/docs/note"));
        assert_ne!(inode("/a/b"), inode("/ab"));
        assert_ne!(inode(""), 0);
    }
}

/// `chdir(path)`: names that do not start with '/' are taken from
/// `path` from now on, and getcwd says it. Only for this program: the
/// directory it was started in is the file service channel's, and the
/// channel stays; what changes is the directory this library puts in
/// front of names.
///
/// # Safety
/// `path` must be null or a string ending in a zero byte.
#[no_mangle]
pub unsafe extern "C" fn chdir(path: *const c_char) -> c_int {
    // SAFETY: the caller's promise.
    let name = match unsafe { text(path) } {
        Ok(name) => name,
        Err(code) => return fail(code),
    };
    with(|state| {
        let Some(files) = state.files() else { return fail(EACCES) };
        match files.stat(name) {
            Ok((Kind::Directory, _)) => {}
            Ok((Kind::File, _)) => return fail(ENOTDIR),
            Err(error) => return fail(reason(error)),
        }
        let moved = taaj::files::Files::in_directory(files.channel(), &files.resolve(name));
        state.files = Some(Some(alloc::sync::Arc::new(moved)));
        0
    })
}

/// Makes the file at `path` (from the root) exactly `length` bytes:
/// cut short, or with zeros added. The service can replace a file's
/// contents and write at an offset, but not cut one, so a shorter file
/// is read up to `length` and written again.
fn cut_to(files: &taaj::files::Files, path: &str, length: u64) -> Result<(), c_int> {
    let size = match files.stat(path).map_err(reason)? {
        (Kind::File, size) => size,
        (Kind::Directory, _) => return Err(EISDIR),
    };
    if length > size {
        // One zero at the new end: a write past the end leaves zeros in
        // between (as the descriptors' lseek says).
        files.write_from(path, length - 1, &[0]).map_err(reason)?;
        return Ok(());
    }
    if length == size {
        return Ok(());
    }
    let mut kept = Vec::with_capacity(usize::try_from(length).map_err(|_| EFBIG)?);
    while (kept.len() as u64) < length {
        let want = (length - kept.len() as u64) as usize;
        let got = files
            .read_from(path, kept.len() as u64, |piece| kept.extend_from_slice(&piece[..piece.len().min(want)]))
            .map_err(reason)?;
        if got == 0 {
            break;
        }
    }
    files.write(path, &kept).map_err(reason)?;
    Ok(())
}

/// `truncate(path, length)`: the file becomes `length` bytes long.
///
/// # Safety
/// `path` must be null or a string ending in a zero byte.
#[no_mangle]
pub unsafe extern "C" fn truncate(path: *const c_char, length: i64) -> c_int {
    // SAFETY: the caller's promise.
    let name = match unsafe { text(path) } {
        Ok(name) => name,
        Err(code) => return fail(code),
    };
    let Ok(length) = u64::try_from(length) else { return fail(EINVAL) };
    with(|state| {
        let Some(files) = state.files() else { return fail(EACCES) };
        let whole = format!("/{}", files.resolve(name));
        match cut_to(files, &whole, length) {
            Ok(()) => 0,
            Err(code) => fail(code),
        }
    })
}

/// `ftruncate(fd, length)`: truncate, for a file open for writing.
#[no_mangle]
pub extern "C" fn ftruncate(fd: c_int, length: i64) -> c_int {
    let Ok(length) = u64::try_from(length) else { return fail(EINVAL) };
    with(|state| {
        let path = match usize::try_from(fd).ok().and_then(|fd| state.table.get(fd)).and_then(Option::as_ref) {
            Some(Open::File(file)) if file.lock().writes() => file.lock().path.clone(),
            Some(_) => return fail(EINVAL), // POSIX: not a file open for writing
            None => return fail(EBADF),
        };
        let Some(files) = state.files() else { return fail(EBADF) };
        match cut_to(files, &path, length) {
            Ok(()) => 0,
            Err(code) => fail(code),
        }
    })
}

/// `link(from, to)`: a second name for a file. TheTaaJ's file system has
/// one name per file: EPERM, POSIX's "the file system does not support
/// links".
#[no_mangle]
pub extern "C" fn link(_from: *const c_char, _to: *const c_char) -> c_int {
    fail(EPERM)
}

/// `symlink(target, path)`: no symbolic links either: EPERM.
#[no_mangle]
pub extern "C" fn symlink(_target: *const c_char, _path: *const c_char) -> c_int {
    fail(EPERM)
}

/// `readlink(path, buffer, size)`: what a symbolic link points at. There
/// are none: EINVAL ("not a symbolic link") for anything that is there,
/// ENOENT for what is not.
///
/// # Safety
/// `path` must be null or a string ending in a zero byte.
#[no_mangle]
pub unsafe extern "C" fn readlink(path: *const c_char, _buffer: *mut c_char, _size: usize) -> isize {
    // SAFETY: the caller's promise.
    match unsafe { access(path, 0) } {
        0 => fail(EINVAL),
        _ => -1, // access said why
    }
}

/// `chown(path, owner, group)`: files have no owners. 0 if it is there,
/// changing nothing, as chmod.
///
/// # Safety
/// `path` must be null or a string ending in a zero byte.
#[no_mangle]
pub unsafe extern "C" fn chown(path: *const c_char, _owner: c_uint, _group: c_uint) -> c_int {
    // SAFETY: the caller's promise.
    unsafe { access(path, 0) }
}

/// `fchown(fd, owner, group)`: the same, for an open descriptor.
#[no_mangle]
pub extern "C" fn fchown(fd: c_int, _owner: c_uint, _group: c_uint) -> c_int {
    fchmod(fd, 0)
}
