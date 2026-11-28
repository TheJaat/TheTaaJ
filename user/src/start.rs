//! Starting a program from the disk, from a program (K142's `debug`,
//! K60's `ipcbench`): the file read here and handed to init as a memory
//! object (abi::init::IMAGE), as libc's spawn does for C programs. The
//! process handle comes back - with KILL and DEBUG, as the starter's.
//!
//! Needs the services `init` and `files` (the shell gives them to
//! `with programs ...`, and to `debug`).

use crate::files::Files;
use crate::Handle;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// What the started program is given besides its arguments: channels
/// for its output and its input, each optional (its startup handles
/// `startup::STDOUT` and `startup::STDIN`).
#[derive(Default, Clone, Copy)]
pub struct Given {
    pub output: Option<Handle>,
    pub input: Option<Handle>,
}

/// Starts `name` (a file in the root) with `arguments`. The handles in
/// `given` are handed over: they are not this program's any more,
/// whether it started or not.
pub fn start(name: &str, arguments: &[&str], given: Given) -> Result<Handle, String> {
    let close_given = || {
        for handle in [given.output, given.input].into_iter().flatten() {
            let _ = crate::close(handle);
        }
    };
    let services = crate::services().ok_or("given no services");
    let (init, files) = match services {
        Ok(services) => match (services.lookup("init"), services.lookup("files")) {
            (Ok(init), Ok(files)) => (init, Files::new(files)),
            _ => {
                close_given();
                return Err(String::from("not given init and the files"));
            }
        },
        Err(why) => {
            close_given();
            return Err(String::from(why));
        }
    };
    let path = format!("/{name}");
    let image = match files.size(&path) {
        Ok(size) if size > 0 && size <= abi::PROGRAM_IMAGE_MAX => match crate::memory_create(size) {
            Ok(image) if files.read_into(&path, image) == Ok(size) => image,
            Ok(image) => {
                let _ = crate::close(image);
                close_given();
                return Err(String::from("its file could not be read"));
            }
            Err(_) => {
                close_given();
                return Err(String::from("no memory for its file"));
            }
        },
        _ => {
            close_given();
            return Err(String::from("no such program on the disk"));
        }
    };
    let mut encoded = [0u8; abi::MESSAGE_MAX as usize];
    let Some(length) = abi::arguments::encode(arguments.iter().copied(), &mut encoded) else {
        let _ = crate::close(image);
        close_given();
        return Err(String::from("arguments too long"));
    };
    let mut flags = 0u64;
    // The handles in the flags' order (STDOUT, then STDIN), after the
    // image; the reply's last.
    let mut handles: Vec<Handle> = alloc::vec![image];
    if let Some(output) = given.output {
        flags |= abi::startup::STDOUT;
        handles.push(output);
    }
    if let Some(input) = given.input {
        flags |= abi::startup::STDIN;
        handles.push(input);
    }
    let request = abi::init::Run {
        kind: abi::init::IMAGE | abi::init::REPLY,
        flags: flags as u8,
        name,
        directory: "",
        file: &path,
        arguments: &encoded[..length],
    };
    let mut message = [0u8; abi::MESSAGE_MAX as usize];
    let Some(size) = request.encode(&mut message) else {
        for handle in handles {
            let _ = crate::close(handle);
        }
        return Err(String::from("request too long"));
    };
    let Ok((mine, theirs)) = crate::channel_create() else {
        for handle in handles {
            let _ = crate::close(handle);
        }
        return Err(String::from("no channel for the answer"));
    };
    handles.push(theirs);
    if let Err(error) = crate::send(init, &message[..size], &handles) {
        for handle in handles {
            let _ = crate::close(handle);
        }
        let _ = crate::close(mine);
        return Err(format!("init did not take the request: {error:?}"));
    }
    let mut answer = [0u8; 128];
    let mut process = [0u64; 1];
    let reply = crate::receive(mine, &mut answer, &mut process);
    let _ = crate::close(mine);
    match reply {
        Ok((2, 1)) if &answer[..2] == b"ok" => Ok(process[0]),
        Ok((n, _)) => Err(String::from(core::str::from_utf8(&answer[..n]).unwrap_or("error"))),
        Err(error) => Err(format!("no answer from init: {error:?}")),
    }
}
