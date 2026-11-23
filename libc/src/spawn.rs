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
//!     by both;
//!   - this program's services.
//!
//! Not file actions (redirections) or attributes: posix_spawn fails
//! with ENOSYS if any were asked for. There is no environment (getenv
//! never finds anything), so `envp` is not used.
//!
//! A process ID here is this program's own number for a child, from 2
//! up, good for waitpid: TheTaaJ names a process by a handle, not by a
//! number everyone shares.

use crate::descriptors::with;
use crate::errno::{E2BIG, ECHILD, EINVAL, EIO, ENOENT, ENOEXEC, ENOMEM, ENOSYS, EPERM};
use alloc::string::String;
use alloc::vec::Vec;
use core::ffi::{c_char, c_int, CStr};
use core::sync::atomic::{AtomicI32, Ordering};
use taaj::Handle;

/// `posix_spawn_file_actions_t` (spawn.h): how many actions were added,
/// and room the C type keeps for later.
#[repr(C)]
pub struct FileActions {
    actions: c_int,
    _reserved: [c_int; 7],
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
    unsafe { actions.write(FileActions { actions: 0, _reserved: [0; 7] }) };
    0
}

/// `posix_spawn_file_actions_destroy(actions)`: nothing to give back. 0.
#[no_mangle]
pub extern "C" fn posix_spawn_file_actions_destroy(_actions: *mut FileActions) -> c_int {
    0
}

/// `posix_spawn_file_actions_addopen(...)`: a new program cannot be
/// given files opened for it yet (see the top): ENOSYS.
#[no_mangle]
pub extern "C" fn posix_spawn_file_actions_addopen(
    _actions: *mut FileActions,
    _descriptor: c_int,
    _path: *const c_char,
    _flags: c_int,
    _mode: c_int,
) -> c_int {
    ENOSYS
}

/// `posix_spawn_file_actions_addclose(...)`: ENOSYS, as addopen.
#[no_mangle]
pub extern "C" fn posix_spawn_file_actions_addclose(_actions: *mut FileActions, _descriptor: c_int) -> c_int {
    ENOSYS
}

/// `posix_spawn_file_actions_adddup2(...)`: ENOSYS, as addopen.
#[no_mangle]
pub extern "C" fn posix_spawn_file_actions_adddup2(_actions: *mut FileActions, _from: c_int, _to: c_int) -> c_int {
    ENOSYS
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
    _envp: *const *const c_char,
) -> c_int {
    // SAFETY: the caller's promise.
    unsafe { spawn(pid, path, actions, attributes, argv, false) }
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
    _envp: *const *const c_char,
) -> c_int {
    // SAFETY: the caller's promise.
    unsafe { spawn(pid, file, actions, attributes, argv, true) }
}

/// # Safety
/// As posix_spawn.
unsafe fn spawn(
    pid: *mut c_int,
    path: *const c_char,
    actions: *const FileActions,
    attributes: *const Attributes,
    argv: *const *const c_char,
    command: bool,
) -> c_int {
    // SAFETY: the caller's promises.
    let asked_more = unsafe { (!actions.is_null() && (*actions).actions != 0) || (!attributes.is_null() && (*attributes).flags != 0) };
    if asked_more {
        return ENOSYS;
    }
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
    let name = words.first().cloned().unwrap_or_else(|| path.clone());
    match start(&path, &name, words.get(1..).unwrap_or(&[]), command) {
        Ok(process) => {
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            children(|list| list.push((id, process)));
            if !pid.is_null() {
                // SAFETY: the caller's promise.
                unsafe { pid.write(id) };
            }
            0
        }
        Err(code) => code,
    }
}

/// Reads the program and asks init to start it: the new process, or an
/// error number.
fn start(path: &str, name: &str, arguments: &[String], command: bool) -> Result<Handle, c_int> {
    // The file, as a path from the root ("" is the root): a command is
    // a file in the root; anything else is a path from here.
    let (file, directory, memory) = with(|state| {
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
    })?;
    ask_init(&file, &directory, memory, name, arguments)
}

