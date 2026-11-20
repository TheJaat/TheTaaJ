//! Asking the registry for services, by name.
//!
//! A program that needs services is given one channel: to the
//! registry. `lookup("files")` sends the name on it and gets back a
//! handle - a copy, made for this program, of the channel the file
//! service answers on. What names exist on that channel was decided by
//! whoever handed it over (the `registry` crate says how); a name that
//! is not there for this program is `NotFound`, whoever else may have
//! it.
//!
//! One exchange per lookup, as with the file service: the request
//! carries the sending end of a new channel, and the answer comes back
//! on that.

use alloc::string::String;
use alloc::vec::Vec;

use registry::protocol::{Request, NOT_FOUND, OK};

use crate::{Error, Handle};

/// Why a service could not be had.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ServiceError {
    /// The registry answered: there is no such service - for you.
    NotFound,
    /// The registry could not be asked, or did not answer.
    Unreachable(Error),
}

impl ServiceError {
    pub fn describe(self) -> &'static str {
        match self {
            ServiceError::NotFound => "no such service",
            ServiceError::Unreachable(Error::TimedOut) => "the registry is not answering",
            ServiceError::Unreachable(_) => "the registry cannot be reached",
        }
    }
}

pub struct Services {
    registry: Handle,
}

impl Services {
    /// `registry` is this program's channel to the registry.
    pub fn new(registry: Handle) -> Services {
        Services { registry }
    }

    /// One exchange: the reply's length (status byte included), and
    /// the handle that came with it, if one did.
    fn call(&self, request: Request, reply: &mut [u8]) -> Result<(usize, Option<Handle>), ServiceError> {
        let mut message = [0u8; 1 + registry::protocol::NAME_MAX];
        // A name too long to ask for is a name nothing has.
        let length = request.encode(&mut message).ok_or(ServiceError::NotFound)?;
        let (answer, theirs) = crate::channel_create().map_err(ServiceError::Unreachable)?;
        let sent = crate::send(self.registry, &message[..length], &[theirs]);
        if sent.is_err() {
            let _ = crate::close(theirs); // not sent: still ours
        }
        // The wait has a limit, and it is a long one: while the
        // registry is being replaced the request waits in the channel,
        // and the new registry answers it.
        let deadline = crate::ticks() + crate::ANSWER_WAIT;
        let mut handles = [0u64; 1];
        let received = sent.and_then(|()| crate::receive_until(answer, reply, &mut handles, deadline));
        let _ = crate::close(answer);
        let (length, count) = received.map_err(ServiceError::Unreachable)?;
        Ok((length, (count == 1).then_some(handles[0])))
    }

    /// The service called `name`: a handle to it, which is the
    /// caller's to keep, and to close.
    pub fn lookup(&self, name: &str) -> Result<Handle, ServiceError> {
        let mut reply = [0u8; 8];
        match self.call(Request::Lookup(name), &mut reply)? {
            (1.., Some(handle)) if reply[0] == OK => Ok(handle),
            (length, handle) => {
                // Anything else that brought a handle: not wanted.
                if let Some(handle) = handle {
                    let _ = crate::close(handle);
                }
                match (length, reply[0]) {
                    (1.., NOT_FOUND) => Err(ServiceError::NotFound),
                    _ => Err(ServiceError::Unreachable(Error::InvalidArgument)),
                }
            }
        }
    }

    /// The names a lookup would find.
    pub fn list(&self) -> Result<Vec<String>, ServiceError> {
        let mut reply = [0u8; abi::MESSAGE_MAX as usize];
        match self.call(Request::List, &mut reply)? {
            (length @ 1.., None) if reply[0] == OK => Ok(abi::arguments::decode(&reply[1..length])
                .map(|name| String::from_utf8_lossy(name).into_owned())
                .collect()),
            (_, handle) => {
                if let Some(handle) = handle {
                    let _ = crate::close(handle);
                }
                Err(ServiceError::Unreachable(Error::InvalidArgument))
            }
        }
    }

    /// A test: asks the registry to end, as if it had failed. No reply
    /// comes - init notices, and starts another.
    pub fn crash(&self) -> Result<(), ServiceError> {
        let mut message = [0u8; 1];
        let length = Request::Crash.encode(&mut message).ok_or(ServiceError::NotFound)?;
        crate::send(self.registry, &message[..length], &[]).map_err(ServiceError::Unreachable)
    }
}
