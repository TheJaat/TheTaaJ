//! The file server (fileserver/src/lib.rs): requests from any program
//! with a handle to it. The input is a sequence of messages (a length
//! byte, then the message), served in turn by one store in memory - so
//! a request can find what the ones before it left. Paths too
//! (fileserver/src/path.rs).
#![no_main]
use fileserver::protocol::MESSAGE_MAX;
use fileserver::ram::RamStore;
use fileserver::{serve, Access};

fuzz::fuzz_target!(|data: &[u8]| {
    let mut store = RamStore::new();
    let mut reply = [0u8; MESSAGE_MAX];
    for message in fuzz::pieces(data) {
        let _ = serve(&mut store, Access::ReadWrite, message, &mut reply);
    }
    let text = String::from_utf8_lossy(data);
    let (directory, path) = text.split_once('\n').unwrap_or(("", &text));
    let resolved = fileserver::path::resolve(directory, path);
    let _ = fileserver::path::display(&resolved);
});
