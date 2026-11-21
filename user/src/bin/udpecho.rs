//! udpecho: sends every datagram back to whoever sent it - a program
//! on the disk.
//!
//!   udpecho [PORT [COUNT]]
//!
//! Listens on PORT (7, the port set aside for exactly this, if not
//! said) and answers each datagram with the same bytes. After COUNT
//! datagrams it ends; without a count it goes on until stopped
//! (Ctrl+C, or `kill`).

#![no_std]
#![no_main]

use taaj::net::Net;
use taaj::{eprintln, println};

taaj::program!(main);

fn main() -> i64 {
    let arguments = taaj::arguments();
    let (Some(port), Some(count)) = (
        arguments.first().map_or(Some(7u16), |text| text.parse().ok()).filter(|&port| port != 0),
        arguments.get(1).map_or(Some(u64::MAX), |text| text.parse().ok()),
    ) else {
        eprintln!("usage: udpecho [PORT [COUNT]]");
        return 2;
    };
    if arguments.len() > 2 {
        eprintln!("usage: udpecho [PORT [COUNT]]");
        return 2;
    }
    let Some(net) = Net::find() else {
        eprintln!("udpecho: no network for this program");
        return 1;
    };
    let socket = match net.udp_bind(port) {
        Ok(socket) => socket,
        Err(error) => {
            eprintln!("udpecho: port {port}: {}", error.describe());
            return 1;
        }
    };
    println!("udpecho: listening on port {port}");
    let mut data = [0u8; abi::net::DATAGRAM_MAX];
    for _ in 0..count {
        match socket.receive_until(&mut data, u64::MAX) {
            Ok(Some((from, from_port, length))) => {
                println!("udpecho: {length} bytes from {from}:{from_port}");
                if socket.send_to(from, from_port, &data[..length]).is_err() {
                    eprintln!("udpecho: could not answer {from}:{from_port}");
                }
            }
            Ok(None) => {}
            Err(_) => {
                eprintln!("udpecho: the network service stopped");
                return 1;
            }
        }
    }
    0
}
