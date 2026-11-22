//! HTTP/1.1, the server's half: reading a request, and writing the
//! head of a response (RFC 9110, RFC 9112).
//!
//! HTTP is text over a TCP connection. The client sends a request:
//!
//! ```text
//!   GET /index.html HTTP/1.1\r\n        the request line: method, target, version
//!   Host: 127.0.0.1:8080\r\n            headers: a name, a colon, a value
//!   User-Agent: curl/8.5.0\r\n
//!   \r\n                                an empty line: the end of the head
//! ```
//!
//! and the server answers in the same shape - a status line, headers,
//! an empty line - and then the body: the file's bytes.
//!
//! ```text
//!   HTTP/1.1 200 OK\r\n
//!   Content-Type: text/html; charset=utf-8\r\n
//!   Content-Length: 1234\r\n
//!   Connection: close\r\n
//!   \r\n
//!   <!doctype html>...
//! ```
//!
//! Nothing here touches a connection or a file: `head_length` finds
//! where a request's head ends in the bytes read so far, `parse` reads
//! it, `path` turns its target into a file name that cannot leave the
//! site, and `response_head` writes the answer's head. The program that
//! moves the bytes is `user/src/bin/httpd.rs`.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;

/// The longest head this server reads. Larger is refused (431): a
/// client that sends headers for ever must not fill the memory.
pub const HEAD_MAX: usize = 8192;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    Ok = 200,
    BadRequest = 400,
    Forbidden = 403,
    NotFound = 404,
    MethodNotAllowed = 405,
    RequestTimeout = 408,
    HeadersTooLarge = 431,
    InternalError = 500,
    NotImplemented = 501,
    VersionNotSupported = 505,
}

impl Status {
    pub fn code(self) -> u16 {
        self as u16
    }

    /// The words that go after the number. (Nobody reads them; they
    /// are for people looking at the bytes.)
    pub fn reason(self) -> &'static str {
        match self {
            Status::Ok => "OK",
            Status::BadRequest => "Bad Request",
            Status::Forbidden => "Forbidden",
            Status::NotFound => "Not Found",
            Status::MethodNotAllowed => "Method Not Allowed",
            Status::RequestTimeout => "Request Timeout",
            Status::HeadersTooLarge => "Request Header Fields Too Large",
            Status::InternalError => "Internal Server Error",
            Status::NotImplemented => "Not Implemented",
            Status::VersionNotSupported => "HTTP Version Not Supported",
        }
    }

    /// A sentence for the error page.
    fn explanation(self) -> &'static str {
        match self {
            Status::Ok => "Here it is.",
            Status::BadRequest => "The request could not be read.",
            Status::Forbidden => "That is not to be looked at.",
            Status::NotFound => "There is nothing here by that name.",
            Status::MethodNotAllowed => "This server only gives things: GET and HEAD.",
            Status::RequestTimeout => "The request did not arrive in time.",
            Status::HeadersTooLarge => "The request's head is too long.",
            Status::InternalError => "Something went wrong on this side.",
            Status::NotImplemented => "This server does not know that method.",
            Status::VersionNotSupported => "This server speaks HTTP/1.0 and HTTP/1.1.",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Method {
    Get,
    Head,
    /// Known, and not done here: POST, PUT, DELETE and the rest.
    Other,
}

/// A request, read: its parts point into the bytes it was read from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Request<'a> {
    pub method: Method,
    /// As sent: the method's name.
    pub method_name: &'a str,
    /// What is asked for: "/index.html?x=1", as sent.
    pub target: &'a str,
    /// 0 for HTTP/1.0, 1 for HTTP/1.1.
    pub minor: u8,
    /// Everything after the request line, up to the empty line.
    headers: &'a str,
}

impl<'a> Request<'a> {
    /// The value of the first header called `name` (in any case), with
    /// the spaces around it taken off.
    pub fn header(&self, name: &str) -> Option<&'a str> {
        self.headers.split('\n').find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case(name).then(|| value.trim_matches([' ', '\t', '\r']))
        })
    }
}

/// Where the head ends in `bytes` - the length of the head with its
/// empty line - if it has all arrived. A line may end in "\r\n" or,
/// from careless clients, in "\n" alone.
pub fn head_length(bytes: &[u8]) -> Option<usize> {
    let mut at = 0;
    while let Some(found) = bytes[at..].iter().position(|&byte| byte == b'\n') {
        let end = at + found + 1;
        // The line just ended is empty: only "\n" or "\r\n".
        let line = &bytes[at..end];
        if line == b"\n" || line == b"\r\n" {
            return Some(end);
        }
        at = end;
    }
    None
}

/// A token: what a method's name is made of (RFC 9110, 5.6.2).
fn is_token(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte))
}

