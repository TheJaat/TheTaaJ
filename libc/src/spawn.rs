//! Starting programs: `posix_spawn`, `posix_spawnp`, and waiting for
//! them - `waitpid`, `wait`, `wait4`.
//!
//! TheTaaJ starts a program as the shell does: this program reads the
//! file into a memory object (it has the files, init does not need
//! them) and asks init - the service "init" - to start it, handing down
//! some of its own handles: its standard output and error, and its
//! channel to the registry, so that the new program has the same names
//! as this one and no more. init gives it nothing of its own. A program
//! that was not given "init" cannot start another: EPERM.
//!
//! What the new program gets:
//!   - its arguments, `argv[1..]`, and its name `argv[0]`;
//!   - its file, as a path from the root (what it learns as its own:
//!     `__taaj_executable`);
//!   - this program's current directory;
//!   - this program's descriptors, as Unix's exec leaves them: all but
//!     those marked close-on-exec - standard output and error (and
//!     whether the output is the screen), and 3 and up (a file by its
//!     name, offset and flags; a pipe's end as a handle, both in a
//!     channel of their own, abi::startup::DESCRIPTORS, which libc's
//!     start reads into the new program's table, descriptors.rs). No
//!     standard input, though, unless a file action gives it one: a
//!     keyboard is read by whoever asked for the line;
//!   - this program's services;
//!   - this program's namespace: each directory it mounted, at the same
//!     place (J62, namespace.rs), with the descriptors;
//!   - the environment, `envp` (abi::init::ENVIRONMENT), which libc's
//!     start makes `environ` (lib.rs).
//!
//! File actions (`standard_for`) are done on what would be the new
//! program's descriptors, and it gets what comes out. As 0, 1 or 2, a
//! file is a stream the file service keeps (STREAM_OUT, STREAM_IN), so
//! that whatever the program writes reaches the file with nobody here
//! to copy it; as 3 and up, the file itself. Not attributes:
//! posix_spawn fails with ENOSYS if any were asked for.
//!
//! `system` and `popen` run a command line through the same: a
//! pipeline of programs with redirections (words.rs) is started from
//! here; anything more - `;`, `&&`, `$VAR`, wildcards - is handed to
//! the shell, `/sh -c command` (sh.rs, sh_system.rs), which starts its
//! commands through the same again.
//!
//! A process ID here is this program's own number for a child, from 2
//! up, good for waitpid: TheTaaJ names a process by a handle, not by a
//! number everyone shares.

use crate::descriptors::with;
use crate::descriptors::reason;
use crate::errno::{E2BIG, EACCES, EBADF, ECHILD, EINVAL, EIO, ENOENT, ENOEXEC, ENOMEM, ENOSYS, EPERM};

// open's flags (fcntl.h), as descriptors.rs has them.
const O_RDONLY: c_int = 0;
const O_ACCMODE: c_int = 3;
const O_CREAT: c_int = 0o100;
const O_TRUNC: c_int = 0o1000;
const O_WRONLY: c_int = 1;
const O_RDWR: c_int = 2;
const O_APPEND: c_int = 0o2000;
use alloc::string::String;
use alloc::vec::Vec;
use core::ffi::{c_char, c_int, CStr};
use core::sync::atomic::{AtomicI32, Ordering};
use taaj::Handle;

/// `posix_spawn_file_actions_t` (spawn.h): how many actions were added,
/// and the list of them, kept on the heap (`destroy` gives it back).
#[repr(C)]
pub struct FileActions {
    actions: c_int,
    _pad: c_int,
    list: *mut Vec<Action>,
    _reserved: [c_int; 4],
}

/// One file action: what to do to the new program's descriptors, in
/// order, before it starts - as Unix does them in the child between
/// fork and exec.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    /// `descriptor` open on `path`, with open's flags.
    Open { descriptor: c_int, path: String, flags: c_int },
    Close(c_int),
    /// `to` a copy of `from`.
    Dup2 { from: c_int, to: c_int },
    /// The new program's whole tree is the directory `path` names
    /// (posix_spawn_file_actions_addroot_np, J63): not an action on a
    /// descriptor, but on its namespace.
    Root(String),
}

/// `posix_spawnattr_t` (spawn.h).
#[repr(C)]
pub struct Attributes {
    flags: c_int,
    _reserved: [c_int; 7],
}

/// The children not yet waited for: their numbers, and their processes.
/// Behind a lock, which is held only to look or change, never while
/// waiting for a child: any thread may start or wait for one.
static CHILDREN: taaj::sync::Mutex<Vec<(c_int, Handle)>> = taaj::sync::Mutex::new(Vec::new());
static NEXT_ID: AtomicI32 = AtomicI32::new(2);

fn children<R>(f: impl FnOnce(&mut Vec<(c_int, Handle)>) -> R) -> R {
    f(&mut CHILDREN.lock())
}

/// `posix_spawn_file_actions_init(actions)`: no actions. 0.
///
/// # Safety
/// `actions` must be valid for writing.
#[no_mangle]
pub unsafe extern "C" fn posix_spawn_file_actions_init(actions: *mut FileActions) -> c_int {
    // SAFETY: the caller's promise.
    unsafe { actions.write(FileActions { actions: 0, _pad: 0, list: core::ptr::null_mut(), _reserved: [0; 4] }) };
    0
}

/// `posix_spawn_file_actions_destroy(actions)`: the list given back. 0.
///
/// # Safety
/// `actions` made by posix_spawn_file_actions_init.
#[no_mangle]
pub unsafe extern "C" fn posix_spawn_file_actions_destroy(actions: *mut FileActions) -> c_int {
    // SAFETY: the caller's promise; `list` is null or from Box::into_raw.
    unsafe {
        if let Some(actions) = actions.as_mut() {
            if !actions.list.is_null() {
                drop(alloc::boxed::Box::from_raw(actions.list));
            }
            actions.list = core::ptr::null_mut();
            actions.actions = 0;
        }
    }
    0
}

/// Adds one action. EBADF for a descriptor that cannot be one.
///
/// # Safety
/// `actions` made by posix_spawn_file_actions_init.
unsafe fn add(actions: *mut FileActions, action: Action) -> c_int {
    // SAFETY: the caller's promise.
    let Some(actions) = (unsafe { actions.as_mut() }) else { return EINVAL };
    let bad = |fd: c_int| !(0..OPEN_MAX).contains(&fd);
    if match &action {
        Action::Open { descriptor, .. } => bad(*descriptor),
        Action::Close(fd) => bad(*fd),
        Action::Dup2 { from, to } => bad(*from) || bad(*to),
        Action::Root(_) => false,
    } {
        return EBADF;
    }
    if actions.list.is_null() {
        actions.list = alloc::boxed::Box::into_raw(alloc::boxed::Box::new(Vec::new()));
    }
    // SAFETY: from Box::into_raw, and only this list's owner uses it.
    unsafe { (*actions.list).push(action) };
    actions.actions += 1;
    0
}

/// The descriptors a program may have (descriptors.rs's OPEN_MAX).
const OPEN_MAX: c_int = 32;

