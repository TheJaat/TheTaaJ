//! Sockets: socket, connect, bind, listen, accept, send, recv,
//! shutdown, setsockopt, getsockname (sys/socket.h).
//!
//! The Berkeley socket calls are how a C program has used the network
//! since 1983, and most network code ever written is in them. Here
//! each is a few lines in front of what the Rust programs use
//! (`taaj::net`): a socket is a descriptor in the same table as the
//! files (descriptors.rs), and stands for one of three things:
//!
//!   socket()          Fresh: nothing yet, perhaps a port (bind)
//!   connect()         Connected: a `TcpStream`
//!   listen()          Listening: a `TcpListener`, which `accept`
//!                     asks for the next connection
//!
//! read and write on a connected socket are the stream's; so are send
//! and recv, which only add flags (none are known). A socket is TCP
//! over IPv4 only, for now: SOCK_DGRAM and other families are refused
//! with the errno that says so.

use core::ffi::{c_int, c_uint, c_void};

use alloc::boxed::Box;

use taaj::net::{Ipv4, NetError, TcpListener, TcpStream};
use taaj::Error;

use crate::descriptors::{fail, with, Open, State};
use crate::errno::*;

const AF_INET: c_int = 2;
const SOCK_STREAM: c_int = 1;
const IPPROTO_TCP: c_int = 6;
const SOL_SOCKET: c_int = 1;
const SO_REUSEADDR: c_int = 2;
const SHUT_RD: c_int = 0;
const SHUT_WR: c_int = 1;
const SHUT_RDWR: c_int = 2;
/// The size of a struct sockaddr_in (netinet/in.h).
const SOCKADDR_IN: usize = 16;

// The whole of it is boxed in the descriptor table (`Open::Socket`):
// one allocation per socket, whichever kind it is.
#[allow(clippy::large_enum_variant)]
pub enum Socket {
    Fresh { port: u16 },
    Connected { stream: TcpStream, shut: bool },
    Listening(TcpListener),
}

/// The network service's reasons, as errno numbers.
fn reason(error: NetError) -> c_int {
    use abi::net as status;
    match error {
        NetError::Refused(status::REFUSED) => ECONNREFUSED,
        NetError::Refused(status::RESET) => ECONNRESET,
        NetError::Refused(status::TIMED_OUT) => ETIMEDOUT,
        NetError::Refused(status::UNREACHABLE) => EHOSTUNREACH,
        NetError::Refused(status::NO_ROUTE) => ENETUNREACH,
        NetError::Refused(status::IN_USE) => EADDRINUSE,
        NetError::Refused(status::NO_ROOM) => ENOBUFS,
        NetError::Refused(_) => EIO,
        NetError::Unreachable(Error::TimedOut) => ETIMEDOUT,
        NetError::Unreachable(_) => EIO,
    }
}

impl Socket {
    /// `read` on this socket.
    pub fn read(&mut self, out: &mut [u8]) -> isize {
        match self {
            Socket::Connected { stream, .. } => match stream.read(out) {
                Ok(count) => count as isize,
                Err(error) => fail(reason(error)),
            },
            _ => fail(ENOTCONN),
        }
    }

    /// `write` on this socket: all of `bytes`, or -1.
    pub fn write(&mut self, bytes: &[u8]) -> isize {
        match self {
            Socket::Connected { shut: true, .. } => fail(EPIPE),
            Socket::Connected { stream, .. } => match stream.write(bytes) {
                Ok(()) => bytes.len() as isize,
                // Cut off, or ended: on Unix that is EPIPE too.
                Err(NetError::Refused(abi::net::RESET)) => fail(EPIPE),
                Err(error) => fail(reason(error)),
            },
            _ => fail(ENOTCONN),
        }
    }
}

/// The socket behind descriptor `fd`, or why there is none.
fn socket_of(state: &mut State, fd: c_int) -> Result<&mut Socket, c_int> {
    match usize::try_from(fd).ok().and_then(|fd| state.table.get_mut(fd)).and_then(Option::as_mut) {
        Some(Open::Socket(socket)) => Ok(socket),
        Some(_) => Err(ENOTSOCK),
        None => Err(EBADF),
    }
}

/// An IPv4 address and port from C's struct sockaddr_in.
///
/// # Safety
/// `address` must be null or point to `length` readable bytes.
unsafe fn address_from(address: *const c_void, length: c_uint) -> Result<(Ipv4, u16), c_int> {
    if address.is_null() {
        return Err(EFAULT);
    }
    if (length as usize) < SOCKADDR_IN {
        return Err(EINVAL);
    }
    // SAFETY: not null, and the caller's promise of `length` bytes,
    // which is at least 16.
    let bytes = unsafe { core::slice::from_raw_parts(address.cast::<u8>(), SOCKADDR_IN) };
    // The family is in this machine's order; the port and the address
    // in the network's.
    if c_int::from(u16::from_ne_bytes([bytes[0], bytes[1]])) != AF_INET {
        return Err(EAFNOSUPPORT);
    }
    Ok((Ipv4([bytes[4], bytes[5], bytes[6], bytes[7]]), u16::from_be_bytes([bytes[2], bytes[3]])))
}

