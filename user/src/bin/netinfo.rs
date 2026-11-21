//! netinfo: who this machine is on its network, and what its network
//! service has counted - a program on the disk.

#![no_std]
#![no_main]

use abi::net as protocol;
use taaj::net::{Ipv4, Net};
use taaj::{eprintln, println};

taaj::program!(main);

fn main() -> i64 {
    let Some(net) = Net::find() else {
        eprintln!("netinfo: no network for this program");
        return 1;
    };
    let info = match net.info() {
        Ok(info) => info,
        Err(error) => {
            eprintln!("netinfo: {}", error.describe());
            return 1;
        }
    };
    let (mac, counted) = (info.mac, info.counters);
    println!("address  {}/{}", Ipv4(info.address), info.prefix);
    println!("router   {}", Ipv4(info.gateway));
    println!("card     {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}", mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]);
    println!("frames   {} in, {} out", counted[protocol::FRAMES_IN], counted[protocol::FRAMES_OUT]);
    println!(
        "answered {} questions for this address, {} pings",
        counted[protocol::ARP_ANSWERED],
        counted[protocol::ECHOES_ANSWERED]
    );
    println!(
        "datagrams {} in; {} for a port no program has, {} a program did not take",
        counted[protocol::DATAGRAMS_IN],
        counted[protocol::NO_SUCH_PORT],
        counted[protocol::NOT_TAKEN]
    );
    println!(
        "dropped  {} damaged, {} for others, {} of a kind not spoken here; {} not sent",
        counted[protocol::BAD],
        counted[protocol::NOT_FOR_US],
        counted[protocol::UNSUPPORTED],
        counted[protocol::UNDELIVERED]
    );
    println!(
        "tcp      {} segments in, {} sent again, {} answered \"no such connection\"",
        counted[protocol::SEGMENTS_IN],
        counted[protocol::SEGMENTS_AGAIN],
        counted[protocol::SEGMENTS_REFUSED]
    );
    0
}