/// `posix_spawn_file_actions_addopen(actions, fd, path, flags, mode)`:
/// the new program's `fd` open on `path`. As 0, 1 or 2, a stream:
/// written - from the start with O_TRUNC, at the end otherwise - or
/// read, from the start (the file service's STREAM_OUT and STREAM_IN).
/// Above, the file, made ready here as open would (made, emptied) and
/// handed down by name. `mode` means nothing here: files have no
/// permissions.
///
/// # Safety
/// `actions` made by posix_spawn_file_actions_init; `path` a string.
#[no_mangle]
pub unsafe extern "C" fn posix_spawn_file_actions_addopen(
    actions: *mut FileActions,
    descriptor: c_int,
    path: *const c_char,
    flags: c_int,
    _mode: c_int,
) -> c_int {
    if path.is_null() {
        return EINVAL;
    }
    // SAFETY: the caller's promise: a string.
    let path = unsafe { CStr::from_ptr(path) }.to_string_lossy().into_owned();
    // SAFETY: the caller's promise.
    unsafe { add(actions, Action::Open { descriptor, path, flags }) }
}

/// `posix_spawn_file_actions_addclose(actions, fd)`: the new program
/// does not get `fd`.
///
/// # Safety
/// `actions` made by posix_spawn_file_actions_init.
#[no_mangle]
pub unsafe extern "C" fn posix_spawn_file_actions_addclose(actions: *mut FileActions, descriptor: c_int) -> c_int {
    // SAFETY: the caller's promise.
    unsafe { add(actions, Action::Close(descriptor)) }
}

/// `posix_spawn_file_actions_adddup2(actions, from, to)`: the new
/// program's `to` is what its `from` is - this program's, or what an
/// earlier action made it. A pipe's end given as 0, 1 or 2 is how one
/// program's output becomes another's input.
///
/// # Safety
/// `actions` made by posix_spawn_file_actions_init.
#[no_mangle]
pub unsafe extern "C" fn posix_spawn_file_actions_adddup2(actions: *mut FileActions, from: c_int, to: c_int) -> c_int {
    // SAFETY: the caller's promise.
    unsafe { add(actions, Action::Dup2 { from, to }) }
}

/// `posix_spawn_file_actions_addroot_np(actions, path)` (spawn.h,
/// TheTaaJ's own, as glibc's addchdir_np is glibc's): the new program's
/// whole tree is the directory `path` names - "/" is that directory,
/// its current directory too - and nothing else (J63, a sandbox).
///
/// It is a sandbox because of what the program is given, not because of
/// what it is asked not to do: the directory's channel, mounted at "/"
/// (J62), and a registry without "files", "files.look" or /tmp's "tmp" in it, so it
/// holds no channel to anything outside that directory, whatever it
/// does - a program written without libc included. The service lets no
/// name on a directory's channel out of it (".." is refused there). The
/// directory is mounted to change, if this program may change it, and
/// to read otherwise. Its other services and its descriptors it gets as
/// any program would: what it is handed, it has.
///
/// # Safety
/// `actions` made by posix_spawn_file_actions_init; `path` a string.
#[no_mangle]
pub unsafe extern "C" fn posix_spawn_file_actions_addroot_np(actions: *mut FileActions, path: *const c_char) -> c_int {
    if path.is_null() {
        return EINVAL;
    }
    // SAFETY: the caller's promise: a string.
    let path = unsafe { CStr::from_ptr(path) }.to_string_lossy().into_owned();
    // SAFETY: the caller's promise.
    unsafe { add(actions, Action::Root(path)) }
}

/// The directory a Root action names, open: to change if it may be,
/// to read if not. ENOTDIR, ENOENT and the rest as open says them.
fn open_root(path: &str) -> Result<taaj::files::File, c_int> {
    use taaj::files::{open, FileError, Status};
    let files = with(|state| state.files().cloned()).ok_or(EACCES)?;
    let wanted = open::DIRECTORY | open::READ;
    match files.open(path, wanted | open::WRITE) {
        Err(FileError::Refused(Status::ReadOnly)) => files.open(path, wanted),
        opened => opened,
    }
    .map_err(reason)
}

/// `posix_spawnattr_init(attributes)`: none. 0.
///
/// # Safety
/// `attributes` must be valid for writing.
#[no_mangle]
pub unsafe extern "C" fn posix_spawnattr_init(attributes: *mut Attributes) -> c_int {
    // SAFETY: the caller's promise.
    unsafe { attributes.write(Attributes { flags: 0, _reserved: [0; 7] }) };
    0
}

/// `posix_spawnattr_destroy(attributes)`: 0.
#[no_mangle]
pub extern "C" fn posix_spawnattr_destroy(_attributes: *mut Attributes) -> c_int {
    0
}

/// `posix_spawn(pid, path, actions, attributes, argv, envp)`: starts the
/// program in the file `path` (from the current directory, unless it
/// starts with '/'). 0 and the child's number in `*pid`, or an error
/// number (not -1 and errno: posix_spawn returns it).
///
/// # Safety
/// `pid` null or valid for writing; `path` a string; `actions` and
/// `attributes` null or valid; `argv` an array of strings ending in null.
#[no_mangle]
pub unsafe extern "C" fn posix_spawn(
    pid: *mut c_int,
    path: *const c_char,
    actions: *const FileActions,
    attributes: *const Attributes,
    argv: *const *const c_char,
    envp: *const *const c_char,
) -> c_int {
    // SAFETY: the caller's promise.
    unsafe { spawn(pid, path, actions, attributes, argv, envp, false) }
}

/// `posix_spawnp(...)`: posix_spawn, but a name with no '/' in it is a
/// command - a file in the root, as the shell runs it (TheTaaJ's PATH,
/// such as it is).
///
/// # Safety
/// As posix_spawn.
#[no_mangle]
pub unsafe extern "C" fn posix_spawnp(
    pid: *mut c_int,
    file: *const c_char,
    actions: *const FileActions,
    attributes: *const Attributes,
    argv: *const *const c_char,
    envp: *const *const c_char,
) -> c_int {
    // SAFETY: the caller's promise.
    unsafe { spawn(pid, file, actions, attributes, argv, envp, true) }
}

/// # Safety
/// As posix_spawn.
unsafe fn spawn(
    pid: *mut c_int,
    path: *const c_char,
    actions: *const FileActions,
    attributes: *const Attributes,
    argv: *const *const c_char,
    envp: *const *const c_char,
    command: bool,
) -> c_int {
    // SAFETY: the caller's promises.
    if unsafe { !attributes.is_null() && (*attributes).flags != 0 } {
        return ENOSYS;
    }
    // SAFETY: the caller's promise: null, or made by
    // posix_spawn_file_actions_init and added to.
    let actions: &[Action] = match unsafe { actions.as_ref() } {
        Some(actions) if !actions.list.is_null() => unsafe { &*actions.list },
        _ => &[],
    };
    if path.is_null() {
        return EINVAL;
    }
    // SAFETY: the caller's promise: a string.
    let path = unsafe { CStr::from_ptr(path) }.to_string_lossy().into_owned();
    let mut words = Vec::new();
    if !argv.is_null() {
        let mut at = argv;
        // SAFETY: an array of strings ending in null, as promised.
        while let Some(word) = unsafe { at.read().as_ref() } {
            words.push(unsafe { CStr::from_ptr(word) }.to_string_lossy().into_owned());
            at = unsafe { at.add(1) };
        }
    }
    // SAFETY: the caller's promise: null, or strings ending in null.
    let environment = unsafe { environment_of(envp) };
    match run(&path, &words, actions, command, &environment) {
        Ok(id) => {
            if !pid.is_null() {
                // SAFETY: the caller's promise.
                unsafe { pid.write(id) };
            }
            0
        }
        Err(code) => code,
    }
}

