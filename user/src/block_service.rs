//! The block service: the disk protocol (`abi::block`), served.
//!
//! Every disk driver answers the same requests on the same two
//! channels - one that may write, one that may only read - so that fs,
//! and anyone else, cannot tell which driver is behind them: `blk` for
//! a virtio disk, `ahci` for a SATA one. What differs is only how the
//! sectors get between the disk and memory; that is a driver's
//! `Device`. Checking requests, the clients' memory, the channels and
//! the watchdog's test all live here, once.
//!
//! A request for one sector carries its data in the messages. A request
//! for many (READ_MANY, WRITE_MANY) comes with a memory object: the
//! driver maps it, copies between it and its DMA buffer, and unmaps it
//! again before replying. One device request and one copy for 128 KiB,
//! where sector-sized messages would take 256 of each. The device
//! cannot use the client's memory directly: it needs physical
//! addresses, and only DMA memory comes with those.
//!
//! A client's memory object is the client's: it may be smaller than the
//! request claims, or not a memory object at all. Its size is asked of
//! the kernel before anything is mapped or touched, so a wrong one
//! costs the client its request and the driver nothing.

use abi::block;

use crate::{println, Error, Handle};

/// A disk, as a driver drives it.
pub trait Device {
    /// How many sectors it has, and whether it refuses writes.
    fn sectors(&self) -> u64;
    fn read_only(&self) -> bool;
    /// `count` sectors (1 to block::SECTORS_MAX, inside the disk -
    /// checked before this is called) from `sector` on, between the disk
    /// and the data buffer. Returns a block status.
    fn transfer(&mut self, write: bool, sector: u64, count: u32) -> u8;
    /// The first `length` bytes of the data buffer, which holds at
    /// least block::SECTORS_MAX sectors.
    fn data(&mut self, length: usize) -> &mut [u8];
}

/// Checks a transfer against the disk, then does it.
fn transfer(device: &mut impl Device, write: bool, sector: u64, count: u32) -> u8 {
    if count == 0 || count > block::SECTORS_MAX {
        return block::BAD_REQUEST;
    }
    if !sector.checked_add(u64::from(count)).is_some_and(|end| end <= device.sectors()) {
        return block::OUT_OF_RANGE;
    }
    if write && device.read_only() {
        return block::READ_ONLY;
    }
    device.transfer(write, sector, count)
}

/// READ_MANY or WRITE_MANY: `count` sectors between the disk and the
/// client's memory object `lent`. Returns a block status.
fn transfer_many(device: &mut impl Device, write: bool, sector: u64, count: u32, lent: Handle) -> u8 {
    if count == 0 || count > block::SECTORS_MAX {
        return block::BAD_REQUEST;
    }
    let length = count as usize * block::SECTOR;
    // Is it a memory object, and big enough? Asked before mapping:
    // copying past the end of a smaller one would be a fault - in this
    // process, the driver.
    if !crate::memory_size(lent).is_ok_and(|size| size >= length as u64) {
        return block::BAD_REQUEST;
    }
    // Writable only to fill it; refused if the handle lacks the right.
    let Ok(theirs) = crate::memory_map(lent, !write) else {
        return block::BAD_REQUEST;
    };
    // SAFETY (both copies): `theirs` is a mapping of at least `length`
    // bytes, checked above, and stays mapped until the unmap below. The
    // client may change the bytes meanwhile; then it reads or writes
    // its own garbage, and nothing else.
    let status = if write {
        unsafe { theirs.copy_to_nonoverlapping(device.data(length).as_mut_ptr(), length) };
        transfer(device, true, sector, count)
    } else {
        let status = transfer(device, false, sector, count);
        if status == block::OK {
            unsafe { theirs.copy_from_nonoverlapping(device.data(length).as_ptr(), length) };
        }
        status
    };
    let _ = crate::memory_unmap(theirs);
    status
}