/// Reads a request's head (as `head_length` found it). The error is
/// the status to answer with.
pub fn parse(head: &[u8]) -> Result<Request<'_>, Status> {
    // Only ASCII is allowed in a head; anything else is no request.
    let text = core::str::from_utf8(head).map_err(|_| Status::BadRequest)?;
    if !text.is_ascii() {
        return Err(Status::BadRequest);
    }
    // Empty lines before the request line are to be skipped (a client
    // may send one after the body of an earlier request).
    let text = text.trim_start_matches(['\r', '\n']);
    let (line, headers) = text.split_once('\n').ok_or(Status::BadRequest)?;
    let line = line.strip_suffix('\r').unwrap_or(line);
    // Exactly three parts, one space between each.
    let mut parts = line.split(' ');
    let (Some(method_name), Some(target), Some(version), None) = (parts.next(), parts.next(), parts.next(), parts.next()) else {
        return Err(Status::BadRequest);
    };
    if !is_token(method_name) || target.is_empty() || target.bytes().any(|byte| byte.is_ascii_control()) {
        return Err(Status::BadRequest);
    }
    let minor = match version {
        "HTTP/1.1" => 1,
        "HTTP/1.0" => 0,
        _ if version.starts_with("HTTP/") => return Err(Status::VersionNotSupported),
        _ => return Err(Status::BadRequest),
    };
    // Each header line must be "name: value" - and a line starting with
    // a space (an old way of folding one header over several lines) is
    // refused, as RFC 9112 asks of a server.
    for header in headers.split('\n').map(|line| line.strip_suffix('\r').unwrap_or(line)).filter(|line| !line.is_empty()) {
        match header.split_once(':') {
            Some((name, _)) if is_token(name) => {}
            _ => return Err(Status::BadRequest),
        }
    }
    let method = match method_name {
        "GET" => Method::Get,
        "HEAD" => Method::Head,
        "POST" | "PUT" | "DELETE" | "PATCH" | "OPTIONS" | "TRACE" | "CONNECT" => Method::Other,
        _ => return Err(Status::NotImplemented),
    };
    let request = Request { method, method_name, target, minor, headers };
    // HTTP/1.1 requires a Host header: a request without one is not
    // well-formed (RFC 9112, 3.2).
    if minor == 1 && request.header("Host").is_none() {
        return Err(Status::BadRequest);
    }
    Ok(request)
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// The file a target names, relative to the site's directory: no '/'
/// in front, the query ("?...") and fragment ("#...") left off, "%xx"
/// turned back into the byte, and the empty path (the site itself) as
/// "". Every way out of the site is refused: ".." as a part, a part
/// that is "." or empty (two slashes), a zero byte or a backslash -
/// whether written plainly or as %2e%2e.
pub fn path(target: &str) -> Result<String, Status> {
    // The absolute form ("http://host/path") is allowed by HTTP/1.1, and
    // the server must take it: the host is left off.
    let target = match target.strip_prefix("http://") {
        Some(rest) => rest.find('/').map_or("/", |at| &rest[at..]),
        None => target,
    };
    let target = target.split(['?', '#']).next().unwrap_or("");
    let rest = target.strip_prefix('/').ok_or(Status::BadRequest)?;
    let mut bytes = Vec::with_capacity(rest.len());
    let mut iter = rest.bytes();
    while let Some(byte) = iter.next() {
        if byte == b'%' {
            let (Some(high), Some(low)) = (iter.next().and_then(hex), iter.next().and_then(hex)) else {
                return Err(Status::BadRequest);
            };
            bytes.push(high << 4 | low);
        } else {
            bytes.push(byte);
        }
    }
    let path = String::from_utf8(bytes).map_err(|_| Status::BadRequest)?;
    if path.contains(['\0', '\\']) || path.bytes().any(|byte| byte.is_ascii_control()) {
        return Err(Status::BadRequest);
    }
    // A slash at the end means "the directory": the parts before it.
    let parts = path.strip_suffix('/').unwrap_or(&path);
    if !parts.is_empty() && parts.split('/').any(|part| part.is_empty() || part == "." || part == "..") {
        return Err(Status::Forbidden);
    }
    Ok(String::from(parts))
}

/// What kind of thing a file is, from its name: the Content-Type a
/// browser needs to know whether to show it, run it or save it.
pub fn content_type(name: &str) -> &'static str {
    let extension = name.rsplit_once('.').map_or("", |(_, extension)| extension);
    match extension.to_ascii_lowercase().as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "txt" | "md" => "text/plain; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "ico" => "image/x-icon",
        "pdf" => "application/pdf",
        // Anything else is bytes: the browser offers to save it.
        _ => "application/octet-stream",
    }
}