/// Starts the program `path` with `words` (argv, its name first) and
/// the file actions done: its number, for waitpid, or an error number.
/// (posix_spawn, and system and popen, which build their own actions.)
pub(crate) fn run(path: &str, words: &[String], actions: &[Action], command: bool, environment: &[u8]) -> Result<c_int, c_int> {
    let name = words.first().cloned().unwrap_or_else(|| String::from(path));
    // The last Root action, if any: the new program's tree.
    let root = match actions.iter().rev().find_map(|action| if let Action::Root(path) = action { Some(path) } else { None }) {
        Some(path) => Some(open_root(path)?),
        None => None,
    };
    let standard = standard_for(actions)?;
    let process = start(path, &name, words.get(1..).unwrap_or(&[]), command, standard, environment, root)?;
    Ok(adopt(process))
}

/// A child just started: numbered, listed for waitpid, and watched, so
/// that its end sends this program SIGCHLD (which does nothing unless
/// caught - signal.c). init started it, so it is not the kernel's idea
/// of this program's child: the kernel is asked to tell (PROCESS_NOTIFY).
pub(crate) fn adopt(process: Handle) -> c_int {
    let _ = taaj::process_notify(process, abi::signal::SIGCHLD);
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    children(|list| list.push((id, process)));
    id
}

extern "C" {
    /// This program's environment (libc/c/unix.c).
    static environ: *const *const c_char;
}

/// An environment - `NAME=value` strings, then null - as the words of a
/// Run request (abi::init::ENVIRONMENT): each ending in a zero byte.
/// Null is this program's own (`environ`); a string with no '=' is left
/// out.
///
/// # Safety
/// `list` null or an array of strings ending in null.
pub(crate) unsafe fn environment_of(list: *const *const c_char) -> Vec<u8> {
    // SAFETY: environ is always such an array (unix.c).
    let mut at = if list.is_null() { unsafe { environ } } else { list };
    let mut out = Vec::new();
    // SAFETY: the caller's promise, or environ's.
    while let Some(entry) = unsafe { at.read().as_ref() } {
        let entry = unsafe { CStr::from_ptr(entry) }.to_bytes();
        if entry.contains(&b'=') {
            out.extend_from_slice(entry);
            out.push(0);
        }
        at = unsafe { at.add(1) };
    }
    out
}

/// What a descriptor of the new program is, while the actions are done.
#[derive(Clone)]
enum Source {
    /// This program's own standard input, output or error.
    Input,
    Output,
    Error,
    /// A channel end: a pipe's, or a stream opened for the new program;
    /// `reads` for one to read from.
    Channel { handle: Handle, reads: bool },
    /// A file: its channel (with the descriptors that share it), the
    /// name it was opened by, the place, open's flags.
    File { file: crate::descriptors::SharedFile, offset: u64, flags: c_int },
}

/// One of the new program's descriptors beyond 0, 1 and 2, as it is
/// handed down (abi::descriptors): a handle of its own for a stream.
enum Given {
    /// A file: a copy of its channel, and the rest abi::descriptors's
    /// Entry::File says.
    File { handle: Handle, path: String, offset: u64, flags: c_int },
    Read(Handle),
    Write(Handle),
    /// A directory mounted in this program's namespace (J62): a copy of
    /// its channel, and where.
    Mount { handle: Handle, path: String },
}

/// The new program's descriptors: its standard input, output and error,
/// and the rest - handles of its own, to be handed down (closed by
/// whoever does not hand them).
pub(crate) struct Standard {
    input: Option<Handle>,
    output: Option<Handle>,
    error: Option<Handle>,
    /// Its output is this program's, and that is the screen.
    screen: bool,
    /// Its descriptors from 3 on.
    rest: Vec<(u8, Given)>,
}

impl Standard {
    fn close(self) {
        for handle in [self.input, self.output, self.error].into_iter().flatten() {
            let _ = taaj::close(handle);
        }
        for (_, given) in self.rest {
            let (Given::Read(handle) | Given::Write(handle) | Given::File { handle, .. } | Given::Mount { handle, .. }) = given;
            let _ = taaj::close(handle);
        }
    }
}