/// Writes an address and port as a struct sockaddr_in to `address`,
/// cut to the room `*length` says there is, and sets `*length` to the
/// size of the whole - as C asks.
///
/// # Safety
/// Both must be null, or `length` point to a socklen_t and `address`
/// to that many writable bytes.
unsafe fn address_to(address: *mut c_void, length: *mut c_uint, ip: Ipv4, port: u16) {
    if address.is_null() || length.is_null() {
        return;
    }
    let mut bytes = [0u8; SOCKADDR_IN];
    bytes[..2].copy_from_slice(&(AF_INET as u16).to_ne_bytes());
    bytes[2..4].copy_from_slice(&port.to_be_bytes());
    bytes[4..8].copy_from_slice(&ip.0);
    // SAFETY: the caller's promise: `length` points to a socklen_t,
    // and `address` to that many bytes, of which no more are written.
    unsafe {
        let room = (*length as usize).min(SOCKADDR_IN);
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), address.cast::<u8>(), room);
        *length = SOCKADDR_IN as c_uint;
    }
}

/// The network service: asked for once, at the first socket.
fn net(state: &mut State) -> Result<&taaj::net::Net, c_int> {
    // EACCES, as for files: the program was not given it.
    state.net.get_or_insert_with(taaj::net::Net::find).as_ref().ok_or(EACCES)
}

/// `socket(family, type, protocol)`: a new descriptor that is a socket,
/// connected to nothing yet.
#[no_mangle]
pub extern "C" fn socket(family: c_int, kind: c_int, protocol: c_int) -> c_int {
    if family != AF_INET {
        return fail(EAFNOSUPPORT);
    }
    if kind != SOCK_STREAM || (protocol != 0 && protocol != IPPROTO_TCP) {
        return fail(EPROTONOSUPPORT);
    }
    with(|state| {
        if let Err(code) = net(state) {
            return fail(code);
        }
        let Some(slot) = state.free_slot() else { return fail(EMFILE) };
        state.table[slot] = Some(Open::Socket(Box::new(Socket::Fresh { port: 0 })));
        slot as c_int
    })
}

/// `connect(fd, address, length)`: makes a connection to the address,
/// and waits until it is made or known not to be.
///
/// # Safety
/// `address` must be null or point to `length` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn connect(fd: c_int, address: *const c_void, length: c_uint) -> c_int {
    // SAFETY: the caller's promise.
    let (to, port) = match unsafe { address_from(address, length) } {
        Ok(found) => found,
        Err(code) => return fail(code),
    };
    with(|state| {
        match socket_of(state, fd) {
            Ok(Socket::Fresh { port: 0 }) => {}
            // (A connection from a port chosen with bind is not
            // something the service can make yet.)
            Ok(Socket::Fresh { .. }) => return fail(EOPNOTSUPP),
            Ok(Socket::Connected { .. }) => return fail(EISCONN),
            Ok(Socket::Listening(_)) => return fail(EOPNOTSUPP),
            Err(code) => return fail(code),
        }
        if port == 0 {
            return fail(EINVAL);
        }
        let stream = match net(state).map(|net| net.tcp_connect(to, port)) {
            Ok(Ok(stream)) => stream,
            Ok(Err(error)) => return fail(reason(error)),
            Err(code) => return fail(code),
        };
        if let Ok(socket) = socket_of(state, fd) {
            *socket = Socket::Connected { stream, shut: false };
        }
        0
    })
}

/// `bind(fd, address, length)`: the port the socket is to listen on.
/// The address part must be INADDR_ANY or this machine's: a machine
/// with one card has nothing else to choose.
///
/// # Safety
/// `address` must be null or point to `length` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn bind(fd: c_int, address: *const c_void, length: c_uint) -> c_int {
    // SAFETY: the caller's promise.
    let (_, port) = match unsafe { address_from(address, length) } {
        Ok(found) => found,
        Err(code) => return fail(code),
    };
    with(|state| match socket_of(state, fd) {
        Ok(Socket::Fresh { port: bound @ 0 }) => {
            *bound = port;
            0
        }
        Ok(_) => fail(EINVAL),
        Err(code) => fail(code),
    })
}

/// `listen(fd, backlog)`: connections to the bound port are taken from
/// now on. (Unbound: a free port of the service's choosing, which
/// getsockname tells.)
#[no_mangle]
pub extern "C" fn listen(fd: c_int, _backlog: c_int) -> c_int {
    with(|state| {
        let port = match socket_of(state, fd) {
            Ok(Socket::Fresh { port }) => *port,
            Ok(Socket::Listening(_)) => return 0,
            Ok(Socket::Connected { .. }) => return fail(EISCONN),
            Err(code) => return fail(code),
        };
        let listener = match net(state).map(|net| net.tcp_listen(port)) {
            Ok(Ok(listener)) => listener,
            Ok(Err(error)) => return fail(reason(error)),
            Err(code) => return fail(code),
        };
        if let Ok(socket) = socket_of(state, fd) {
            *socket = Socket::Listening(listener);
        }
        0
    })
}