/// The head of a response: the status line and the headers, ending in
/// the empty line. `length` is the body's length in bytes - said even
/// when no body follows (to a HEAD request), as HTTP asks. Every
/// connection is closed after one answer, and says so.
pub fn response_head(status: Status, content_type: &str, length: u64) -> String {
    let mut head = String::new();
    // (Writing to a String cannot fail.)
    let _ = write!(head, "HTTP/1.1 {} {}\r\n", status.code(), status.reason());
    let _ = write!(head, "Server: TheTaaJ\r\nContent-Type: {content_type}\r\nContent-Length: {length}\r\n");
    if status == Status::MethodNotAllowed {
        head.push_str("Allow: GET, HEAD\r\n");
    }
    head.push_str("Connection: close\r\n\r\n");
    head
}

/// A small page saying what went wrong.
pub fn error_page(status: Status) -> String {
    let mut page = String::new();
    let _ = write!(
        page,
        "<!doctype html>\n<html><head><meta charset=\"utf-8\"><title>{code} {reason}</title></head>\n\
         <body><h1>{code} {reason}</h1><p>{explanation}</p><hr><p>TheTaaJ</p></body></html>\n",
        code = status.code(),
        reason = status.reason(),
        explanation = status.explanation()
    );
    page
}

#[cfg(test)]
mod tests {
    use super::*;

    const CURL: &[u8] = b"GET /index.html HTTP/1.1\r\nHost: 127.0.0.1:8080\r\nUser-Agent: curl/8.5.0\r\nAccept: */*\r\n\r\n";

    #[test]
    fn the_end_of_a_head_is_an_empty_line() {
        assert_eq!(head_length(CURL), Some(CURL.len()));
        // More after it (a body, or the next request): the head only.
        let mut more = CURL.to_vec();
        more.extend_from_slice(b"GET / HTTP/1.1\r\n");
        assert_eq!(head_length(&more), Some(CURL.len()));
        // Not all there yet, however much has come.
        for cut in 0..CURL.len() {
            assert_eq!(head_length(&CURL[..cut]), None, "{cut}");
        }
        assert_eq!(head_length(b"GET / HTTP/1.0\n\n"), Some(16));
        assert_eq!(head_length(b"GET / HTTP/1.0\r\n\n"), Some(17));
        assert_eq!(head_length(b""), None);
        assert_eq!(head_length(b"\r\n"), Some(2));
    }

    #[test]
    fn a_request_from_curl_is_read() {
        let request = parse(CURL).unwrap();
        assert_eq!((request.method, request.method_name, request.target, request.minor), (Method::Get, "GET", "/index.html", 1));
        assert_eq!(request.header("host"), Some("127.0.0.1:8080"));
        assert_eq!(request.header("USER-AGENT"), Some("curl/8.5.0"));
        assert_eq!(request.header("Accept"), Some("*/*"));
        assert_eq!(request.header("Cookie"), None);
        let request = parse(b"HEAD / HTTP/1.0\n\n").unwrap();
        assert_eq!((request.method, request.minor), (Method::Head, 0));
        assert_eq!(parse(b"\r\n\r\nGET / HTTP/1.1\r\nHost: x\r\n\r\n").map(|request| request.target), Ok("/"));
        assert_eq!(parse(b"GET / HTTP/1.1\r\nHost:x\r\nX-Empty:\r\n\r\n").map(|request| request.header("x-empty")), Ok(Some("")));
    }

    #[test]
    fn what_is_not_a_request_is_said_to_be_so() {
        for (head, status) in [
            (&b"GET / HTTP/1.1\r\n\r\n"[..], Status::BadRequest), // no Host
            (b"GET /  HTTP/1.1\r\nHost: x\r\n\r\n", Status::BadRequest),
            (b"GET / HTTP/1.1 extra\r\nHost: x\r\n\r\n", Status::BadRequest),
            (b"GET /\r\n\r\n", Status::BadRequest),
            (b"GET / HTTP/2.0\r\nHost: x\r\n\r\n", Status::VersionNotSupported),
            (b"GET / HTTP/1.1\r\nHost x\r\n\r\n", Status::BadRequest),
            (b"GET / HTTP/1.1\r\nHost: x\r\n folded\r\n\r\n", Status::BadRequest),
            (b"GET / HTTP/1.1\r\nBad Name: x\r\nHost: x\r\n\r\n", Status::BadRequest),
            (b"G(T / HTTP/1.1\r\nHost: x\r\n\r\n", Status::BadRequest),
            (b"BREW / HTTP/1.1\r\nHost: x\r\n\r\n", Status::NotImplemented),
            (b"GET /\xc3\xa9 HTTP/1.1\r\nHost: x\r\n\r\n", Status::BadRequest),
            (b"GET /\xff HTTP/1.1\r\nHost: x\r\n\r\n", Status::BadRequest),
            (b"GET /a\tb HTTP/1.1\r\nHost: x\r\n\r\n", Status::BadRequest),
            (b"\x16\x03\x01\x02\x00\x01\x00\x01\xfc\x03\x03\r\n\r\n", Status::BadRequest), // a TLS hello
            (b"\r\n", Status::BadRequest),
        ] {
            assert_eq!(parse(head).map(|request| request.target), Err(status), "{:?}", core::str::from_utf8(head));
        }
        // Methods that exist but are not done here: read, and answered
        // 405 by the program.
        assert_eq!(parse(b"POST /form HTTP/1.1\r\nHost: x\r\n\r\n").map(|request| request.method), Ok(Method::Other));
        assert_eq!(parse(b"DELETE / HTTP/1.0\r\n\r\n").map(|request| request.method), Ok(Method::Other));
    }

