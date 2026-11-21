//! udpsend: sends one datagram, and shows an answer if one comes - a
//! program on the disk.
//!
//!   udpsend ADDRESS PORT TEXT...
//!
//! The text goes out from a port of the network service's choosing;
//! whatever comes back to that port within two seconds is printed.
//! Exit code 0 if an answer came, 1 if none did.

#![no_std]
#![no_main]

extern crate alloc;

use taaj::net::{Ipv4, Net};
use taaj::{eprintln, println};

taaj::program!(main);

fn main() -> i64 {
    let arguments = taaj::arguments();
    let (Some(to), Some(port)) = (
        arguments.first().and_then(|text| Ipv4::parse(text)),
        arguments.get(1).and_then(|text| text.parse::<u16>().ok()).filter(|&port| port != 0),
    ) else {
        eprintln!("usage: udpsend ADDRESS PORT TEXT...");
        return 2;
    };
    let text = arguments[2..].join(" ");
    if text.is_empty() || text.len() > abi::net::DATAGRAM_MAX {
        eprintln!("usage: udpsend ADDRESS PORT TEXT... (at most {} bytes)", abi::net::DATAGRAM_MAX);
        return 2;
    }
    let Some(net) = Net::find() else {
        eprintln!("udpsend: no network for this program");
        return 1;
    };
    let socket = match net.udp_bind(0) {
        Ok(socket) => socket,
        Err(error) => {
            eprintln!("udpsend: {}", error.describe());
            return 1;
        }
    };
    if socket.send_to(to, port, text.as_bytes()).is_err() {
        eprintln!("udpsend: the network service took nothing");
        return 1;
    }
    println!("udpsend: {} bytes from port {} to {to}:{port}", text.len(), socket.port());
    let mut data = [0u8; abi::net::DATAGRAM_MAX];
    match socket.receive_until(&mut data, taaj::ticks() + 200) {
        Ok(Some((from, from_port, length))) => {
            println!("udpsend: {length} bytes back from {from}:{from_port}: {}", text_of(&data[..length]));
            0
        }
        Ok(None) => {
            println!("udpsend: no answer");
            1
        }
        Err(_) => {
            eprintln!("udpsend: the network service stopped");
            1
        }
    }
}

/// Bytes as text, for showing: what is not text shows as "?".
fn text_of(bytes: &[u8]) -> &str {
    core::str::from_utf8(bytes).unwrap_or("?")
}
