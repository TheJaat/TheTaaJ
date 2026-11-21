//! tcpsend: makes a TCP connection, sends, and shows what comes back -
//! a program on the disk.
//!
//!   tcpsend ADDRESS PORT TEXT...     sends the text and a new line
//!   tcpsend ADDRESS PORT fill N      sends N bytes of a pattern
//!   tcpsend ADDRESS PORT             sends nothing
//!
//! Then it says it has no more to send, and reads until the other side
//! says the same. What came back is printed if it is a line or two of
//! text; in any case its length and a sum of its bytes are, which is
//! enough to compare with what the other side says it sent.
//!
//! Exit code 0 if the connection was made and ended in order.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;

use taaj::net::{Ipv4, Net, TcpStream};
use taaj::{eprintln, println};

taaj::program!(main);

/// Byte `n` of the pattern `fill` sends: the same on every machine, so
/// the other side can check each byte without being sent a copy.
fn pattern(n: u64) -> u8 {
    (n.wrapping_mul(7).wrapping_add(n >> 8).wrapping_add(n >> 16)) as u8
}

/// A sum that notices bytes out of order as well as wrong ones
/// (Fletcher's: the second sum adds up the first).
#[derive(Default)]
struct Sum {
    a: u32,
    b: u32,
}

impl Sum {
    fn add(&mut self, data: &[u8]) {
        for &byte in data {
            self.a = (self.a + u32::from(byte)) % 65521;
            self.b = (self.b + self.a) % 65521;
        }
    }

    fn value(&self) -> u32 {
        self.b << 16 | self.a
    }
}

fn send_pattern(stream: &mut TcpStream, length: u64) -> Result<Sum, taaj::net::NetError> {
    let mut sum = Sum::default();
    let mut piece = [0u8; 2048];
    let mut sent = 0;
    while sent < length {
        let count = (length - sent).min(piece.len() as u64) as usize;
        for (n, byte) in piece[..count].iter_mut().enumerate() {
            *byte = pattern(sent + n as u64);
        }
        stream.write(&piece[..count])?;
        sum.add(&piece[..count]);
        sent += count as u64;
    }
    Ok(sum)
}

fn main() -> i64 {
    let arguments = taaj::arguments();
    let (Some(to), Some(port)) = (
        arguments.first().and_then(|text| Ipv4::parse(text)),
        arguments.get(1).and_then(|text| text.parse::<u16>().ok()).filter(|&port| port != 0),
    ) else {
        eprintln!("usage: tcpsend ADDRESS PORT [TEXT... | fill N]");
        return 2;
    };
    let fill = match &arguments[2..] {
        [word, length] if word == "fill" => match length.parse::<u64>() {
            Ok(length) => Some(length),
            Err(_) => {
                eprintln!("usage: tcpsend ADDRESS PORT fill N");
                return 2;
            }
        },
        _ => None,
    };
    let Some(net) = Net::find() else {
        eprintln!("tcpsend: no network for this program");
        return 1;
    };
    let started = taaj::ticks();
    let mut stream = match net.tcp_connect(to, port) {
        Ok(stream) => stream,
        Err(error) => {
            eprintln!("tcpsend: {to}:{port}: {}", error.describe());
            return 1;
        }
    };
    println!("tcpsend: connected to {to}:{port} from port {}", stream.local_port());

    let sent = match fill {
        Some(length) => send_pattern(&mut stream, length).map(|sum| (length, sum)),
        None => {
            let mut text = arguments[2..].join(" ");
            if !text.is_empty() {
                text.push('\n');
            }
            let mut sum = Sum::default();
            sum.add(text.as_bytes());
            stream.write(text.as_bytes()).map(|()| (text.len() as u64, sum))
        }
    };
    let (length, sum) = match sent.and_then(|sent| stream.shutdown().map(|()| sent)) {
        Ok(sent) => sent,
        Err(error) => {
            eprintln!("tcpsend: while sending: {}", error.describe());
            return 1;
        }
    };
    println!("tcpsend: {length} bytes sent, sum {:08x}", sum.value());

    let mut back = Sum::default();
    let mut total = 0u64;
    let mut kept = Vec::new();
    let mut piece = [0u8; 4096];
    loop {
        match stream.read(&mut piece) {
            Ok(0) => break,
            Ok(count) => {
                back.add(&piece[..count]);
                total += count as u64;
                if kept.len() < 200 {
                    kept.extend_from_slice(&piece[..count.min(200 - kept.len())]);
                }
            }
            Err(error) => {
                eprintln!("tcpsend: after {total} bytes back: {}", error.describe());
                return 1;
            }
        }
    }
    println!("tcpsend: {total} bytes back, sum {:08x}, in {} ms", back.value(), (taaj::ticks() - started) * 10);
    if total <= 200 {
        if let Ok(text) = core::str::from_utf8(&kept) {
            if text.chars().all(|c| c == '\n' || !c.is_control()) {
                for line in text.lines() {
                    println!("tcpsend: | {line}");
                }
            }
        }
    }
    0
}