/// The request to init, with the image and the handles handed down
/// (abi::init); init's reply comes on a channel of this request's own
/// (abi::init::REPLY), so that it is not some other program's.
fn ask_init(file: &str, directory: &str, image: Handle, name: &str, arguments: &[String]) -> Result<Handle, c_int> {
    use abi::startup::{SCREEN, SERVICES, STDERR, STDOUT};
    let mut given = alloc::vec![image];
    let close_given = |given: &[Handle]| {
        for &handle in given {
            let _ = taaj::close(handle);
        }
    };
    let Some(services) = taaj::services() else {
        close_given(&given);
        return Err(EPERM);
    };
    let Ok(init) = services.lookup("init") else {
        close_given(&given);
        return Err(EPERM); // not given the right to start programs
    };
    // A copy to hand down, which the new program may hand down again.
    use taaj::rights::{DUPLICATE, SEND, TRANSFER};
    let pass = |handle: Option<Handle>| handle.and_then(|h| taaj::duplicate(h, SEND | DUPLICATE | TRANSFER).ok());
    let mut flags = 0u8;
    // In the order of their bits: stdout, services, stderr.
    if let Some(stdout) = pass(taaj::stdout()) {
        given.push(stdout);
        flags |= STDOUT as u8;
        if taaj::output_is_screen() {
            flags |= SCREEN as u8;
        }
    }
    if let Some(registry) = pass(Some(services.handle())) {
        given.push(registry);
        flags |= SERVICES as u8;
    }
    if let Some(stderr) = pass(taaj::stderr()) {
        given.push(stderr);
        flags |= STDERR as u8;
    }
    let mut encoded = [0u8; abi::MESSAGE_MAX as usize];
    let Some(length) = abi::arguments::encode(arguments.iter().map(String::as_str), &mut encoded) else {
        close_given(&given);
        let _ = taaj::close(init);
        return Err(E2BIG);
    };
    let whole = alloc::format!("/{file}");
    let request = abi::init::Run {
        kind: abi::init::IMAGE | abi::init::REPLY,
        flags,
        name,
        directory,
        file: &whole,
        arguments: &encoded[..length],
    };
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
/// There is no shell for a C program here, so the command is split into
/// words as sh would split it (quotes and backslashes work), and the
/// first is started as posix_spawnp starts a name: a bare name is the
/// file of that name in the root. A command that needs a shell - a pipe,
/// a redirection, `;`, `&&`, `$`, a wildcard, `VAR=x cmd` (`words_of`) -
/// is not run: -1, with ENOSYS, and a line on standard error saying
/// why. A program that cannot be started gives status 127, as sh's "not
/// found" does, with a line saying why; one that may not be started (no
/// "init" service) -1 with EPERM. An empty command does nothing, 0.
/// `system(NULL)` says 1: commands can be run, in this way.
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
    let words = match crate::words::words_of(&command) {
        Ok(words) => words,
        Err(why) => {
            let why = match why {
                crate::words::Unshelled::Needs(c) => alloc::format!("'{c}' needs a shell"),
                crate::words::Unshelled::Unfinished => String::from("a quote or backslash is not finished"),
                crate::words::Unshelled::Assignment => String::from("setting a variable for a command needs a shell"),
            };
            complain(&alloc::format!("system: cannot run \"{command}\": {why}, and there is none here\n"));
            return crate::descriptors::fail(ENOSYS);
        }
    };
    let Some(name) = words.first() else { return 0 };
    let process = match start(name, name, &words[1..], true) {
        Ok(process) => process,
        Err(EPERM) => return crate::descriptors::fail(EPERM),
        Err(code) => {
            let why = match code {
                ENOENT => "not found",
                ENOEXEC => "not a program",
                ENOMEM => "no memory for it",
                _ => "it could not be read",
            };
            complain(&alloc::format!("system: {name}: {why}\n"));
            return 127 << 8;
        }
    };
    let status = match taaj::wait_any(&[process]) {
        Ok((_, code)) => status_of(code),
        Err(_) => 127 << 8,
    };
    let _ = taaj::close(process);
    status
}