/// `accept(fd, address, length)`: waits for the next connection to a
/// listening socket; a new descriptor for it.
///
/// # Safety
/// `address` and `length` must be null, or as `getsockname` asks.
#[no_mangle]
pub unsafe extern "C" fn accept(fd: c_int, address: *mut c_void, length: *mut c_uint) -> c_int {
    with(|state| {
        if state.free_slot().is_none() {
            return fail(EMFILE);
        }
        let stream = match socket_of(state, fd) {
            Ok(Socket::Listening(listener)) => loop {
                match listener.accept_until(u64::MAX) {
                    Ok(Some(stream)) => break stream,
                    Ok(None) => {}
                    Err(error) => return fail(reason(error)),
                }
            },
            Ok(_) => return fail(EINVAL),
            Err(code) => return fail(code),
        };
        let (from, from_port) = stream.peer();
        // SAFETY: the caller's promise.
        unsafe { address_to(address, length, from, from_port) };
        let Some(slot) = state.free_slot() else { return fail(EMFILE) };
        state.table[slot] = Some(Open::Socket(Box::new(Socket::Connected { stream, shut: false })));
        slot as c_int
    })
}

/// `send(fd, buffer, count, flags)`: `write`, with no flags known.
///
/// # Safety
/// As `write`'s.
#[no_mangle]
pub unsafe extern "C" fn send(fd: c_int, buffer: *const c_void, count: usize, flags: c_int) -> isize {
    if flags != 0 {
        return fail(EOPNOTSUPP);
    }
    // SAFETY: the caller's promise, passed on.
    unsafe { crate::descriptors::write(fd, buffer, count) }
}

/// `recv(fd, buffer, count, flags)`: `read`, with no flags known.
///
/// # Safety
/// As `read`'s.
#[no_mangle]
pub unsafe extern "C" fn recv(fd: c_int, buffer: *mut c_void, count: usize, flags: c_int) -> isize {
    if flags != 0 {
        return fail(EOPNOTSUPP);
    }
    // SAFETY: the caller's promise, passed on.
    unsafe { crate::descriptors::read(fd, buffer, count) }
}

/// `shutdown(fd, how)`: SHUT_WR (or SHUT_RDWR) - nothing more will be
/// written; the other side reads to the end. SHUT_RD changes nothing
/// here: what arrives is kept until it is read or the socket closed.
#[no_mangle]
pub extern "C" fn shutdown(fd: c_int, how: c_int) -> c_int {
    with(|state| match (socket_of(state, fd), how) {
        (Ok(Socket::Connected { .. }), SHUT_RD) => 0,
        (Ok(Socket::Connected { shut: true, .. }), SHUT_WR | SHUT_RDWR) => 0,
        (Ok(Socket::Connected { stream, shut }), SHUT_WR | SHUT_RDWR) => match stream.shutdown() {
            Ok(()) => {
                *shut = true;
                0
            }
            Err(error) => fail(reason(error)),
        },
        (Ok(Socket::Connected { .. }), _) => fail(EINVAL),
        (Ok(_), _) => fail(ENOTCONN),
        (Err(code), _) => fail(code),
    })
}

/// `setsockopt(fd, level, option, value, length)`: SO_REUSEADDR is
/// taken and changes nothing - a port is free again as soon as nothing
/// uses it. Nothing else is known.
#[no_mangle]
pub extern "C" fn setsockopt(fd: c_int, level: c_int, option: c_int, _value: *const c_void, _length: c_uint) -> c_int {
    with(|state| match socket_of(state, fd) {
        Ok(_) if level == SOL_SOCKET && option == SO_REUSEADDR => 0,
        Ok(_) => fail(ENOPROTOOPT),
        Err(code) => fail(code),
    })
}

/// `getsockname(fd, address, length)`: the port of this machine the
/// socket uses. (The address part is left 0.0.0.0, "this machine":
/// the library does not ask the service which address that is.)
///
/// # Safety
/// `length` must point to a socklen_t and `address` to that many
/// writable bytes.
#[no_mangle]
pub unsafe extern "C" fn getsockname(fd: c_int, address: *mut c_void, length: *mut c_uint) -> c_int {
    if address.is_null() || length.is_null() {
        return fail(EFAULT);
    }
    with(|state| {
        let port = match socket_of(state, fd) {
            Ok(Socket::Fresh { port }) => *port,
            Ok(Socket::Connected { stream, .. }) => stream.local_port(),
            Ok(Socket::Listening(listener)) => listener.port(),
            Err(code) => return fail(code),
        };
        // SAFETY: the caller's promise.
        unsafe { address_to(address, length, Ipv4::NONE, port) };
        0
    })
}

