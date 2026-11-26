//! A namespace per process (J62): `taaj_mount` and `taaj_unmount`
//! (taaj.h).
//!
//! Every name a program uses - `open("/data/notes")` - is worked out by
//! this library before it is sent anywhere: which directory it is in,
//! and what it is called there. Until now there was one answer, the
//! file service's root. Now a program has a table of its own: a
//! directory, open as a channel to its service (J60), seen at a path
//! the program chooses. A name under that path goes to that directory,
//! with the rest of the name; the deepest mount wins; anything else goes
//! to the root, as before. The table is `taaj::files::Files`'s, and
//! nothing about it is in the kernel or the file service: a mount is a
//! handle and a path, in the program's own memory.
//!
//! A program's mounts are handed down to every program it starts
//! (posix_spawn, spawn.rs: one MOUNT message each with the descriptors,
//! abi::descriptors), so a namespace is the parent's to give - Plan 9's
//! idea, which Unix approached with mount namespaces. What a child is
//! not given it can't name: J63 builds sandboxes on that.
//!
//! What it changes for a C program:
//!   - `..` is worked out in the namespace, by name: `/data/..` is `/`,
//!     whatever directory is mounted at `/data`;
//!   - `rename` between two mounts fails with EXDEV, as between two file
//!     systems on Unix (copy, then remove: what `mv` does);
//!   - a mount point can't be removed or renamed: EBUSY;
//!   - a directory listing shows a mount in it as a directory, even if
//!     the directory underneath has nothing by that name.

use alloc::sync::Arc;
use core::ffi::{c_char, c_int};

use crate::descriptors::{fail, reason, text, with};
use crate::errno::EACCES;
use taaj::files::{open, Files};

/// `taaj_mount`'s flag: the directory is mounted to read only. Every
/// change under it is refused (EROFS), whatever the directory allows.
pub const TAAJ_MOUNT_READ_ONLY: c_int = 1;

/// `taaj_mount(at, directory, flags)` (taaj.h): from now on, the
/// directory `directory` names is seen at `at` too, in place of what is
/// there - which is out of sight until `taaj_unmount(at)`. `at` need
/// not be there. Both names are worked out as any name is, so
/// `directory` may be under another mount. Returns 0, or -1 and errno:
/// ENOENT or ENOTDIR if `directory` is not a directory; EROFS to mount
/// for changing a directory this program may only read; EINVAL for an
/// unknown flag.
///
/// # Safety
/// `at` and `directory` must be null or strings ending in a zero byte.
#[no_mangle]
pub unsafe extern "C" fn taaj_mount(at: *const c_char, directory: *const c_char, flags: c_int) -> c_int {
    if flags & !TAAJ_MOUNT_READ_ONLY != 0 {
        return fail(crate::errno::EINVAL);
    }
    // SAFETY: the caller's promise.
    let (at, directory) = match unsafe { (text(at), text(directory)) } {
        (Ok(at), Ok(directory)) => (at, directory),
        (Err(code), _) | (_, Err(code)) => return fail(code),
    };
    let Some(files) = with(|state| state.files().cloned()) else { return fail(EACCES) };
    let writes = if flags & TAAJ_MOUNT_READ_ONLY != 0 { 0 } else { open::WRITE };
    // Asked with the table let go: the service may take its time.
    let opened = match files.open(directory, open::DIRECTORY | open::READ | writes) {
        Ok(opened) => opened,
        Err(error) => return fail(reason(error)),
    };
    match files.mounted(at, opened) {
        Ok(mounted) => replace(mounted),
        Err(error) => fail(reason(error)),
    }
}

/// `taaj_unmount(at)` (taaj.h): what is mounted at `at` is not any
/// more, and what was there before is seen again. Programs already
/// started keep the mounts they were given. Returns 0, or -1 and errno
/// EINVAL if nothing is mounted at `at`.
///
/// # Safety
/// `at` must be null or a string ending in a zero byte.
#[no_mangle]
pub unsafe extern "C" fn taaj_unmount(at: *const c_char) -> c_int {
    // SAFETY: the caller's promise.
    let at = match unsafe { text(at) } {
        Ok(at) => at,
        Err(code) => return fail(code),
    };
    let Some(files) = with(|state| state.files().cloned()) else { return fail(EACCES) };
    match files.unmounted(at) {
        Ok(unmounted) => replace(unmounted),
        Err(_) => fail(crate::errno::EINVAL),
    }
}

/// The namespace from now on. A thread that took the old one a moment
/// ago finishes with it: each `Files` is whole, and they share the
/// directories.
fn replace(files: Files) -> c_int {
    with(|state| state.files = Some(Some(Arc::new(files))));
    0
}
