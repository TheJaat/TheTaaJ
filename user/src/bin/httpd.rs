//! httpd: a web server - a program on the disk.
//!
//!   httpd [PORT [DIRECTORY [COUNT]]]
//!
//! Listens on PORT (80, the port set aside for the web, if not said)
//! and gives out the files under DIRECTORY (/www if not said): a
//! request for "/" or for a directory is answered with its
//! index.html. Connections are served one at a time, one request each,
//! and closed; others wait their turn in the listener's backlog. After
//! COUNT connections it ends; without a count it goes on until stopped
//! (Ctrl+C, or `kill`).
//!
//! It is given the file service's reading channel only: whatever a
//! request says, nothing on the disk can be changed through it. And a
//! request's path is checked (net::http::path) before it is put after
//! the directory, so no request reaches a file outside it.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec;

use net::http::{self, Method, Status};
use taaj::files::{FileError, Files, Kind, Status as FileStatus};
use taaj::net::{Net, TcpStream};
use taaj::{eprintln, println};

taaj::program!(main);

/// How long a client has to send its whole request: 10 seconds (of 100
/// Hz ticks). One that is slower - or that connects and says nothing -
/// would otherwise hold the server, and everyone waiting behind it,
/// for ever.
const REQUEST_WAIT: u64 = 1000;

/// After the answer, how long to go on reading what the client still
/// sends (and throwing it away) before closing: 2 seconds. Closing a
/// connection with unread bytes in it makes TCP send a reset, and a
/// reset can make the client throw away the end of the answer it has
/// not read yet.
const LINGER: u64 = 200;

/// What was answered, for the log line.
struct Served {
    status: Status,
    /// Body bytes written.
    bytes: u64,
}

/// Reads the request's head: the bytes up to the empty line. An error
/// is the status to answer with - None if there is no one to answer
/// (the client went away, or never said anything).
fn read_head(stream: &mut TcpStream, head: &mut [u8]) -> Result<usize, Option<Status>> {
    let deadline = taaj::ticks() + REQUEST_WAIT;
    let mut length = 0;
    loop {
        if let Some(end) = http::head_length(&head[..length]) {
            return Ok(end);
        }
        if length == head.len() {
            return Err(Some(Status::HeadersTooLarge));
        }
        match stream.read_until(&mut head[length..], deadline) {
            Ok(Some(0)) | Err(_) => return Err(if length == 0 { None } else { Some(Status::BadRequest) }),
            Ok(Some(count)) => length += count,
            Ok(None) => return Err(if length == 0 { None } else { Some(Status::RequestTimeout) }),
        }
    }
}

/// The file a request's path names under the site's directory, and its
/// size. A directory is answered with the index.html inside it.
fn find(files: &Files, site: &str, name: &str) -> Result<(String, u64), Status> {
    let full = if name.is_empty() { String::from(site) } else { format!("{site}/{name}") };
    let refused = |error: FileError| match error {
        FileError::Refused(FileStatus::NotFound | FileStatus::BadName | FileStatus::NotDirectory) => Status::NotFound,
        _ => Status::InternalError,
    };
    match files.stat(&full).map_err(refused)? {
        (Kind::File, size) => Ok((full, size)),
        (Kind::Directory, _) => {
            let index = format!("{full}/index.html");
            match files.stat(&index).map_err(refused)? {
                (Kind::File, size) => Ok((index, size)),
                (Kind::Directory, _) => Err(Status::NotFound),
            }
        }
    }
}

/// Writes a small page saying what went wrong (only its head, to HEAD).
fn refuse(stream: &mut TcpStream, status: Status, body: bool) -> Served {
    let page = http::error_page(status);
    let head = http::response_head(status, "text/html; charset=utf-8", page.len() as u64);
    let mut answer = head;
    if body {
        answer.push_str(&page);
    }
    let bytes = if stream.write(answer.as_bytes()).is_ok() && body { page.len() as u64 } else { 0 };
    Served { status, bytes }
}

