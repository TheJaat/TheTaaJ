//! Names from a directory descriptor (J63): where the `*at` functions -
//! `openat`, `fstatat`, `mkdirat`, `unlinkat`, `renameat`, `faccessat`,
//! `utimensat` - send a name.
//!
//! POSIX added them so that a program can hold a directory open and
//! name things in it, whatever happens to the directory's own name
//! meanwhile: a directory moved while a program works in it, or a
//! program that must never reach outside it. TheTaaJ can do that for
//! real, because an open directory is a channel to the file service
//! that stays that directory (J60): a name sent on it is a name in that
//! directory, and the service lets no name out of it.
//!
//! So a name given with a directory descriptor goes one of three ways:
//!   - a name from the root ("/docs"), or AT_FDCWD: the program's
//!     namespace, as every other call (J62);
//!   - a plain name ("notes", "a/b"): on the directory's own channel -
//!     the directory, wherever it is now;
//!   - a name that climbs (".." in it): by name, from where the
//!     directory was opened, in the namespace. A directory's channel
//!     takes no "..": going up is the namespace's business, and the
//!     namespace knows the directory only by the name it was opened by.
//!
//! And one name only this program can answer (J66): /proc/self is this
//! program's own directory in /proc, /proc/PID, which the service can't
//! know - it doesn't know who asks. /proc/self/exe is the program's own
//! file, by the name it was started from (lib.rs's EXECUTABLE).

use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use core::ffi::c_int;

use crate::descriptors::{Open, SharedFile, State};
use crate::errno::{EACCES, EBADF, ENOENT, ENOTDIR};
use taaj::files::{path, Files, Kind};

/// fcntl.h's: a name from the current directory.
pub const AT_FDCWD: c_int = -100;
/// fcntl.h's: do not follow a last symbolic link. There are none, so
/// it changes nothing.
pub const AT_SYMLINK_NOFOLLOW: c_int = 0x100;
/// fcntl.h's, for unlinkat: remove a directory, as rmdir.
pub const AT_REMOVEDIR: c_int = 0x200;
/// fcntl.h's, for faccessat: as the program is, not as it started.
/// Everyone is the same here: it changes nothing.
pub const AT_EACCESS: c_int = 0x200;
/// fcntl.h's, for fstatat: an empty name means the descriptor itself.
pub const AT_EMPTY_PATH: c_int = 0x1000;

/// Where a name goes.
pub(crate) struct Place {
    /// The files to ask: the namespace, or a directory's own.
    pub(crate) files: Arc<Files>,
    /// The name to give `files`.
    pub(crate) name: String,
    /// The same, as a path from the root of the namespace: what a
    /// descriptor opened on it remembers (fchdir, fdopendir), and what
    /// stat numbers it by.
    pub(crate) shown: String,
    /// The directory's channel, if the name goes on one; None for the
    /// namespace. Two places with the same are one place to ask.
    pub(crate) on: Option<taaj::Handle>,
    /// The directory descriptor's file, held while its channel is used
    /// - a close on another thread can't take the channel away in the
    /// middle.
    _directory: Option<SharedFile>,
}

/// A name under /proc/self, as this program's own (J66): /proc/self/exe
/// is its file, anything else is under /proc/PID. None for any other
/// name; ENOENT for the file of a program that has none (built into the
/// kernel image).
fn own(shown: &str) -> Option<Result<String, c_int>> {
    let rest = shown.strip_prefix("/proc/self")?;
    if !rest.is_empty() && !rest.starts_with('/') {
        return None; // /proc/selfish
    }
    Some(match rest {
        "/exe" => crate::executable().map(String::from).ok_or(ENOENT),
        _ => Ok(format!("/proc/{}{rest}", taaj::process_id())),
    })
}

/// Where `name`, given with `dirfd`, goes. ENOENT for an empty name (as
/// POSIX: AT_EMPTY_PATH is each caller's); EBADF if `dirfd` is no
/// descriptor; ENOTDIR if it is not a directory; EACCES if this program
/// was given no files at all.
pub(crate) fn place(state: &mut State, dirfd: c_int, name: &str) -> Result<Place, c_int> {
    if name.is_empty() {
        return Err(ENOENT);
    }
    let in_namespace = |state: &mut State, name: String, directory| {
        let files = state.files().cloned().ok_or(EACCES)?;
        let shown = format!("/{}", files.resolve(&name));
        let (name, shown) = match own(&shown) {
            Some(Ok(shown)) => (shown.clone(), shown),
            Some(Err(code)) => return Err(code),
            None => (name, shown),
        };
        Ok(Place { files, name, shown, on: None, _directory: directory })
    };
    if dirfd == AT_FDCWD || name.starts_with('/') {
        return in_namespace(state, String::from(name), None);
    }
    let directory = match usize::try_from(dirfd).ok().and_then(|fd| state.table.get(fd)).and_then(Option::as_ref) {
        Some(Open::File(file)) => file.clone(),
        Some(_) => return Err(ENOTDIR),
        None => return Err(EBADF),
    };
    let (channel, kind, opened_as) = {
        let file = directory.lock();
        (file.file.channel(), file.file.kind(), file.path.clone())
    };
    if kind != Kind::Directory {
        return Err(ENOTDIR);
    }
    if name.split('/').any(|step| step == "..") {
        return in_namespace(state, format!("{opened_as}/{name}"), Some(directory));
    }
    let shown = format!("/{}", path::resolve(opened_as.trim_start_matches('/'), name));
    Ok(Place { files: Arc::new(Files::new(channel)), name: String::from(name), shown, on: Some(channel), _directory: Some(directory) })
}