    #[test]
    fn a_target_becomes_a_name_inside_the_site() {
        for (target, name) in [
            ("/", ""),
            ("/index.html", "index.html"),
            ("/docs/", "docs"),
            ("/docs/a.txt", "docs/a.txt"),
            ("/a%20file.txt", "a file.txt"),
            ("/%41%62c", "Abc"),
            ("/page?x=1&y=/../..", "page"),
            ("/page#part", "page"),
            ("/?", ""),
            ("http://127.0.0.1:8080/index.html", "index.html"),
            ("http://127.0.0.1:8080", ""),
            ("/%C3%A9t%C3%A9", "été"),
            ("/..x/x..", "..x/x.."),
        ] {
            assert_eq!(path(target).as_deref(), Ok(name), "{target}");
        }
        for (target, status) in [
            ("/..", Status::Forbidden),
            ("/../welcome", Status::Forbidden),
            ("/docs/../../welcome", Status::Forbidden),
            ("/%2e%2e/welcome", Status::Forbidden),
            ("/%2E%2e%2fwelcome", Status::Forbidden),
            ("/./index.html", Status::Forbidden),
            ("//index.html", Status::Forbidden),
            ("/docs//a", Status::Forbidden),
            ("/a%00b", Status::BadRequest),
            ("/a%5c..%5cb", Status::BadRequest),
            ("/a%0ab", Status::BadRequest),
            ("/%zz", Status::BadRequest),
            ("/%4", Status::BadRequest),
            ("/%ff", Status::BadRequest),
            ("index.html", Status::BadRequest),
            ("*", Status::BadRequest),
        ] {
            assert_eq!(path(target), Err(status), "{target}");
        }
    }

    #[test]
    fn a_file_says_what_it_is_by_its_name() {
        assert_eq!(content_type("index.html"), "text/html; charset=utf-8");
        assert_eq!(content_type("INDEX.HTM"), "text/html; charset=utf-8");
        assert_eq!(content_type("style.css"), "text/css; charset=utf-8");
        assert_eq!(content_type("docs/notes.txt"), "text/plain; charset=utf-8");
        assert_eq!(content_type("logo.png"), "image/png");
        assert_eq!(content_type("a.tar.gz"), "application/octet-stream");
        assert_eq!(content_type("README"), "application/octet-stream");
        assert_eq!(content_type(".css"), "text/css; charset=utf-8");
    }

    #[test]
    fn a_response_head_is_written_as_http_asks() {
        assert_eq!(
            response_head(Status::Ok, "text/html; charset=utf-8", 1234),
            "HTTP/1.1 200 OK\r\nServer: TheTaaJ\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: 1234\r\nConnection: close\r\n\r\n"
        );
        let refused = response_head(Status::MethodNotAllowed, "text/html; charset=utf-8", 9);
        assert!(refused.starts_with("HTTP/1.1 405 Method Not Allowed\r\n"));
        assert!(refused.contains("\r\nAllow: GET, HEAD\r\n"));
        // What the program writes is what a request parser reads as a
        // head: it ends where `head_length` says.
        assert_eq!(head_length(refused.as_bytes()), Some(refused.len()));
        let page = error_page(Status::NotFound);
        assert!(page.contains("<title>404 Not Found</title>") && page.contains("There is nothing here by that name."));
        // Every status has its own words.
        let all = [
            Status::Ok,
            Status::BadRequest,
            Status::Forbidden,
            Status::NotFound,
            Status::MethodNotAllowed,
            Status::RequestTimeout,
            Status::HeadersTooLarge,
            Status::InternalError,
            Status::NotImplemented,
            Status::VersionNotSupported,
        ];
        for (n, status) in all.iter().enumerate() {
            assert!(all[..n].iter().all(|other| other.reason() != status.reason() && other.explanation() != status.explanation()));
        }
    }
}
