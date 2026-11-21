//! ping: is that machine there? - a program on the disk.
//!
//!   ping ADDRESS [COUNT [BYTES]]
//!
//! Sends COUNT echo requests (4 if not said), a second apart, each
//! with BYTES bytes of data (56), and says what came back. Exit code 0
//! if every one was answered.
//!
//! The times are in the kernel's ticks, ten milliseconds each: an
//! answer within the same tick shows as "under 10 ms".

#![no_std]
#![no_main]

use taaj::net::{Ipv4, Net, NetError};
use taaj::{eprintln, println};

taaj::program!(main);

/// How long an answer may take, and how far apart the pings are: one
/// second.
const WAIT: u32 = 100;

fn main() -> i64 {
    let arguments = taaj::arguments();
    let number = |n: usize, default: u16| arguments.get(n).map_or(Some(default), |text| text.parse().ok());
    let (Some(to), Some(count), Some(bytes)) = (
        arguments.first().and_then(|text| Ipv4::parse(text)),
        number(1, 4).filter(|&count| count > 0),
        number(2, 56).filter(|&bytes| bytes <= abi::net::PING_DATA_MAX),
    ) else {
        eprintln!("usage: ping ADDRESS [COUNT [BYTES]] - an address like 10.0.2.2, at most {} bytes", abi::net::PING_DATA_MAX);
        return 2;
    };
    if arguments.len() > 3 {
        eprintln!("usage: ping ADDRESS [COUNT [BYTES]]");
        return 2;
    }
    let Some(net) = Net::find() else {
        eprintln!("ping: no network for this program");
        return 1;
    };

    let mut answered = 0u16;
    for sequence in 1..=count {
        let began = taaj::ticks();
        match net.ping(to, sequence, bytes, WAIT) {
            Ok(pong) => {
                answered += 1;
                match taaj::ticks() - began {
                    0 => println!("{bytes} bytes from {}: seq={sequence} ttl={} time under 10 ms", pong.from, pong.ttl),
                    ticks => println!("{bytes} bytes from {}: seq={sequence} ttl={} time {} ms", pong.from, pong.ttl, ticks * 10),
                }
            }
            Err(NetError::Refused(abi::net::TIMED_OUT)) => println!("no answer from {to}: seq={sequence}"),
            Err(NetError::Refused(abi::net::UNREACHABLE)) => println!("{to} cannot be reached: seq={sequence}, {}", abi::net::describe(abi::net::UNREACHABLE)),
            Err(error) => {
                eprintln!("ping: {}", error.describe());
                return 1;
            }
        }
        // A second from one ping to the next, however long the answer took.
        let next = began + u64::from(WAIT);
        if sequence < count && taaj::ticks() < next {
            taaj::sleep(next - taaj::ticks());
        }
    }
    println!("{count} sent to {to}, {answered} answered");
    i64::from(answered != count)
}