/// Does the actions on what would be the new program's descriptors and
/// makes handles for them.
///
/// It starts as this program's table - every descriptor, as Unix's
/// child has them between fork and exec - except standard input when
/// that is the keyboard or this program's own input (a keyboard, a file
/// or a pipe would be read by both: the new program gets one only when
/// asked for, by an action, or when this program's 0 was redirected).
/// Descriptors marked close-on-exec are there while the actions run -
/// a dup2 from one is how a pipe this library made for itself (pipes.rs
/// `private_pipe`) reaches the program - and are dropped after. So are
/// sockets, which cannot be handed on yet.
fn standard_for(actions: &[Action]) -> Result<Standard, c_int> {
    use crate::descriptors::Open;
    use taaj::rights::{DUPLICATE, RECEIVE, SEND, TRANSFER};
    // Streams opened for the new program: ours, closed once it has its
    // copies, whatever happens.
    let mut opened: Vec<Handle> = Vec::new();
    let result = with(|state| {
        // (descriptor, what it is, dropped at the end: close-on-exec)
        let mut table: Vec<(c_int, Source, bool)> = Vec::new();
        for (fd, open) in state.table.iter().enumerate() {
            let source = match open {
                Some(Open::Input) => None,
                Some(Open::Output) => Some(Source::Output),
                Some(Open::Error) => Some(Source::Error),
                Some(Open::PipeRead(end)) => Some(Source::Channel { handle: end.channel.0, reads: true }),
                Some(Open::PipeWrite(end)) => Some(Source::Channel { handle: end.0, reads: false }),
                Some(Open::File(shared)) => {
                    let (offset, flags) = {
                        let file = shared.lock();
                        (file.offset, file.flags)
                    };
                    Some(Source::File { file: shared.clone(), offset, flags })
                }
                Some(Open::Socket(_)) | None => None,
            };
            if let Some(source) = source {
                table.push((fd as c_int, source, state.is_cloexec(fd)));
            }
        }
        let set = |table: &mut Vec<(c_int, Source, bool)>, fd: c_int, source: Option<Source>| {
            table.retain(|(n, _, _)| *n != fd);
            if let Some(source) = source {
                table.push((fd, source, false));
            }
        };
        for action in actions {
            match action {
                // The namespace's, done by `run`.
                Action::Root(_) => {}
                Action::Close(fd) => set(&mut table, *fd, None),
                Action::Dup2 { from, to } => {
                    let source = table.iter().find(|(n, _, _)| n == from).map(|(_, source, _)| source.clone());
                    // This program's own input, which the table leaves
                    // out: given when asked for by number.
                    let source = match source {
                        Some(source) => source,
                        None if *from == 0 && matches!(state.table.first(), Some(Some(Open::Input))) => Source::Input,
                        None => return Err(EBADF),
                    };
                    set(&mut table, *to, Some(source));
                }
                Action::Open { descriptor, path, flags } if *descriptor > 2 => {
                    // Beyond 0, 1 and 2 a file stays a file: opened here,
                    // as open would, and its channel handed down.
                    let files = state.files().ok_or(EACCES)?;
                    let flags = flags & !crate::pipes::O_CLOEXEC;
                    let file = files.open(path, crate::descriptors::open_flags(flags)).map_err(reason)?;
                    let from_root = alloc::format!("/{}", files.resolve(path));
                    let file = crate::descriptors::shared(crate::descriptors::File { file, path: from_root, offset: 0, flags });
                    set(&mut table, *descriptor, Some(Source::File { file, offset: 0, flags }));
                }
                Action::Open { descriptor, path, flags } => {
                    let files = state.files().ok_or(EACCES)?;
                    // A stream goes one way: a file opened to read and
                    // write is read as standard input, written as the
                    // others.
                    let reads = match flags & O_ACCMODE {
                        O_RDONLY => true,
                        O_RDWR => *descriptor == 0,
                        _ => false,
                    };
                    if reads && flags & O_CREAT != 0 && files.stat(path).is_err() {
                        files.append(path, &[]).map_err(reason)?;
                    }
                    let stream = if reads {
                        files.stream_in(path)
                    } else {
                        if flags & O_CREAT == 0 {
                            files.stat(path).map_err(reason)?;
                        }
                        files.stream_out(path, flags & O_TRUNC == 0)
                    };
                    let stream = stream.map_err(reason)?;
                    opened.push(stream);
                    set(&mut table, *descriptor, Some(Source::Channel { handle: stream, reads }));
                }
            }
        }
        table.retain(|(fd, _, dropped)| !dropped && (0..OPEN_MAX).contains(fd));
        // A file as 0, 1 or 2 must be a stream: written to its end, or
        // read from where the descriptor had got to.
        let mut streams = Vec::new();
        for (fd, source, _) in &mut table {
            if let (0..=2, Source::File { file, offset, flags }) = (*fd, &*source) {
                let reads = flags & O_ACCMODE == O_RDONLY;
                // The file's lock let go before `source` is replaced.
                let stream = {
                    let file = file.lock();
                    if reads { file.file.stream_in_from(*offset) } else { file.file.stream_out() }
                };
                let stream = stream.map_err(reason)?;
                streams.push(stream);
                *source = Source::Channel { handle: stream, reads };
            }
        }
        opened.extend(streams);
        Ok(table)
    });
    let made = result.and_then(|table| {
        let find = |fd: c_int| table.iter().find(|(n, _, _)| *n == fd).map(|(_, source, _)| source.clone());
        let screen = matches!(find(1), Some(Source::Output)) && taaj::output_is_screen();
        // A handle of the new program's own for what `source` is.
        let copy = |source: &Source| -> Result<Option<(Handle, bool)>, c_int> {
            let (handle, reads) = match source {
                Source::Input => (taaj::stdin(), true),
                Source::Output => (taaj::stdout(), false),
                Source::Error => (taaj::stderr(), false),
                Source::Channel { handle, reads } => (Some(*handle), *reads),
                Source::File { .. } => return Ok(None),
            };
            // A standard handle this program was not given: the new one
            // does not get it either. One that cannot be handed on (a
            // keyboard: typed lines go to whoever asked) is refused.
            let Some(handle) = handle else { return Ok(None) };
            let rights = if reads { RECEIVE } else { SEND };
            taaj::duplicate(handle, rights | DUPLICATE | TRANSFER).map(|copy| Some((copy, reads))).map_err(|_| EPERM)
        };
        let mut standard = Standard { input: None, output: None, error: None, screen, rest: Vec::new() };
        for (fd, source, _) in &table {
            let made = match copy(source) {
                Ok(made) => made,
                Err(code) => {
                    standard.close();
                    return Err(code);
                }
            };
            match (*fd, made, source) {
                (0, made, _) => standard.input = made.map(|(handle, _)| handle),
                (1, made, _) => standard.output = made.map(|(handle, _)| handle),
                (2, made, _) => standard.error = made.map(|(handle, _)| handle),
                (fd, Some((handle, reads)), _) => {
                    standard.rest.push((fd as u8, if reads { Given::Read(handle) } else { Given::Write(handle) }))
                }
                (fd, None, Source::File { file, offset, flags }) => {
                    let file = file.lock();
                    match file.file.share() {
                        Ok(handle) => standard.rest.push((fd as u8, Given::File { handle, path: file.path.clone(), offset: *offset, flags: *flags })),
                        Err(_) => {
                            standard.close();
                            return Err(crate::errno::EMFILE);
                        }
                    }
                }
                (_, None, _) => {}
            }
        }
        Ok(standard)
    });
    for handle in opened {
        let _ = taaj::close(handle);
    }
    made
}

/// The new program's descriptors beyond 0, 1 and 2 as a channel to hand
/// down (abi::startup::DESCRIPTORS): one message each, the handles in
/// them; the far end closed, so that the program reads them all and
/// does not wait. None if there are none. The handles in `rest` go
/// with the messages, or are closed.
fn descriptors_channel(rest: Vec<(u8, Given)>, mounts: Vec<(String, Handle)>) -> Result<Option<Handle>, c_int> {
    use abi::descriptors::Entry;
    if rest.is_empty() && mounts.is_empty() {
        return Ok(None);
    }
    let channel = taaj::channel_create();
    let mut message = [0u8; abi::MESSAGE_MAX as usize];
    let mut failed = channel.is_err();
    // The mounts first, in the order they were made (a later one at the
    // same place, or deeper, is seen over an earlier one): the namespace
    // (J62, namespace.rs).
    let mounts = mounts.into_iter().map(|(path, handle)| (0u8, Given::Mount { handle, path }));
    for (descriptor, given) in mounts.chain(rest) {
        let (entry, handle) = match &given {
            Given::File { handle, path, offset, flags } => (Entry::File { descriptor, flags: *flags, offset: *offset, path }, Some(*handle)),
            Given::Read(handle) => (Entry::Read { descriptor }, Some(*handle)),
            Given::Write(handle) => (Entry::Write { descriptor }, Some(*handle)),
            Given::Mount { handle, path } => (Entry::Mount { path }, Some(*handle)),
        };
        let sent = match (&channel, entry.encode(&mut message)) {
            (Ok((tx, _)), Some(length)) if !failed => taaj::send(*tx, &message[..length], handle.as_slice()).is_ok(),
            _ => false,
        };
        if !sent {
            failed = true;
            if let Some(handle) = handle {
                let _ = taaj::close(handle);
            }
        }
    }
    match channel {
        Ok((tx, rx)) => {
            let _ = taaj::close(tx);
            if failed {
                let _ = taaj::close(rx);
                return Err(crate::errno::EMFILE);
            }
            Ok(Some(rx))
        }
        Err(_) => Err(ENOMEM),
    }
}

/// Reads the program and asks init to start it: the new process, or an
/// error number.
fn start(
    path: &str,
    name: &str,
    arguments: &[String],
    command: bool,
    standard: Standard,
    environment: &[u8],
    root: Option<taaj::files::File>,
) -> Result<Handle, c_int> {
    // The file, as a path from the root ("" is the root): a command is
    // a file in the root; anything else is a path from here.
    let search = command && !path.contains('/');
    let directories = if search { search_path() } else { Vec::new() };
    let found = with(|state| {
        let files = state.files().ok_or(ENOENT)?;
        let file = if search {
            // The first directory that has it (a file, not empty); if
            // none does, the error the first gives.
            let candidates: Vec<String> = directories
                .iter()
                .map(|directory| match directory.as_str() {
                    "" => files.resolve(path),
                    directory => files.resolve(&alloc::format!("{}/{path}", directory.trim_end_matches('/'))),
                })
                .collect();
            candidates
                .iter()
                .find(|file| matches!(files.stat(&alloc::format!("/{file}")), Ok((taaj::files::Kind::File, _))))
                .or(candidates.first())
                .cloned()
                .ok_or(ENOENT)?
        } else {
            files.resolve(path)
        };
        // `file` is from the root; the calls below take a path from the
        // current directory, so it goes to them as one from the root.
        let from_root = alloc::format!("/{file}");
        let size = files.size(&from_root).map_err(|_| ENOENT)?;
        if size == 0 || size > abi::PROGRAM_IMAGE_MAX {
            return Err(ENOEXEC);
        }
        let memory = taaj::memory_create(size).map_err(|_| ENOMEM)?;
        match files.read_into(&from_root, memory) {
            Ok(got) if got == size => Ok((file, String::from(files.directory()), memory)),
            _ => {
                let _ = taaj::close(memory);
                Err(EIO)
            }
        }
    });
    match found {
        // In its own tree, it starts at its root.
        Ok((file, directory, memory)) => {
            let directory = if root.is_some() { String::new() } else { directory };
            ask_init(&file, &directory, memory, name, arguments, standard, environment, root.as_ref())
        }
        Err(code) => {
            standard.close();
            Err(code)
        }
    }
}

