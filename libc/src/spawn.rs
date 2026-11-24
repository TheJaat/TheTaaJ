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
//!   - this program's standard output and standard error, and whether
//!     the output is the screen; no standard input - a keyboard is read
//!     by whoever asked for the line, and a file or pipe would be read
//!     by both - unless a file action gives it one;
//!   - this program's services;
//!   - the environment, `envp` (abi::init::ENVIRONMENT), which libc's
//!     start makes `environ` (lib.rs).
//!
//! File actions (`standard_for`) are done on what would be the new
//! program's descriptors, and it gets the 0, 1 and 2 that come out: a
//! pipe's end (pipes.rs), this program's own, or a file - as a stream
//! the file service keeps (STREAM_OUT, STREAM_IN), so that whatever the
//! program writes reaches the file with nobody here to copy it. Not
//! attributes: posix_spawn fails with ENOSYS if any were asked for.
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
}

/// `posix_spawnattr_t` (spawn.h).
#[repr(C)]
pub struct Attributes {
    flags: c_int,
    _reserved: [c_int; 7],
}

/// The children not yet waited for: their numbers, and their processes.
struct Children {
    list: core::cell::UnsafeCell<Vec<(c_int, Handle)>>,
}

// SAFETY: a C program has one thread on TheTaaJ (as the rest of this
// library assumes), and every use is a short, non-reentrant borrow.
unsafe impl Sync for Children {}

static CHILDREN: Children = Children { list: core::cell::UnsafeCell::new(Vec::new()) };
static NEXT_ID: AtomicI32 = AtomicI32::new(2);