/// Writes the file: the head, then its bytes a message at a time, as
/// the file service gives them.
fn give(stream: &mut TcpStream, files: &Files, file: &str, size: u64, body: bool) -> Served {
    let head = http::response_head(Status::Ok, http::content_type(file), size);
    let mut served = Served { status: Status::Ok, bytes: 0 };
    if stream.write(head.as_bytes()).is_err() || !body {
        return served;
    }
    while served.bytes < size {
        let mut written = Ok(());
        let read = files.read_from(file, served.bytes, |piece| {
            // Never more than the head promised, even if the file grew
            // in the meantime: the client counts on Content-Length.
            let piece = &piece[..piece.len().min((size - served.bytes) as usize)];
            written = stream.write(piece);
            served.bytes += piece.len() as u64;
        });
        // The head has gone out, so an error can no longer be said in
        // the status: the connection ends short of Content-Length, and
        // that is how the client learns the answer is not whole.
        match (read, written) {
            (Ok(0), _) | (Err(_), _) | (_, Err(_)) => break,
            _ => {}
        }
    }
    served
}

/// Serves one connection: one request, one answer.
fn serve(stream: &mut TcpStream, files: &Files, site: &str) -> Option<(String, Served)> {
    let mut head = vec![0u8; http::HEAD_MAX];
    let length = match read_head(stream, &mut head) {
        Ok(length) => length,
        Err(None) => return None,
        Err(Some(status)) => return Some((String::from("-"), refuse(stream, status, true))),
    };
    let request = match http::parse(&head[..length]) {
        Ok(request) => request,
        Err(status) => return Some((String::from("-"), refuse(stream, status, true))),
    };
    let what = format!("{} {}", request.method_name, request.target);
    let body = request.method != Method::Head;
    if request.method == Method::Other {
        return Some((what, refuse(stream, Status::MethodNotAllowed, true)));
    }
    let served = match http::path(request.target).and_then(|name| find(files, site, &name)) {
        Ok((file, size)) => give(stream, files, &file, size, body),
        Err(status) => refuse(stream, status, body),
    };
    Some((what, served))
}

/// No more to write; reads what the client still sends, for a while,
/// so that the close that follows is a quiet one.
fn finish(stream: &mut TcpStream) {
    if stream.shutdown().is_err() {
        return;
    }
    let deadline = taaj::ticks() + LINGER;
    let mut rest = [0u8; 512];
    while let Ok(Some(1..)) = stream.read_until(&mut rest, deadline) {}
}

fn main() -> i64 {
    const USAGE: &str = "usage: httpd [PORT [DIRECTORY [COUNT]]]";
    let arguments = taaj::arguments();
    let (Some(port), Some(count)) = (
        arguments.first().map_or(Some(80u16), |text| text.parse().ok()).filter(|&port| port != 0),
        arguments.get(2).map_or(Some(u64::MAX), |text| text.parse().ok()),
    ) else {
        eprintln!("{USAGE}");
        return 2;
    };
    if arguments.len() > 3 {
        eprintln!("{USAGE}");
        return 2;
    }
    let Some(files) = taaj::file_service() else {
        eprintln!("httpd: no files for this program");
        return 1;
    };
    // The site's directory as a whole path from the root, so a name from
    // a request is put after it and nothing else.
    let site = files.resolve(arguments.get(1).map_or("/www", |text| text.as_str()));
    match files.stat(&site) {
        Ok((Kind::Directory, _)) => {}
        Ok((Kind::File, _)) => {
            eprintln!("httpd: /{site}: not a directory");
            return 1;
        }
        Err(error) => {
            eprintln!("httpd: /{site}: {}", error.describe());
            return 1;
        }
    }
    let Some(net) = Net::find() else {
        eprintln!("httpd: no network for this program");
        return 1;
    };
    let mut listener = match net.tcp_listen(port) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("httpd: port {port}: {}", error.describe());
            return 1;
        }
    };
    println!("httpd: serving /{site} on port {port}");
    for _ in 0..count {
        let mut stream = match listener.accept_until(u64::MAX) {
            Ok(Some(stream)) => stream,
            Ok(None) => continue,
            Err(_) => {
                eprintln!("httpd: the network service stopped");
                return 1;
            }
        };
        let (from, _) = stream.peer();
        match serve(&mut stream, &files, &site) {
            Some((what, served)) => {
                finish(&mut stream);
                println!("httpd: {from} \"{what}\" {} {} bytes", served.status.code(), served.bytes);
            }
            None => println!("httpd: {from} sent no request"),
        }
    }
    0
}