extern "C" {
    fn getenv(name: *const c_char) -> *const c_char;
}

/// Where a command - a name with no '/' - is looked for, in order: the
/// directories in PATH, split at ':' (an empty one is the current
/// directory), or, with no PATH, the root and then /bin. Commands have
/// always been files in the root here; /bin is where a Unix program
/// expects them.
fn search_path() -> Vec<String> {
    // SAFETY: a string ending in a zero; getenv's answer is null or one.
    let path = unsafe { getenv(c"PATH".as_ptr()).as_ref() };
    match path {
        // SAFETY: getenv's answer, a string.
        Some(path) => unsafe { CStr::from_ptr(path) }.to_string_lossy().split(':').map(String::from).collect(),
        None => alloc::vec![String::from("/"), String::from("/bin")],
    }
}

/// A registry for a program in a tree of its own: this program's names,
/// each meaning what it means here, but the file services - "files",
/// "files.look", and /tmp's "tmp" and "tmp.look" (J64): every way out
/// of the tree. None if nothing is left.
fn sandbox_registry(services: &taaj::services::Services, name: &str) -> Result<Option<Handle>, c_int> {
    let names = services.list().map_err(|_| EPERM)?;
    let kept: Vec<(&str, &str)> = names.iter().map(String::as_str).filter(|name| !matches!(*name, "files" | "files.look" | "tmp" | "tmp.look")).map(|name| (name, name)).collect();
    if kept.is_empty() {
        return Ok(None);
    }
    services.narrow(name, &kept).map(Some).map_err(|_| EPERM)
}

/// The request to init, with the image and the handles handed down
/// (abi::init); init's reply comes on a channel of this request's own
/// (abi::init::REPLY), so that it is not some other program's.
fn ask_init(
    file: &str,
    directory: &str,
    image: Handle,
    name: &str,
    arguments: &[String],
    standard: Standard,
    environment: &[u8],
    root: Option<&taaj::files::File>,
) -> Result<Handle, c_int> {
    use abi::startup::{SCREEN, SERVICES, STDERR, STDIN, STDOUT};
    let mut given = alloc::vec![image];
    let close_given = |given: &[Handle]| {
        for &handle in given {
            let _ = taaj::close(handle);
        }
    };
    let Some(services) = taaj::services() else {
        close_given(&given);
        standard.close();
        return Err(EPERM);
    };
    let Ok(init) = services.lookup("init") else {
        close_given(&given);
        standard.close();
        return Err(EPERM); // not given the right to start programs
    };
    // A copy to hand down, which the new program may hand down again.
    use taaj::rights::{DUPLICATE, SEND, TRANSFER};
    let registry = match root {
        None => taaj::duplicate(services.handle(), SEND | DUPLICATE | TRANSFER).ok(),
        // In a tree of its own: every name this program has, but the
        // files. None at all if those were all.
        Some(_) => match sandbox_registry(&services, name) {
            Ok(registry) => registry,
            Err(code) => {
                close_given(&given);
                standard.close();
                let _ = taaj::close(init);
                return Err(code);
            }
        },
    };
    let Standard { input, output, error, screen, rest } = standard;
    let mut flags = 0u8;
    // In the order of their bits: stdout, services, stdin, stderr.
    for (handle, flag) in [(output, STDOUT), (registry, SERVICES), (input, STDIN), (error, STDERR)] {
        if let Some(handle) = handle {
            given.push(handle);
            flags |= flag as u8;
        }
    }
    if output.is_some() && screen {
        flags |= SCREEN as u8;
    }
    // This program's mounts, each directory's channel shared: the child
    // sees the same names (J62). A mount whose channel can't be copied
    // is left out.
    let mounts = match root {
        // Its tree: the one directory, at "/".
        Some(root) => root.share().ok().map(|handle| (String::from("/"), handle)).into_iter().collect(),
        None => with(|state| state.files().cloned())
            .map(|files| {
                files
                    .mounts()
                    .iter()
                    .filter_map(|mount| mount.directory.share().ok().map(|handle| (alloc::format!("/{}", mount.at), handle)))
                    .collect()
            })
            .unwrap_or_default(),
    };
    // Then the descriptors from 3 on, and the mounts, if there are any:
    // one channel.
    let descriptors = match descriptors_channel(rest, mounts) {
        Ok(descriptors) => descriptors,
        Err(code) => {
            close_given(&given);
            let _ = taaj::close(init);
            return Err(code);
        }
    };
    let with_descriptors = if let Some(channel) = descriptors {
        given.push(channel);
        abi::init::DESCRIPTORS
    } else {
        0
    };
    let mut encoded = [0u8; abi::MESSAGE_MAX as usize];
    let Some(length) = abi::arguments::encode(arguments.iter().map(String::as_str), &mut encoded) else {
        close_given(&given);
        let _ = taaj::close(init);
        return Err(E2BIG);
    };
    // The environment, if there is one, travels with the arguments
    // (abi::init::ENVIRONMENT).
    let mut packed = [0u8; abi::MESSAGE_MAX as usize];
    let (kind, arguments) = if environment.is_empty() {
        (abi::init::IMAGE | abi::init::REPLY | with_descriptors, &encoded[..length])
    } else {
        let Some(size) = abi::init::pack(&encoded[..length], environment, &mut packed) else {
            close_given(&given);
            let _ = taaj::close(init);
            return Err(E2BIG);
        };
        (abi::init::IMAGE | abi::init::REPLY | abi::init::ENVIRONMENT | with_descriptors, &packed[..size])
    };
    let whole = alloc::format!("/{file}");
    let request = abi::init::Run { kind, flags, name, directory, file: &whole, arguments };
    let mut message = [0u8; abi::MESSAGE_MAX as usize];
    let Some(size) = request.encode(&mut message) else {
        close_given(&given);
        let _ = taaj::close(init);
        return Err(E2BIG);
    };
    let Ok((mine, theirs)) = taaj::channel_create() else {
        close_given(&given);
        let _ = taaj::close(init);
        return Err(ENOMEM);
    };
    given.push(theirs);
    let sent = taaj::send(init, &message[..size], &given);
    let _ = taaj::close(init);
    if sent.is_err() {
        close_given(&given); // not sent: still ours
        let _ = taaj::close(mine);
        return Err(EIO);
    }
    let mut answer = [0u8; 128];
    let mut process = [0u64; 1];
    let reply = taaj::receive(mine, &mut answer, &mut process);
    let _ = taaj::close(mine);
    match reply {
        Ok((2, 1)) if &answer[..2] == b"ok" => Ok(process[0]),
        Ok((n, count)) => {
            if count > 0 {
                let _ = taaj::close(process[0]);
            }
            Err(if answer[..n].ends_with(b"is not a program") { ENOEXEC } else { EIO })
        }
        Err(_) => Err(EIO),
    }
}