fn children<R>(f: impl FnOnce(&mut Vec<(c_int, Handle)>) -> R) -> R {
    // SAFETY: see Children; nothing called here touches CHILDREN again.
    f(unsafe { &mut *CHILDREN.list.get() })
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
/// the new program's `fd` open on `path`. Only 0, 1 and 2 reach it (a
/// program here is given no others), as a stream: written - from the
/// start with O_TRUNC, at the end otherwise - or read, from the start
/// (the file service's STREAM_OUT and STREAM_IN). `mode` means nothing
/// here: files have no permissions.
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
    let standard = standard_for(actions)?;
    let process = start(path, &name, words.get(1..).unwrap_or(&[]), command, standard, environment)?;
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    children(|list| list.push((id, process)));
    Ok(id)
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
#[derive(Clone, Copy)]
enum Source {
    /// This program's own standard input, output or error.
    Input,
    Output,
    Error,
    /// A channel end: a pipe's, or a stream opened for the new program.
    Channel(Handle),
}

/// The new program's standard input, output and error: handles of its
/// own, to be handed down (closed by whoever does not hand them).
pub(crate) struct Standard {
    input: Option<Handle>,
    output: Option<Handle>,
    error: Option<Handle>,
    /// Its output is this program's, and that is the screen.
    screen: bool,
}

impl Standard {
    fn close(self) {
        for handle in [self.input, self.output, self.error].into_iter().flatten() {
            let _ = taaj::close(handle);
        }
    }
}

/// Does the actions on what would be the new program's descriptors -
/// at first this program's, but with no standard input (a keyboard, a
/// file or a pipe would be read by both) - and makes handles for its 0,
/// 1 and 2.
fn standard_for(actions: &[Action]) -> Result<Standard, c_int> {
    use crate::descriptors::Open;
    use taaj::rights::{DUPLICATE, RECEIVE, SEND, TRANSFER};
    // Streams opened for the new program: ours, closed once it has its
    // copies, whatever happens.
    let mut opened: Vec<Handle> = Vec::new();
    let result = with(|state| {
        let mut table: Vec<(c_int, Source)> = alloc::vec![(1, Source::Output), (2, Source::Error)];
        let set = |table: &mut Vec<(c_int, Source)>, fd: c_int, source: Option<Source>| {
            table.retain(|&(n, _)| n != fd);
            if let Some(source) = source {
                table.push((fd, source));
            }
        };
        // This program's descriptor `fd`, as the new one would have it:
        // only what was not touched by an action yet.
        let mut touched: Vec<c_int> = alloc::vec![0, 1, 2];
        for action in actions {
            match action {
                Action::Close(fd) => {
                    set(&mut table, *fd, None);
                    touched.push(*fd);
                }
                Action::Dup2 { from, to } => {
                    let source = match table.iter().find(|&&(n, _)| n == *from) {
                        Some(&(_, source)) => source,
                        None if touched.contains(from) => return Err(EBADF),
                        None => {
                            let open = state.table.get(*from as usize).and_then(Option::as_ref).ok_or(EBADF)?;
                            match open {
                                Open::Input => Source::Input,
                                Open::Output => Source::Output,
                                Open::Error => Source::Error,
                                Open::PipeRead(end) => Source::Channel(end.borrow().channel.0),
                                Open::PipeWrite(end) => Source::Channel(end.0),
                                Open::File(file) => {
                                    let files = state.files.as_ref().and_then(Option::as_ref).ok_or(EBADF)?;
                                    let stream = if file.writes() { files.stream_out(&file.path, true) } else { files.stream_in(&file.path) };
                                    let stream = stream.map_err(reason)?;
                                    opened.push(stream);
                                    Source::Channel(stream)
                                }
                                Open::Socket(_) => return Err(EBADF),
                            }
                        }
                    };
                    set(&mut table, *to, Some(source));
                    touched.push(*to);
                }
                Action::Open { descriptor, path, flags } => {
                    let files = state.files().ok_or(EACCES)?;
                    let stream = if flags & O_ACCMODE == O_RDONLY {
                        files.stream_in(path)
                    } else {
                        if flags & O_CREAT == 0 {
                            files.stat(path).map_err(reason)?;
                        }
                        files.stream_out(path, flags & O_TRUNC == 0)
                    };
                    let stream = stream.map_err(reason)?;
                    opened.push(stream);
                    set(&mut table, *descriptor, Some(Source::Channel(stream)));
                    touched.push(*descriptor);
                }
            }
        }
        let find = |fd: c_int| table.iter().find(|&&(n, _)| n == fd).map(|&(_, source)| source);
        Ok((find(0), find(1), find(2)))
    });
    let made = result.and_then(|(input, output, error)| {
        let screen = matches!(output, Some(Source::Output)) && taaj::output_is_screen();
        let copy = |source: Option<Source>, rights: u32| -> Result<Option<Handle>, c_int> {
            let handle = match source {
                None => return Ok(None),
                Some(Source::Input) => taaj::stdin(),
                Some(Source::Output) => taaj::stdout(),
                Some(Source::Error) => taaj::stderr(),
                Some(Source::Channel(handle)) => Some(handle),
            };
            // A standard handle this program was not given: the new one
            // does not get it either. One that cannot be handed on (a
            // keyboard: typed lines go to whoever asked) is refused.
            let Some(handle) = handle else { return Ok(None) };
            taaj::duplicate(handle, rights | DUPLICATE | TRANSFER).map(Some).map_err(|_| EPERM)
        };
        let input = copy(input, RECEIVE)?;
        let output = match copy(output, SEND) {
            Ok(output) => output,
            Err(code) => {
                if let Some(handle) = input {
                    let _ = taaj::close(handle);
                }
                return Err(code);
            }
        };
        let error = match copy(error, SEND) {
            Ok(error) => error,
            Err(code) => {
                for handle in input.into_iter().chain(output) {
                    let _ = taaj::close(handle);
                }
                return Err(code);
            }
        };
        Ok(Standard { input, output, error, screen })
    });
    for handle in opened {
        let _ = taaj::close(handle);
    }
    made
}

/// Reads the program and asks init to start it: the new process, or an
/// error number.
fn start(path: &str, name: &str, arguments: &[String], command: bool, standard: Standard, environment: &[u8]) -> Result<Handle, c_int> {
    // The file, as a path from the root ("" is the root): a command is
    // a file in the root; anything else is a path from here.
    let found = with(|state| {
        let files = state.files().ok_or(ENOENT)?;
        let file = if command && !path.contains('/') { String::from(path) } else { files.resolve(path) };
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
        Ok((file, directory, memory)) => ask_init(&file, &directory, memory, name, arguments, standard, environment),
        Err(code) => {
            standard.close();
            Err(code)
        }
    }
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
    let registry = taaj::duplicate(services.handle(), SEND | DUPLICATE | TRANSFER).ok();
    let Standard { input, output, error, screen } = standard;
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
        (abi::init::IMAGE | abi::init::REPLY, &encoded[..length])
    } else {
        let Some(size) = abi::init::pack(&encoded[..length], environment, &mut packed) else {
            close_given(&given);
            let _ = taaj::close(init);
            return Err(E2BIG);
        };
        (abi::init::IMAGE | abi::init::REPLY | abi::init::ENVIRONMENT, &packed[..size])
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

/// What waitpid says of a child that ended with `code`: its exit code
/// in the second byte, or the signal Unix would have stopped it with in
/// the first - SIGSEGV for a program the kernel killed for breaking a
/// rule, SIGKILL for one stopped on request (Ctrl+C, `kill`), SIGPIPE
/// for one whose reader went away.
fn status_of(code: i64) -> c_int {
    match code {
        abi::KILLED => 11,
        abi::STOPPED => 9,
        abi::NO_READER => 13,
        code => ((code & 0xff) as c_int) << 8,
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
            // SAFETY: room for two.
            if unsafe { crate::pipes::pipe(ends.as_mut_ptr()) } != 0 {
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
                start(&job.path, name, &job.words[1..], true, standard, &job.environment).inspect_err(|&code| {
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
            Ok(process) => {
                let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
                children(|list| list.push((id, process)));
                ids.push(id);
            }
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
static POPENED: Children = Children { list: core::cell::UnsafeCell::new(Vec::new()) };

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
    // SAFETY: room for two.
    if unsafe { crate::pipes::pipe(ends.as_mut_ptr()) } != 0 {
        return -1;
    }
    let (ours, started) = if writing != 0 {
        (ends[1], pipeline(&command, Some(ends[0]), None))
    } else {
        (ends[0], pipeline(&command, None, Some(ends[1])))
    };
    match started {
        Ok(ids) => {
            // SAFETY: as Children.
            let list = unsafe { &mut *POPENED.list.get() };
            list.extend(ids.into_iter().map(|id| (ours, id as Handle)));
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
    // SAFETY: as Children.
    let list = unsafe { &mut *POPENED.list.get() };
    let ids: Vec<c_int> = list.iter().filter(|&&(of, _)| of == fd).map(|&(_, id)| id as c_int).collect();
    list.retain(|&(of, _)| of != fd);
    if ids.is_empty() {
        return crate::descriptors::fail(ECHILD);
    }
    ids.into_iter().fold(0, |_, id| reap(id))
}