/// Answers one request; returns the reply's length in `reply`. `lent`
/// is the memory object that came with it, if one did.
fn answer(device: &mut impl Device, request: &[u8], reply: &mut [u8], may_write: bool, lent: Option<Handle>) -> usize {
    let op = request.first().copied();
    if matches!(op, Some(block::WRITE | block::WRITE_MANY)) && !may_write {
        reply[0] = block::READ_ONLY;
        return 1;
    }
    if let (Some(op @ (block::READ_MANY | block::WRITE_MANY)), Some(lent)) = (op, lent) {
        let count = request.get(9..13).and_then(|b| b.try_into().ok()).map(u32::from_le_bytes);
        let sector = request.get(1..9).and_then(|b| b.try_into().ok()).map(u64::from_le_bytes);
        reply[0] = match (sector, count) {
            (Some(sector), Some(count)) if request.len() == block::MANY_REQUEST_SIZE => {
                transfer_many(device, op == block::WRITE_MANY, sector, count, lent)
            }
            _ => block::BAD_REQUEST,
        };
        return 1;
    }
    let sector = request.get(1..9).and_then(|b| b.try_into().ok()).map(u64::from_le_bytes);
    let (status, length) = match (op, sector) {
        (Some(block::INFO), _) => {
            reply[1..9].copy_from_slice(&device.sectors().to_le_bytes());
            reply[9] = u8::from(device.read_only());
            (block::OK, 9)
        }
        (Some(block::READ), Some(sector)) if request.len() == block::REQUEST_SIZE => {
            match transfer(device, false, sector, 1) {
                block::OK => {
                    reply[1..=block::SECTOR].copy_from_slice(device.data(block::SECTOR));
                    (block::OK, block::SECTOR)
                }
                status => (status, 0),
            }
        }
        (Some(block::WRITE), Some(sector)) if request.len() == block::REQUEST_SIZE + block::SECTOR => {
            device.data(block::SECTOR).copy_from_slice(&request[block::REQUEST_SIZE..]);
            (transfer(device, true, sector, 1), 0)
        }
        _ => (block::BAD_REQUEST, 0),
    };
    reply[0] = status;
    1 + length
}

/// Serves `device` until both channels have no clients left (0), or a
/// channel fails (6). `name` is the driver's, for its log lines.
pub fn serve(device: &mut impl Device, name: &str, writers: Handle, readers: Handle) -> i64 {
    let mut message = [0u8; abi::MESSAGE_MAX as usize];
    let mut reply = [0u8; abi::MESSAGE_MAX as usize];
    let channels = [writers, readers];
    let mut open = [true, true];
    loop {
        // Whichever channel has a request - both, in turn, if both do.
        let live: [Handle; 2] = match open {
            [true, true] => channels,
            [true, false] => [writers; 2],
            [false, true] => [readers; 2],
            [false, false] => return 0,
        };
        let which = match crate::object_wait(&live) {
            Ok(n) => n,
            Err(_) => return 6,
        };
        let requests = live[which];
        let may_write = requests == writers;
        // As in fs: room for any message, so none can get stuck.
        let mut handles = [0u64; abi::MESSAGE_HANDLES_MAX as usize];
        let (n, count) = match crate::receive(requests, &mut message, &mut handles) {
            Ok(received) => received,
            Err(Error::PeerClosed) => {
                open[usize::from(!may_write)] = false; // no clients left on it
                continue;
            }
            Err(error) => {
                println!("[{name}] cannot receive: {error:?}");
                return 6;
            }
        };
        // The reply channel, and for the "many" requests the client's
        // memory. Anything else: not a request.
        if !(1..=2).contains(&count) {
            for &handle in &handles[..count] {
                let _ = crate::close(handle);
            }
            continue;
        }
        let lent = (count == 2).then_some(handles[1]);
        // The test of a driver that has stopped: answered first.
        let mut stall = 0;
        if n == block::REQUEST_SIZE && message[0] == block::STALL {
            let mut ticks = [0u8; 8];
            ticks.copy_from_slice(&message[1..9]);
            stall = u64::from_le_bytes(ticks);
        }
        let length = if (1..=abi::STALL_MAX).contains(&stall) {
            reply[0] = block::OK;
            1
        } else {
            stall = 0;
            answer(device, &message[..n], &mut reply, may_write, lent)
        };
        let _ = crate::send(handles[0], &reply[..length], &[]);
        for &handle in &handles[..count] {
            let _ = crate::close(handle);
        }
        if stall > 0 {
            println!("[{name}] asked to stall: answering nothing for {stall} ticks");
            crate::sleep(stall);
            println!("[{name}] answering again");
        }
    }
}
