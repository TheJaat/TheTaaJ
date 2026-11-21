//! tcpecho: sends back everything written to it on a TCP connection -
//! a program on the disk.
//!
//!   tcpecho [PORT [COUNT]]
//!
//! Listens on PORT (7, the port set aside for exactly this, if not
//! said). Each connection made to it is served to its end - every
//! byte read is written back - and then the next; others wait their
//! turn. After COUNT connections it ends; without a count it goes on
//! until stopped (Ctrl+C, or `kill`).

#![no_std]
#![no_main]

use taaj::net::{Net, TcpStream};
use taaj::{eprintln, println};

taaj::program!(main);

/// The bytes echoed, and whether the connection ended as it should:
/// with the other side saying it had no more.
fn serve(stream: &mut TcpStream) -> (u64, bool) {
    let mut data = [0u8; 4096];
    let mut echoed = 0;
    loop {
        match stream.read(&mut data) {
            Ok(0) => return (echoed, stream.shutdown().is_ok()),
            Ok(count) => {
                if stream.write(&data[..count]).is_err() {
                    return (echoed, false);
                }
                echoed += count as u64;
            }
            Err(_) => return (echoed, false),
        }
    }
}

fn main() -> i64 {
    let arguments = taaj::arguments();
    let (Some(port), Some(count)) = (
        arguments.first().map_or(Some(7u16), |text| text.parse().ok()).filter(|&port| port != 0),
        arguments.get(1).map_or(Some(u64::MAX), |text| text.parse().ok()),
    ) else {
        eprintln!("usage: tcpecho [PORT [COUNT]]");
        return 2;
    };
    if arguments.len() > 2 {
        eprintln!("usage: tcpecho [PORT [COUNT]]");
        return 2;
    }
    let Some(net) = Net::find() else {
        eprintln!("tcpecho: no network for this program");
        return 1;
    };
    let mut listener = match net.tcp_listen(port) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("tcpecho: port {port}: {}", error.describe());
            return 1;
        }
    };
    println!("tcpecho: listening on port {port}");
    for _ in 0..count {
        let mut stream = match listener.accept_until(u64::MAX) {
            Ok(Some(stream)) => stream,
            Ok(None) => continue,
            Err(_) => {
                eprintln!("tcpecho: the network service stopped");
                return 1;
            }
        };
        let (from, from_port) = stream.peer();
        let (echoed, in_order) = serve(&mut stream);
        println!("tcpecho: {echoed} bytes echoed to {from}:{from_port}{}", if in_order { "" } else { " - and then the connection broke" });
    }
    0
}
