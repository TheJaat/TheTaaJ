//! The file protocol's server side, as a program runs it: requests on
//! channels in, answers out, and the streams and open files a client
//! asks for (J60) - for any store. `fs` serves the disk or memory with
//! it, `dev` its devices (J65). (Not a program: cargo builds the files
//! directly in src/bin, and each of the two includes this.)
//!
//! The protocol itself, each request's answer, is the `fileserver`
//! crate's, tested on the host; here it is only connected to channels.

use alloc::vec::Vec;

use fileserver::nodes::{NodeId, ROOT, ROOT_READ_ONLY};
use fileserver::protocol::{self, Request, Status, MESSAGE_MAX};
use fileserver::{Nodes, Store, Stream};
use taaj::{println, Error, Handle};

/// A READ_INTO: the file's bytes, straight into the memory object the
/// client sent. The reply, or None if the message was not a READ_INTO
/// or the object cannot be mapped for writing (then there is no answer
/// worth sending: the client sees its reply channel closed).
fn read_into(store: &mut impl Store, nodes: &Nodes, node: NodeId, message: &[u8], memory: Handle, reply: &mut [u8; MESSAGE_MAX]) -> Option<usize> {
    let size = usize::try_from(taaj::memory_size(memory).ok()?).ok()?;
    let base = taaj::memory_map(memory, true).ok()?;
    // SAFETY: the mapping is `size` bytes long, writable, and ours until
    // it is unmapped below; nothing else in this program touches it.
    let into = unsafe { core::slice::from_raw_parts_mut(base, size) };
    let length = nodes.serve_into(store, node, message, into, reply);
    let _ = taaj::memory_unmap(base);
    length
}

/// A WRITE_FROM: the bytes, straight from the memory object the client
/// sent. The reply, or None if the object cannot be mapped.
fn write_from(store: &mut impl Store, nodes: &Nodes, node: NodeId, message: &[u8], memory: Handle, reply: &mut [u8; MESSAGE_MAX]) -> Option<usize> {
    let size = usize::try_from(taaj::memory_size(memory).ok()?).ok()?;
    let base = taaj::memory_map(memory, false).ok()?;
    // SAFETY: the mapping is `size` bytes long and ours until it is
    // unmapped below; it is only read.
    let from = unsafe { core::slice::from_raw_parts(base, size) };
    let length = nodes.serve_from(store, node, message, from, reply);
    let _ = taaj::memory_unmap(base);
    length
}

/// A stream this service keeps: the channel it travels on, and what it
/// is.
struct Open {
    channel: Handle,
    stream: Stream,
    /// An input stream whose next message found no room in the channel.
    full: bool,
}

/// Most messages taken from or sent on one stream at a time, so that
/// one busy stream does not keep the requests waiting.
const STREAM_TURN: usize = 64;