/// The exec family, as TheTaaJ can do it. A process here is not
/// replaced by another: there is no such call, and nothing a program
/// holds - its handles, its memory - could be kept across one. So exec
/// starts `path` as a new process, with everything exec would leave it
/// - the arguments, the environment, the current directory, every
/// descriptor not marked close-on-exec, standard input too - waits for
/// it, and ends this one with exactly the exit code it ended with (a
/// signal's too): whoever waits for this program sees what Unix would
/// have shown. What differs: this program stays in memory meanwhile,
/// it is the new program's parent rather than becoming it, and the
/// process ID changes. Returns (-1, with errno) only if the program
/// could not be started, as exec does.
///
/// # Safety
/// `path` a string; `argv` and `envp` null or arrays of strings ending
/// in null.
unsafe fn exec(path: *const c_char, argv: *const *const c_char, envp: *const *const c_char, command: bool) -> c_int {
    use crate::descriptors::fail;
    if path.is_null() {
        return fail(crate::errno::EFAULT);
    }
    // SAFETY: the caller's promise.
    let path = unsafe { CStr::from_ptr(path) }.to_string_lossy().into_owned();
    let mut words = Vec::new();
    if !argv.is_null() {
        let mut at = argv;
        // SAFETY: an array of strings ending in null, as promised.
        while let Some(word) = unsafe { at.read().as_ref() } {
            words.push(unsafe { CStr::from_ptr(word) }.to_string_lossy().into_owned());
            at = unsafe { at.add(1) };
        }
    }
    // SAFETY: the caller's promise.
    let environment = unsafe { environment_of(envp) };
    // Standard input, as exec leaves it. If this program has none (0
    // closed: EBADF), the new one has none either; nor if it is a
    // keyboard, which cannot be handed on (EPERM: typed lines go to
    // whoever asked for them, spawn.rs) - the new program then starts
    // with no input, as one started by posix_spawn does.
    let id = match run(&path, &words, &[Action::Dup2 { from: 0, to: 0 }], command, &environment) {
        Err(EBADF | EPERM) => run(&path, &words, &[], command, &environment),
        other => other,
    };
    let id = match id {
        Ok(id) => id,
        Err(code) => return fail(code),
    };
    let Some(handle) = children(|list| {
        let at = list.iter().position(|&(other, _)| other == id)?;
        Some(list.remove(at).1)
    }) else {
        return fail(ECHILD);
    };
    // The program is running: from here on, this one only waits. A
    // signal sent to it meanwhile is not passed on.
    match taaj::wait_any(&[handle]) {
        Ok((_, code)) => taaj::exit(code),
        Err(_) => taaj::exit(127),
    }
}

/// `execve(path, argv, envp)`: runs `path` in this program's place
/// (see `exec`). -1 with errno if it could not be started.
///
/// # Safety
/// As `exec`.
#[no_mangle]
pub unsafe extern "C" fn execve(path: *const c_char, argv: *const *const c_char, envp: *const *const c_char) -> c_int {
    // SAFETY: the caller's promise.
    unsafe { exec(path, argv, envp, false) }
}

/// `execv(path, argv)`: execve with this program's environment.
///
/// # Safety
/// As `exec`.
#[no_mangle]
pub unsafe extern "C" fn execv(path: *const c_char, argv: *const *const c_char) -> c_int {
    // SAFETY: the caller's promise.
    unsafe { exec(path, argv, core::ptr::null(), false) }
}

/// `execvp(file, argv)`: execv, a name with no '/' looked for in PATH
/// (`search_path`).
///
/// # Safety
/// As `exec`.
#[no_mangle]
pub unsafe extern "C" fn execvp(file: *const c_char, argv: *const *const c_char) -> c_int {
    // SAFETY: the caller's promise.
    unsafe { exec(file, argv, core::ptr::null(), true) }
}

/// `execvpe(file, argv, envp)`: execvp with an environment of its own.
///
/// # Safety
/// As `exec`.
#[no_mangle]
pub unsafe extern "C" fn execvpe(file: *const c_char, argv: *const *const c_char, envp: *const *const c_char) -> c_int {
    // SAFETY: the caller's promise.
    unsafe { exec(file, argv, envp, true) }
}

/// What waitpid says of a child that ended with `code`: its exit code
/// in the second byte, or the signal that ended it in the first - the
/// one it was sent (abi::signal::exit_code), or the one Unix would have
/// stopped it with: SIGSEGV for a program the kernel killed for
/// breaking a rule, SIGKILL for one stopped on request (the shell's
/// `kill`), SIGPIPE for one whose reader went away.
fn status_of(code: i64) -> c_int {
    match code {
        abi::KILLED => 11,
        abi::STOPPED => 9,
        abi::NO_READER => 13,
        code => match abi::signal::of_exit_code(code) {
            Some(signal) => signal as c_int,
            None => ((code & 0xff) as c_int) << 8,
        },
    }
}

/// `waitpid(process, status, options)`: waits for the child `process`
/// to end (or any, for -1, 0 or any other number below 1 - there are no
/// process groups), says how in `*status`, and returns its number. With
/// WNOHANG (1), 0 if none has ended yet. -1 with ECHILD if there is no
/// such child.
///
/// # Safety
/// `status` must be null or valid for writing.
#[no_mangle]
pub unsafe extern "C" fn waitpid(process: c_int, status: *mut c_int, options: c_int) -> c_int {
    let waiting: Vec<(c_int, Handle)> =
        children(|list| list.iter().copied().filter(|&(id, _)| process < 1 || id == process).collect());
    if waiting.is_empty() {
        return crate::descriptors::fail(ECHILD);
    }
    let handles: Vec<Handle> = waiting.iter().map(|&(_, handle)| handle).collect();
    if options & 1 != 0 && matches!(taaj::object_wait_until(&handles, 0), Ok(None)) {
        return 0;
    }
    let Ok((index, code)) = taaj::wait_any(&handles) else {
        return crate::descriptors::fail(ECHILD);
    };
    let (id, handle) = waiting[index];
    children(|list| list.retain(|&(other, _)| other != id));
    let _ = taaj::close(handle);
    if !status.is_null() {
        // SAFETY: the caller's promise.
        unsafe { status.write(status_of(code)) };
    }
    id
}

/// signal.h's siginfo_t, as waitid fills it.
#[repr(C)]
pub struct SigInfo {
    si_signo: c_int,
    si_errno: c_int,
    si_code: c_int,
    si_pid: c_int,
    si_addr: *mut u8,
    si_uid: u32,
    si_status: c_int,
}

const P_ALL: c_int = 0;
const P_PID: c_int = 1;
const WNOHANG: c_int = 1;
const WEXITED: c_int = 4;
const WNOWAIT: c_int = 0x0100_0000;
const CLD_EXITED: c_int = 1;
const CLD_KILLED: c_int = 2;

