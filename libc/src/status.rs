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

use crate::at::AT_FDCWD;
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
///
/// A file with a time of its own (TaajFS version 6: nanoseconds since
/// 1970, from the clock of whoever wrote it) is told that instead: a
/// real date, for `ls -l` and for people. The two kinds order rightly
/// against each other: every file with a time was written after every
/// file with only a stamp (seconds since 1970 that small are before
/// any clock TheTaaJ has read), and the volume never gives a newer
/// change an older time.
fn time_of(stamp: u64, time: u64) -> Time {
    if time != 0 {
        let seconds = c_long::try_from(time / 1_000_000_000).unwrap_or(c_long::MAX);
        return Time { seconds, nanoseconds: (time % 1_000_000_000) as c_long };
    }
    Time { seconds: c_long::try_from(stamp.max(1)).unwrap_or(c_long::MAX), nanoseconds: 0 }
}

fn describe(path: &str, kind: Kind, size: u64, stamp: u64, time: u64) -> Stat {
    let size = i64::try_from(size).unwrap_or(i64::MAX);
    Stat {
        st_atim: time_of(stamp, time),
        st_mtim: time_of(stamp, time),
        st_ctim: time_of(stamp, time),
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
    // SAFETY: the caller's promise, passed on.
    unsafe { fstatat(AT_FDCWD, path, out, 0) }
}

/// `fstatat(dirfd, path, out, flags)`: stat, with `path` taken from the
/// directory `dirfd` is open on (at.rs); with AT_EMPTY_PATH and an
/// empty `path`, fstat of `dirfd` itself. AT_SYMLINK_NOFOLLOW changes
/// nothing: there are no links.
///
/// # Safety
/// As for `stat`.
#[no_mangle]
pub unsafe extern "C" fn fstatat(dirfd: c_int, path: *const c_char, out: *mut Stat, flags: c_int) -> c_int {
    use crate::at::{AT_EMPTY_PATH, AT_SYMLINK_NOFOLLOW};
    // SAFETY: the caller's promise.
    let name = match unsafe { text(path) } {
        Ok(name) => name,
        Err(code) => return fail(code),
    };
    if flags & !(AT_EMPTY_PATH | AT_SYMLINK_NOFOLLOW) != 0 {
        return fail(EINVAL);
    }
    if name.is_empty() && flags & AT_EMPTY_PATH != 0 {
        // SAFETY: the caller's promise.
        return unsafe { fstat(dirfd, out) };
    }
    let found = with(|state| crate::at::place(state, dirfd, name)).and_then(|place| {
        let (kind, size, stamp, time) = place.files.stat_timed(&place.name).map_err(reason)?;
        Ok(describe(&place.shown, kind, size, stamp, time))
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
/// on its own channel - so it is the file, wherever its name went; the
/// three standard ones are character devices, and a pipe's end is a
/// FIFO.
///
/// # Safety
/// `out` must be valid for writing.
#[no_mangle]
pub unsafe extern "C" fn fstat(fd: c_int, out: *mut Stat) -> c_int {
    // Either the answer at once, or the file to ask, once the table is
    // let go: the service may take its time.
    let found = with(|state| match usize::try_from(fd).ok().and_then(|fd| state.table.get(fd)).and_then(Option::as_ref) {
        Some(Open::File(file)) => Ok(Err(file.clone())),
        Some(Open::PipeRead(_) | Open::PipeWrite(_)) => {
            Ok(Ok(Stat { st_dev: 1, st_ino: 1 + fd as u64, st_nlink: 1, st_mode: S_IFIFO | 0o600, st_blksize: 4096, ..Stat::default() }))
        }
        Some(Open::Input | Open::Output | Open::Error | Open::Socket(_)) => Ok(Ok(Stat {
            st_dev: 1,
            st_ino: 1 + fd as u64,
            st_nlink: 1,
            st_mode: S_IFCHR | 0o620,
            st_blksize: 4096,
            ..Stat::default()
        })),
        None => Err(EBADF),
    })
    .and_then(|found| match found {
        Ok(status) => Ok(status),
        Err(file) => {
            let file = file.lock();
            let (kind, size, stamp, time) = file.file.stat_timed().map_err(reason)?;
            Ok(describe(&file.path, kind, size, stamp, time))
        }
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
pub unsafe extern "C" fn mkdir(path: *const c_char, mode: c_uint) -> c_int {
    // SAFETY: the caller's promise, passed on.
    unsafe { mkdirat(AT_FDCWD, path, mode) }
}

/// `mkdirat(dirfd, path, mode)`: mkdir, with `path` taken from the
/// directory `dirfd` is open on (at.rs).
///
/// # Safety
/// `path` must be null or a string ending in a zero byte.
#[no_mangle]
pub unsafe extern "C" fn mkdirat(dirfd: c_int, path: *const c_char, _mode: c_uint) -> c_int {
    // SAFETY: the caller's promise.
    let name = match unsafe { text(path) } {
        Ok(name) => name,
        Err(code) => return fail(code),
    };
    let made = with(|state| crate::at::place(state, dirfd, name)).and_then(|place| place.files.make_directory(&place.name).map_err(reason));
    match made {
        Ok(()) => 0,
        Err(code) => fail(code),
    }
}

/// `rmdir(path)`: removes an empty directory.
///
/// # Safety
/// `path` must be null or a string ending in a zero byte.
#[no_mangle]
pub unsafe extern "C" fn rmdir(path: *const c_char) -> c_int {
    // SAFETY: the caller's promise, passed on.
    unsafe { crate::descriptors::unlinkat(AT_FDCWD, path, crate::at::AT_REMOVEDIR) }
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
    // SAFETY: the caller's promise, passed on.
    unsafe { faccessat(AT_FDCWD, path, mode, 0) }
}

/// `faccessat(dirfd, path, mode, flags)`: access, with `path` taken
/// from the directory `dirfd` is open on (at.rs). W_OK asks that
/// directory's channel - one opened to read refuses changes - or, for
/// the namespace, the files this program was given. The flags
/// (AT_EACCESS, AT_SYMLINK_NOFOLLOW) change nothing: everyone is the
/// same, and there are no links.
///
/// # Safety
/// `path` must be null or a string ending in a zero byte.
#[no_mangle]
pub unsafe extern "C" fn faccessat(dirfd: c_int, path: *const c_char, mode: c_int, flags: c_int) -> c_int {
    use crate::at::{AT_EACCESS, AT_SYMLINK_NOFOLLOW};
    // SAFETY: the caller's promise.
    let name = match unsafe { text(path) } {
        Ok(name) => name,
        Err(code) => return fail(code),
    };
    if flags & !(AT_EACCESS | AT_SYMLINK_NOFOLLOW) != 0 {
        return fail(EINVAL);
    }
    let allowed = with(|state| crate::at::place(state, dirfd, name)).and_then(|place| {
        place.files.stat(&place.name).map_err(reason)?;
        if mode & W_OK != 0 && place.files.read_only().map_err(reason)? {
            return Err(EROFS);
        }
        Ok(())
    });
    match allowed {
        Ok(()) => 0,
        Err(code) => fail(code),
    }
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

/// `fchmodat(dirfd, path, mode, flags)`: as chmod, from `dirfd`.
///
/// # Safety
/// `path` must be null or a string ending in a zero byte.
#[no_mangle]
pub unsafe extern "C" fn fchmodat(dirfd: c_int, path: *const c_char, _mode: c_uint, _flags: c_int) -> c_int {
    // SAFETY: the caller's promise.
    unsafe { faccessat(dirfd, path, 0, 0) }
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
    /// The descriptor it reads (fdopendir), or dirfd made for it; -1
    /// for none. closedir closes it.
    fd: c_int,
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
            fd: -1,
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
    let dir = unsafe { Box::from_raw(dir) };
    if dir.fd >= 0 {
        crate::descriptors::close(dir.fd);
    }
    0
}

/// The path from the root that descriptor `fd` was opened on, if it is
/// a file's or a directory's.
fn path_of(fd: c_int) -> Result<String, c_int> {
    match crate::descriptors::open_at(fd) {
        Some(crate::descriptors::Open::File(file)) => Ok(file.lock().path.clone()),
        Some(_) => Err(ENOTDIR),
        None => Err(EBADF),
    }
}

/// `fdopendir(fd)`: opendir on what `fd` - opened with O_DIRECTORY - is
/// open on. The DIR then owns `fd`: closedir closes it.
#[no_mangle]
pub extern "C" fn fdopendir(fd: c_int) -> *mut Dir {
    let path = match path_of(fd) {
        Ok(path) => path,
        Err(code) => return fail_null(code),
    };
    let Ok(path) = alloc::ffi::CString::new(path) else { return fail_null(EINVAL) };
    // SAFETY: a string ending in a zero.
    let dir = unsafe { opendir(path.as_ptr()) };
    if !dir.is_null() {
        // SAFETY: just made.
        unsafe { (*dir).fd = fd };
    }
    dir
}

/// `dirfd(dir)`: a descriptor for the directory `dir` reads - the one
/// fdopendir was given, or one opened now (O_DIRECTORY) and kept until
/// closedir. -1 with errno if it cannot be opened.
///
/// # Safety
/// `dir` must come from opendir or fdopendir and not have been closed.
#[no_mangle]
pub unsafe extern "C" fn dirfd(dir: *mut Dir) -> c_int {
    // SAFETY: the caller's promise.
    let dir = unsafe { &mut *dir };
    if dir.fd < 0 {
        let Ok(path) = alloc::ffi::CString::new(dir.path.clone()) else { return fail(EINVAL) };
        // SAFETY: a string ending in a zero.
        dir.fd = unsafe { crate::descriptors::__taaj_open(path.as_ptr(), crate::descriptors::O_DIRECTORY) };
    }
    dir.fd
}

/// `fchdir(fd)`: chdir to the directory `fd` (opened with O_DIRECTORY)
/// is open on. ENOTDIR for anything else, EBADF for no descriptor.
#[no_mangle]
pub extern "C" fn fchdir(fd: c_int) -> c_int {
    let path = match path_of(fd) {
        Ok(path) => path,
        Err(code) => return fail(code),
    };
    let Ok(path) = alloc::ffi::CString::new(path) else { return fail(EINVAL) };
    // SAFETY: a string ending in a zero.
    unsafe { chdir(path.as_ptr()) }
}

// ---- marking a file changed -------------------------------------------

/// `UTIME_NOW` and `UTIME_OMIT` (sys/stat.h), in a timespec's
/// nanoseconds: now, and leave as it is.
const UTIME_NOW: c_long = (1 << 30) - 1;
const UTIME_OMIT: c_long = (1 << 30) - 2;

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

/// Touches the file `name`, from `dirfd` (at.rs).
fn touch(dirfd: c_int, name: &str) -> Result<(), c_int> {
    let place = with(|state| crate::at::place(state, dirfd, name))?;
    place.files.touch(&place.name).map_err(reason)
}

/// `utimensat(dirfd, path, times, flags)`: marks the file changed now
/// (times null, or `times[1]` UTIME_NOW), with `path` taken from the
/// directory `dirfd` is open on (at.rs). 0, or -1 with errno: EPERM for
/// a change time given as a time, which TheTaaJ cannot keep. `flags`
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
    // SAFETY: the caller's promise.
    match unsafe { what_to_do(times) }.and_then(|now| if now { touch(dirfd, name) } else { Ok(()) }) {
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
    let file = with(|state| match usize::try_from(fd).ok().and_then(|fd| state.table.get(fd)).and_then(Option::as_ref) {
        Some(Open::File(file)) => Ok(file.clone()),
        Some(_) => Err(EINVAL),
        None => Err(EBADF),
    });
    // The file itself, on its own channel: wherever its name went.
    let touch_it = |file: crate::descriptors::SharedFile| file.lock().file.touch().map_err(reason);
    // SAFETY: the caller's promise.
    let done = file.and_then(|file| unsafe { what_to_do(times) }.and_then(|now| if now { touch_it(file) } else { Ok(()) }));
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
    match touch(AT_FDCWD, name) {
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
        // The same namespace, with names taken from there.
        let moved = files.moved_to(name);
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
    let file = with(|state| match usize::try_from(fd).ok().and_then(|fd| state.table.get(fd)).and_then(Option::as_ref) {
        Some(Open::File(file)) if file.lock().writes() => Ok(file.clone()),
        Some(_) => Err(EINVAL), // POSIX: not a file open for writing
        None => Err(EBADF),
    });
    // On the file's own channel, the table let go: the service sets the
    // size (protocol::TRUNCATE), filling with zeros when it grows.
    match file.and_then(|file| file.lock().file.truncate(length).map_err(reason)) {
        Ok(()) => 0,
        Err(code) => fail(code),
    }
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
pub unsafe extern "C" fn readlink(path: *const c_char, buffer: *mut c_char, size: usize) -> isize {
    // SAFETY: the caller's promise, passed on.
    unsafe { readlinkat(AT_FDCWD, path, buffer, size) }
}

/// `readlinkat(dirfd, path, buffer, size)`: readlink, from `dirfd`.
///
/// # Safety
/// `path` must be null or a string ending in a zero byte.
#[no_mangle]
pub unsafe extern "C" fn readlinkat(dirfd: c_int, path: *const c_char, _buffer: *mut c_char, _size: usize) -> isize {
    // SAFETY: the caller's promise.
    match unsafe { faccessat(dirfd, path, 0, 0) } {
        0 => fail(EINVAL),
        _ => -1, // faccessat said why
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

/// `fchownat(dirfd, path, owner, group, flags)`: as chown, from `dirfd`.
///
/// # Safety
/// `path` must be null or a string ending in a zero byte.
#[no_mangle]
pub unsafe extern "C" fn fchownat(dirfd: c_int, path: *const c_char, _owner: c_uint, _group: c_uint, _flags: c_int) -> c_int {
    // SAFETY: the caller's promise.
    unsafe { faccessat(dirfd, path, 0, 0) }
}

/// `fchown(fd, owner, group)`: the same, for an open descriptor.
#[no_mangle]
pub extern "C" fn fchown(fd: c_int, _owner: c_uint, _group: c_uint) -> c_int {
    fchmod(fd, 0)
}