/// Serves `store` until nobody holds the other end of `requests`: the
/// root, open to change; `look`, if given, the root open to read only.
/// `name` is how this service signs what it prints. Returns the exit
/// code.
pub fn serve(name: &str, store: &mut impl Store, requests: Handle, look: Option<Handle>) -> i64 {
    let mut message = [0u8; MESSAGE_MAX];
    let mut reply = [0u8; MESSAGE_MAX];
    let mut nodes = Nodes::new(store);
    // The channels watched: requests that may change files, then -
    // while anyone holds its other end - requests that may only look,
    // then the streams, then the open files and directories.
    let mut look = look;
    let mut streams: Vec<Open> = Vec::new();
    let mut opened: Vec<(Handle, NodeId)> = Vec::new();
    let mut watched: Vec<Handle> = Vec::new();
    loop {
        send_inputs(store, &mut streams, &mut message);
        watched.clear();
        watched.push(requests);
        watched.extend(look);
        let first_stream = watched.len();
        watched.extend(streams.iter().map(|open| open.channel));
        let first_node = watched.len();
        watched.extend(opened.iter().map(|&(channel, _)| channel));
        // An input stream waiting for room is tried again a tick later:
        // the kernel has no "wake me when there is room".
        let ready = if streams.iter().any(|open| open.full) {
            taaj::object_wait_until(&watched, taaj::ticks() + 1)
        } else {
            taaj::object_wait(&watched).map(Some)
        };
        let from = match ready {
            Ok(Some(from)) => from,
            Ok(None) => continue,
            Err(error) => {
                println!("[{name}] cannot wait: {error:?}");
                return 2;
            }
        };
        if (first_stream..first_node).contains(&from) {
            take_from(store, &mut streams, from - first_stream, &mut message);
            continue;
        }
        empty_outputs(store, &mut streams, &mut message);
        let node = match from {
            0 => ROOT,
            _ if from < first_stream => ROOT_READ_ONLY,
            _ => opened[from - first_node].1,
        };
        // Room for the largest message and the most handles one can
        // carry: a message that did not fit would stay at the head of
        // the queue, and be refused again, forever.
        let mut handles = [0u64; abi::MESSAGE_HANDLES_MAX as usize];
        let (n, count) = match taaj::receive(watched[from], &mut message, &mut handles) {
            Ok(received) => received,
            // The service lives as long as its first channel has a
            // sender; the second going only means fewer to watch.
            Err(Error::PeerClosed) if from == 0 => return 0,
            Err(Error::PeerClosed) if from < first_stream => {
                look = None;
                continue;
            }
            // Every handle to an open file closed: the client is done
            // with it.
            Err(Error::PeerClosed) => {
                let (channel, node) = opened.remove(from - first_node);
                let _ = taaj::close(channel);
                nodes.close(store, node);
                continue;
            }
            Err(error) => {
                println!("[{name}] cannot receive: {error:?}");
                return 2;
            }
        };
        // Two handles: a file to open, with its channel; a stream to
        // open, with its channel; or a READ_INTO or WRITE_FROM, with its
        // memory object. The object is let go before the answer goes: a
        // client that starts a program from it at once must find it its
        // own alone, or the kernel copies it first (ipc::spawn).
        if count == 2 && message[0] == protocol::OPEN {
            let (length, node) = nodes.open(store, node, &message[..n], &mut reply);
            match node {
                Some(node) => opened.push((handles[1], node)),
                None => {
                    let _ = taaj::close(handles[1]);
                }
            }
            let _ = taaj::send(handles[0], &reply[..length], &[]);
            let _ = taaj::close(handles[0]);
            continue;
        }
        if count == 2 && Stream::is_opened_by(&message[..n]) {
            let (length, stream) = if streams.len() >= protocol::STREAMS_MAX {
                reply[0] = Status::Busy as u8;
                (1, None)
            } else {
                nodes.open_stream(store, node, &message[..n], &mut reply)
            };
            match stream {
                Some(stream) => streams.push(Open { channel: handles[1], stream, full: false }),
                None => {
                    let _ = taaj::close(handles[1]);
                }
            }
            let _ = taaj::send(handles[0], &reply[..length], &[]);
            let _ = taaj::close(handles[0]);
            continue;
        }
        if count == 2 && message[0] == protocol::WRITE_FROM {
            let length = write_from(store, &nodes, node, &message[..n], handles[1], &mut reply);
            let _ = taaj::close(handles[1]);
            if let Some(length) = length {
                let _ = taaj::send(handles[0], &reply[..length], &[]);
            }
            let _ = taaj::close(handles[0]);
            continue;
        }
        if count == 2 {
            let length = read_into(store, &nodes, node, &message[..n], handles[1], &mut reply);
            let _ = taaj::close(handles[1]);
            if let Some(length) = length {
                let _ = taaj::send(handles[0], &reply[..length], &[]);
            }
            let _ = taaj::close(handles[0]);
            continue;
        }
        // One handle, the reply channel. Anything else: not a request.
        if count != 1 {
            for &handle in &handles[..count] {
                let _ = taaj::close(handle);
            }
            continue;
        }
        let length = nodes.serve(store, node, &message[..n], &mut reply);
        // A client that gave up has closed its end: nothing to do.
        let _ = taaj::send(handles[0], &reply[..length], &[]);
        let _ = taaj::close(handles[0]);
        // The test of a service that stops answering: answered - so
        // the one who asked is not the one kept waiting - then silence.
        if reply[0] == Status::Ok as u8 {
            if let Some(Request { op: protocol::STALL, number, .. }) = Request::decode(&message[..n]) {
                println!("[{name}] asked to stall: answering nothing for {number} ticks");
                taaj::sleep(number);
                println!("[{name}] answering again");
            }
        }
    }
}