/// `waitid(type, id, info, options)`: waitpid, saying more. `type` is
/// P_ALL (any child) or P_PID (child `id`); `options` must ask for
/// WEXITED - the only way a child changes here - and may add WNOHANG
/// (if none has ended: 0, with `info->si_pid` 0) and WNOWAIT (report the
/// child, but leave it to be waited for again). `*info` says who
/// (si_pid), how (si_code: CLD_EXITED, or CLD_KILLED by a signal) and
/// with what (si_status: the exit code, or the signal). 0, or -1 with
/// ECHILD (no such child) or EINVAL.
///
/// # Safety
/// `info` must be valid for writing.
#[no_mangle]
pub unsafe extern "C" fn waitid(kind: c_int, id: u32, info: *mut SigInfo, options: c_int) -> c_int {
    if (kind != P_ALL && kind != P_PID) || options & WEXITED == 0 || info.is_null() {
        return crate::descriptors::fail(EINVAL);
    }
    let waiting: Vec<(c_int, Handle)> =
        children(|list| list.iter().copied().filter(|&(child, _)| kind == P_ALL || child as u32 == id).collect());
    if waiting.is_empty() {
        return crate::descriptors::fail(ECHILD);
    }
    let handles: Vec<Handle> = waiting.iter().map(|&(_, handle)| handle).collect();
    // SAFETY: the caller's promise.
    let info = unsafe { &mut *info };
    *info = SigInfo { si_signo: 0, si_errno: 0, si_code: 0, si_pid: 0, si_addr: core::ptr::null_mut(), si_uid: 0, si_status: 0 };
    if options & WNOHANG != 0 && matches!(taaj::object_wait_until(&handles, 0), Ok(None)) {
        return 0;
    }
    let Ok((index, code)) = taaj::wait_any(&handles) else {
        return crate::descriptors::fail(ECHILD);
    };
    let (child, handle) = waiting[index];
    if options & WNOWAIT == 0 {
        children(|list| list.retain(|&(other, _)| other != child));
        let _ = taaj::close(handle);
    }
    let status = status_of(code);
    let signal = status & 0x7f;
    info.si_signo = abi::signal::SIGCHLD as c_int;
    info.si_pid = child;
    (info.si_code, info.si_status) = if signal != 0 { (CLD_KILLED, signal) } else { (CLD_EXITED, (status >> 8) & 0xff) };
    0
}

/// `__taaj_signal_child(id, signal)` (for signal.c's kill): `signal` to
/// the child numbered `id` - 0 only asks whether there is one. 0, or an
/// error number: EPERM if this program started no such child (it may
/// be anyone's; TheTaaJ gives a program no way to reach a process it
/// was not handed, and so no way to know).
#[no_mangle]
pub extern "C" fn __taaj_signal_child(id: c_int, signal: c_int) -> c_int {
    let Some(handle) = children(|list| list.iter().find(|&&(other, _)| other == id).map(|&(_, handle)| handle)) else {
        return EPERM;
    };
    if signal == 0 {
        return 0;
    }
    match taaj::signal_send(handle, signal as u64) {
        Ok(()) => 0,
        Err(_) => crate::errno::EINVAL,
    }
}

/// `wait(status)`: waitpid for any child.
///
/// # Safety
/// As waitpid.
#[no_mangle]
pub unsafe extern "C" fn wait(status: *mut c_int) -> c_int {
    // SAFETY: the caller's promise.
    unsafe { waitpid(-1, status, 0) }
}

/// `wait4(process, status, options, usage)`: waitpid that also says how
/// much the child used. TheTaaJ does not count that, so `*usage` is all
/// zeros.
///
/// # Safety
/// As waitpid; `usage` null or valid for writing a struct rusage.
#[no_mangle]
pub unsafe extern "C" fn wait4(process: c_int, status: *mut c_int, options: c_int, usage: *mut u8) -> c_int {
    // SAFETY: the caller's promise.
    let id = unsafe { waitpid(process, status, options) };
    if id > 0 && !usage.is_null() {
        // SAFETY: the caller's promise: room for a struct rusage
        // (libc/include/sys/resource.h: two timevals and 14 longs).
        unsafe { usage.write_bytes(0, 2 * 16 + 14 * 8) };
    }
    id
}

// ---- system -----------------------------------------------------------

/// Writes `text` to standard error (descriptor 2, wherever it now goes).
fn complain(text: &str) {
    // SAFETY: the bytes of a live string.
    let _ = unsafe { crate::descriptors::write(2, text.as_ptr().cast(), text.len()) };
}

/// `system(command)`: runs a command and waits for it; its status, as
/// waitpid gives it.
///
/// A simple command - words as sh would split them (quotes and
/// backslashes work), pipes (`a | b`) and redirections (`<`, `>`,
/// `>>`, `2>`, `2>&1`, `>&2`) - is started from here (words.rs,
/// `pipeline_of`), each command's first word as posix_spawnp starts a
/// name: a bare name is the file of that name in the root. That is
/// what sh would do with it, without starting sh. A pipeline's status
/// is its last command's, as in sh.
///
/// A command that needs more of a shell - `;`, `&&`, `$`, a wildcard,
/// `VAR=x cmd` - is run by `/sh -c command`, as POSIX says system runs
/// every command. If there is no /sh, it is not run: -1, with ENOSYS,
/// and a line on standard error saying why. A program that cannot be
/// started gives status 127, as sh's "not found" does, with a line
/// saying why, and so does a redirection's file that cannot be opened;
/// one that may not be started (no "init" service) -1 with EPERM. An
/// empty command does nothing, 0. `system(NULL)` says 1: commands can
/// be run.
///
/// # Safety
/// `command` null or a string ending in a zero byte.
#[no_mangle]
pub unsafe extern "C" fn system(command: *const c_char) -> c_int {
    if command.is_null() {
        return 1;
    }
    // SAFETY: the caller's promise.
    let command = unsafe { CStr::from_ptr(command) }.to_string_lossy().into_owned();
    match pipeline(&command, None, None) {
        Ok(ids) => ids.into_iter().fold(0, |_, id| reap(id)),
        Err(Failed::Refused(code)) => crate::descriptors::fail(code),
        Err(Failed::NotStarted(_)) => 127 << 8,
    }
}

/// Why a pipeline did not run.
pub(crate) enum Failed {
    /// -1 and this errno: not run at all.
    Refused(c_int),
    /// A command could not be started, for this reason, and the reason
    /// was said: 127.
    NotStarted(c_int),
}

/// One program of a pipeline to start: its file (as posix_spawnp takes
/// a name), its words (its name first), what to do to its descriptors,
/// and its environment as a Run request has it.
pub(crate) struct Job {
    pub path: String,
    pub words: Vec<String>,
    pub actions: Vec<Action>,
    pub environment: Vec<u8>,
}

/// open's flags for a redirection to a file.
pub(crate) fn output_flags(append: bool) -> c_int {
    O_WRONLY | O_CREAT | if append { O_APPEND } else { O_TRUNC }
}

/// open's flags for reading a file.
pub(crate) const INPUT_FLAGS: c_int = O_RDONLY;

/// open's flags for `<>`: reading and writing, the file made if it is
/// not there.
pub(crate) const READ_WRITE_FLAGS: c_int = O_RDWR | O_CREAT;

/// Waits for child `id`; its status.
pub(crate) fn reap(id: c_int) -> c_int {
    let mut status = 127 << 8;
    // SAFETY: a pointer to our own int.
    unsafe { waitpid(id, &mut status, 0) };
    status
}

/// Closes one of this program's descriptors, quietly.
pub(crate) fn close_fd(fd: Option<c_int>) {
    if let Some(fd) = fd {
        crate::descriptors::close(fd);
    }
}

