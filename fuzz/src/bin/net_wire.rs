//! The network's wire formats (net/src/wire.rs): every frame the card
//! receives, from anyone on the network. The input is an Ethernet
//! frame, followed down; and, so the fuzzer need not build the frame
//! first, each layer's parser is given the input itself too.
#![no_main]
use net::wire::{arp, ethernet, icmp, ipv4, tcp, udp};
use net::Ipv4;

fn packet(data: &[u8]) {
    let Ok((header, payload)) = ipv4::parse(data) else { return };
    match header.protocol {
        1 => drop(icmp::parse(payload)),
        6 => drop(tcp::parse(header.source, header.destination, payload)),
        17 => drop(udp::parse(header.source, header.destination, payload)),
        _ => {}
    }
}

fuzz::fuzz_target!(|data: &[u8]| {
    if let Some(frame) = ethernet::parse(data) {
        match frame.kind {
            0x0800 => packet(frame.payload),
            0x0806 => drop(arp::parse(frame.payload)),
            _ => {}
        }
    }
    let (a, b) = (Ipv4([10, 0, 2, 2]), Ipv4([10, 0, 2, 15]));
    packet(data);
    let _ = arp::parse(data);
    let _ = icmp::parse(data);
    let _ = udp::parse(a, b, data);
    let _ = tcp::parse(a, b, data);
});