// Every channel is watched in one object_wait: the root's two, the
// streams and the open files must fit in it.
const _: () = assert!(2 + protocol::STREAMS_MAX + protocol::OPEN_MAX <= abi::WAIT_HANDLES_MAX as usize);

/// Lets stream `index` go: its channel closed, and no longer watched.
/// For an output stream the program writing finds nobody reading; for
/// an input stream the program reading finds the end.
fn close_stream(streams: &mut Vec<Open>, index: usize) {
    let open = streams.remove(index);
    let _ = taaj::close(open.channel);
}

/// Stream `index`'s channel is ready: messages for an output stream,
/// or the reader of an input stream gone. Every message already waiting
/// (up to STREAM_TURN) is taken, and their bytes added to the file
/// together: one commit. Returns whether the stream is still open.
fn take_from(store: &mut impl Store, streams: &mut Vec<Open>, index: usize, message: &mut [u8; MESSAGE_MAX]) -> bool {
    let mut handles = [0u64; abi::MESSAGE_HANDLES_MAX as usize];
    let mut gathered: Vec<u8> = Vec::new();
    let mut open = true;
    for taken in 0..STREAM_TURN {
        // The first was said to be there; the rest only if they are.
        if taken > 0 && !matches!(taaj::poll(&[streams[index].channel]), Ok(Some(_))) {
            break;
        }
        match taaj::receive(streams[index].channel, message, &mut handles) {
            Ok((n, count)) => {
                for &handle in &handles[..count] {
                    let _ = taaj::close(handle); // not part of a stream
                }
                if let Some(bytes) = fileserver::stream_data(&message[..n]) {
                    gathered.extend_from_slice(bytes);
                }
            }
            Err(_) => {
                // The writer is gone: what it sent first still counts.
                open = false;
                break;
            }
        }
    }
    // A file that cannot take more: the writer is told the only way a
    // stream can tell it, by nobody being there.
    if streams[index].stream.add(store, &gathered).is_err() || !open {
        close_stream(streams, index);
        return false;
    }
    true
}

/// Everything already sent on the output streams, into their files.
fn empty_outputs(store: &mut impl Store, streams: &mut Vec<Open>, message: &mut [u8; MESSAGE_MAX]) {
    let mut index = 0;
    while index < streams.len() {
        if !matches!(streams[index].stream, Stream::Out { .. }) {
            index += 1;
            continue;
        }
        let mut open = true;
        let mut turns = 0;
        // A turn takes up to STREAM_TURN messages; a few turns, so that
        // a writer that keeps the channel full cannot hold the requests
        // back for ever.
        while open && turns < 4 && matches!(taaj::poll(&[streams[index].channel]), Ok(Some(_))) {
            open = take_from(store, streams, index, message);
            turns += 1;
        }
        // A stream closed is out of the list: the next is at `index`.
        if open {
            index += 1;
        }
    }
}

/// The input streams, sent on as far as their channels have room; one
/// at its end is closed.
fn send_inputs(store: &mut impl Store, streams: &mut Vec<Open>, message: &mut [u8; MESSAGE_MAX]) {
    let mut index = 0;
    while index < streams.len() {
        let mut open = true;
        streams[index].full = false;
        for _ in 0..STREAM_TURN {
            let Open { channel, stream, full } = &mut streams[index];
            if !matches!(stream, Stream::In { .. }) {
                break;
            }
            let length = match stream.next(store, message) {
                Ok(0) | Err(_) => 0,
                Ok(length) => length,
            };
            if length == 0 {
                open = false;
                break;
            }
            match taaj::send(*channel, &message[..length], &[]) {
                Ok(()) => stream.sent(length),
                Err(Error::QueueFull) => {
                    *full = true;
                    break;
                }
                Err(_) => {
                    open = false;
                    break;
                }
            }
        }
        // Sent its turn and more to come: tried again soon.
        if open && matches!(streams[index].stream, Stream::In { .. }) && !streams[index].full {
            streams[index].full = true;
        }
        if open {
            index += 1;
        } else {
            close_stream(streams, index);
        }
    }
}