/// Starts the commands of `command`, each one's output the next one's
/// input; the children, in order. `input`, if given, is a descriptor of
/// this program the first command reads, and `output` one the last
/// writes; both are closed here, whatever happens. A command that
/// cannot be started stops the rest; those already running are waited
/// for. What needs a shell is started as one command: `sh -c command`.
fn pipeline(command: &str, input: Option<c_int>, output: Option<c_int>) -> Result<Vec<c_int>, Failed> {
    use crate::words::{Redirection, Unshelled};
    // SAFETY: environ is always an array of strings.
    let environment = unsafe { environment_of(core::ptr::null()) };
    // A builtin of sh's (echo, cd, exit...) is no program: sh runs it.
    let parsed = crate::words::pipeline_of(command).and_then(|stages| {
        match stages.iter().find(|stage| crate::sh::is_builtin(&stage.words[0])) {
            Some(stage) => Err(Unshelled::Builtin(stage.words[0].clone())),
            None => Ok(stages),
        }
    });
    let jobs: Vec<Job> = match parsed {
        Ok(stages) => stages
            .into_iter()
            .map(|stage| Job {
                path: stage.words[0].clone(),
                actions: stage
                    .redirections
                    .iter()
                    .map(|redirection| match redirection {
                        Redirection::In { fd, path } => Action::Open { descriptor: *fd as c_int, path: path.clone(), flags: INPUT_FLAGS },
                        Redirection::Out { fd, path, append } => {
                            Action::Open { descriptor: *fd as c_int, path: path.clone(), flags: output_flags(*append) }
                        }
                        Redirection::Copy { fd, to } => Action::Dup2 { from: *to as c_int, to: *fd as c_int },
                    })
                    .collect(),
                words: stage.words,
                environment: environment.clone(),
            })
            .collect(),
        Err(why) => {
            let there = with(|state| state.files().is_some_and(|files| files.size("/sh").is_ok_and(|size| size > 0)));
            if !there {
                close_fd(input);
                close_fd(output);
                let why = match why {
                    Unshelled::Needs(c) => alloc::format!("'{c}' needs a shell"),
                    Unshelled::Unfinished => String::from("a quote or backslash is not finished"),
                    Unshelled::Assignment => String::from("setting a variable for a command needs a shell"),
                    Unshelled::Malformed => String::from("a pipe or a redirection has nothing on one side"),
                    Unshelled::Builtin(name) => alloc::format!("{name} is a shell's"),
                };
                complain(&alloc::format!("system: cannot run \"{command}\": {why}, and there is no /sh\n"));
                return Err(Failed::Refused(ENOSYS));
            }
            let words = ["sh", "-c", command].map(String::from).to_vec();
            alloc::vec![Job { path: String::from("sh"), words, actions: Vec::new(), environment }]
        }
    };
    start_jobs(&jobs, input, output, "system")
}

/// Starts `jobs`, each one's output the next one's input, as `pipeline`
/// says; `who` starts the lines said about what went wrong.
pub(crate) fn start_jobs(jobs: &[Job], input: Option<c_int>, mut output: Option<c_int>, who: &str) -> Result<Vec<c_int>, Failed> {
    let mut ids = Vec::new();
    let mut reading = input;
    let mut failed = None;
    for (n, job) in jobs.iter().enumerate() {
        let mut actions = Vec::new();
        if let Some(from) = reading {
            actions.push(Action::Dup2 { from, to: 0 });
        }
        // The pipe to the next command: its write end this one's output.
        let mut next = None;
        let writing;
        if n + 1 < jobs.len() {
            let mut ends = [0; 2];
            if crate::pipes::private_pipe(&mut ends) != 0 {
                complain(&alloc::format!("{who}: no descriptor for a pipe\n"));
                failed = Some(Failed::Refused(crate::errno::EMFILE));
                break;
            }
            next = Some(ends[0]);
            writing = Some(ends[1]);
        } else {
            writing = output.take();
        }
        if let Some(from) = writing {
            actions.push(Action::Dup2 { from, to: 1 });
        }
        actions.extend(job.actions.iter().cloned());
        let name = &job.words[0];
        let started = standard_for(&actions)
            .inspect_err(|&code| {
                complain(&alloc::format!("{who}: {name}: a redirection's file cannot be opened: {}\n", describe(code)));
            })
            .and_then(|standard| {
                start(&job.path, name, &job.words[1..], true, standard, &job.environment, None).inspect_err(|&code| {
                    // system says nothing of EPERM: its -1 says it.
                    if code != EPERM || who != "system" {
                        complain(&alloc::format!("{who}: {name}: {}\n", describe(code)));
                    }
                })
            });
        // This program's copies of the ends it gave: the reader sees the
        // end of its input only when every writer's end is closed.
        close_fd(reading);
        close_fd(writing);
        reading = next;
        match started {
            Ok(process) => ids.push(adopt(process)),
            Err(EPERM) => {
                failed = Some(Failed::Refused(EPERM));
                break;
            }
            Err(code) => {
                failed = Some(Failed::NotStarted(code));
                break;
            }
        }
    }
    close_fd(reading);
    close_fd(output);
    match failed {
        None => Ok(ids),
        Some(failed) => {
            for id in ids {
                reap(id);
            }
            Err(failed)
        }
    }
}

/// An error number, said for a line on standard error.
fn describe(code: c_int) -> &'static str {
    match code {
        ENOENT => "not found",
        ENOEXEC => "not a program",
        ENOMEM => "no memory for it",
        EPERM => "not given the right to start programs",
        crate::errno::EISDIR => "it is a directory",
        crate::errno::EROFS => "the files are read-only here",
        EACCES => "no files were given",
        crate::errno::ENFILE => "the file service has too many streams open",
        _ => "it could not be read",
    }
}

/// The pipelines popen started, by the descriptor it gave for each.
static POPENED: taaj::sync::Mutex<Vec<(c_int, Handle)>> = taaj::sync::Mutex::new(Vec::new());

/// What popen (unix.c) calls: starts `command`, and gives a descriptor
/// of this program for it - reading its last command's output, or with
/// `writing`, writing its first one's input. -1 and errno if it was not
/// started.
///
/// # Safety
/// `command` a string ending in a zero byte.
#[no_mangle]
pub unsafe extern "C" fn __taaj_popen(command: *const c_char, writing: c_int) -> c_int {
    if command.is_null() {
        return crate::descriptors::fail(EINVAL);
    }
    // SAFETY: the caller's promise.
    let command = unsafe { CStr::from_ptr(command) }.to_string_lossy().into_owned();
    let mut ends = [0; 2];
    if crate::pipes::private_pipe(&mut ends) != 0 {
        return -1;
    }
    let (ours, started) = if writing != 0 {
        (ends[1], pipeline(&command, Some(ends[0]), None))
    } else {
        (ends[0], pipeline(&command, None, Some(ends[1])))
    };
    match started {
        Ok(ids) => {
            POPENED.lock().extend(ids.into_iter().map(|id| (ours, id as Handle)));
            ours
        }
        Err(failed) => {
            crate::descriptors::close(ours);
            crate::descriptors::fail(match failed {
                Failed::Refused(code) => code,
                Failed::NotStarted(_) => ENOENT,
            })
        }
    }
}

/// What pclose (unix.c) calls, once the stream is closed: waits for the
/// commands popen started for `fd`; the last one's status. -1 and ECHILD
/// if popen gave no such descriptor.
#[no_mangle]
pub extern "C" fn __taaj_pclose(fd: c_int) -> c_int {
    let ids: Vec<c_int> = {
        let mut list = POPENED.lock();
        let ids = list.iter().filter(|&&(of, _)| of == fd).map(|&(_, id)| id as c_int).collect();
        list.retain(|&(of, _)| of != fd);
        ids
    };
    if ids.is_empty() {
        return crate::descriptors::fail(ECHILD);
    }
    ids.into_iter().fold(0, |_, id| reap(id))
}
