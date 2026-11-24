//! USB mass storage: SCSI commands, carried by the Bulk-Only Transport.
//!
//! A USB stick is a SCSI disk at the end of two bulk pipes. Each command
//! is three steps, all on the bulk endpoints:
//!
//!   1. the **CBW** (Command Block Wrapper), 31 bytes out: "USBC", a tag,
//!      how many bytes the data step moves and which way, and the SCSI
//!      command itself (6 to 16 bytes);
//!   2. the **data**, if any: in (READ, INQUIRY) or out (WRITE);
//!   3. the **CSW** (Command Status Wrapper), 13 bytes in: "USBS", the
//!      same tag, how many bytes of the data step did *not* move (the
//!      residue), and a status - 0 passed, 1 failed, 2 phase error.
//!
//! When the device does not want to go on with a data step, it stalls
//! the endpoint. The host then clears the stall (on the device with
//! CLEAR_FEATURE(ENDPOINT_HALT), on the controller with Reset Endpoint)
//! and reads the CSW, which says what happened. Anything worse - a CSW
//! that is not one, a phase error, a stalled CSW twice - is the "reset
//! recovery": the class's Bulk-Only Mass Storage Reset request, then
//! both endpoints' stalls cleared. (USB Mass Storage Class, Bulk-Only
//! Transport 1.0, section 5.3.)
//!
//! A failed command is followed by REQUEST SENSE, which says why: the
//! first TEST UNIT READY of a stick nearly always fails with "unit
//! attention: the medium may have changed", and is simply asked again.
//!
//! The SCSI commands used (SCSI Primary/Block Commands):
//!   - INQUIRY (0x12): vendor and product names, and whether the medium
//!     is removable;
//!   - TEST UNIT READY (0x00), REQUEST SENSE (0x03);
//!   - READ CAPACITY(10) (0x25): the last sector's number and the sector
//!     size; READ CAPACITY(16) (0x9E) when the disk is too big for 32 bits;
//!   - READ(10)/WRITE(10) (0x28/0x2A), READ(16)/WRITE(16) (0x88/0x8A)
//!     past sector 2^32;
//!   - MODE SENSE(6) (0x1A): whether the medium is write-protected.

use crate::xhci::{self, layout, Bus, Controller, Failure};

/// The CBW's and the CSW's first four bytes: "USBC" and "USBS".
const CBW_SIGNATURE: u32 = 0x4342_5355;
const CSW_SIGNATURE: u32 = 0x5342_5355;
const CBW_LENGTH: usize = 31;
const CSW_LENGTH: usize = 13;

/// The only sector size served: abi::block's.
pub const SECTOR: u32 = 512;

/// Which way a command's data goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Data {
    None,
    In(usize),
    Out(usize),
}

/// Why a command did not do what it was asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The device said "failed"; REQUEST SENSE's key, code and qualifier.
    Failed { key: u8, code: u8, qualifier: u8 },
    /// The transport broke (a stall that would not clear, a CSW that was
    /// not one, a transfer that timed out); the device was reset.
    Transport(&'static str),
}

impl Error {
    pub fn describe(self) -> &'static str {
        match self {
            Error::Failed { key: 0x2, .. } => "not ready",
            Error::Failed { key: 0x3, .. } => "medium error",
            Error::Failed { key: 0x4, .. } => "hardware error",
            Error::Failed { key: 0x5, .. } => "illegal request",
            Error::Failed { key: 0x6, .. } => "unit attention",
            Error::Failed { key: 0x7, .. } => "write-protected",
            Error::Failed { .. } => "the command failed",
            Error::Transport(why) => why,
        }
    }
}

/// A mass storage device, ready for READ and WRITE.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Storage {
    pub slot: u8,
    pub interface: u8,
    /// The bulk endpoints: their DCIs, and their addresses (for
    /// CLEAR_FEATURE).
    out_dci: u8,
    in_dci: u8,
    out_address: u8,
    in_address: u8,
    tag: u32,
    pub sectors: u64,
    pub read_only: bool,
    pub removable: bool,
    /// INQUIRY's vendor (8) and product (16) names, as the device says
    /// them (ASCII, padded with spaces).
    pub vendor: [u8; 8],
    pub product: [u8; 16],
}

/// TEST UNIT READY is asked this many times (with REQUEST SENSE between)
/// before the device is taken to have no medium.
const READY_TRIES: u32 = 10;

impl Storage {
    /// Takes the device's bulk endpoints (configured already) and asks it
    /// what it is: its names, whether it is ready, its size.
    pub fn start<B: Bus>(
        controller: &mut Controller<B>,
        slot: u8,
        interface: u8,
        (out_dci, out_address): (u8, u8),
        (in_dci, in_address): (u8, u8),
    ) -> Result<Storage, &'static str> {
        let mut s = Storage {
            slot,
            interface,
            out_dci,
            in_dci,
            out_address,
            in_address,
            tag: 1,
            sectors: 0,
            read_only: false,
            removable: false,
            vendor: [b' '; 8],
            product: [b' '; 16],
        };
        // INQUIRY: 36 bytes, the standard part.
        let n = s.command(controller, &[0x12, 0, 0, 0, 36, 0], Data::In(36)).map_err(|_| "the device would not say what it is")?;
        if n < 32 {
            return Err("the device's INQUIRY answer is too short");
        }
        let answer = controller.storage_data(36);
        if answer[0] & 0x1F != 0x00 {
            return Err("not a disk (its SCSI device type is not 0)");
        }
        s.removable = answer[1] & 0x80 != 0;
        s.vendor.copy_from_slice(&answer[8..16]);
        s.product.copy_from_slice(&answer[16..32]);

        // Ready? The first answer is usually "unit attention" - asked again.
        let mut ready = false;
        for _ in 0..READY_TRIES {
            match s.command(controller, &[0x00, 0, 0, 0, 0, 0], Data::None) {
                Ok(_) => {
                    ready = true;
                    break;
                }
                Err(Error::Failed { key: 0x2, code: 0x3A, .. }) => return Err("no medium in it"),
                Err(Error::Transport(why)) => return Err(why),
                Err(Error::Failed { .. }) => {}
            }
        }
        if !ready {
            return Err("it never became ready");
        }

        // The size: READ CAPACITY(10), or (16) when it says "more".
        s.command(controller, &[0x25, 0, 0, 0, 0, 0, 0, 0, 0, 0], Data::In(8)).map_err(|_| "the device would not say its size")?;
        let answer = controller.storage_data(8);
        let last = u32::from_be_bytes([answer[0], answer[1], answer[2], answer[3]]);
        let mut size = u32::from_be_bytes([answer[4], answer[5], answer[6], answer[7]]);
        s.sectors = u64::from(last) + 1;
        if last == u32::MAX {
            let mut cdb = [0u8; 16];
            cdb[0] = 0x9E; // SERVICE ACTION IN(16)
            cdb[1] = 0x10; // READ CAPACITY(16)
            cdb[13] = 32;
            s.command(controller, &cdb, Data::In(32)).map_err(|_| "the device would not say its size")?;
            let answer = controller.storage_data(12);
            let mut last = [0u8; 8];
            last.copy_from_slice(&answer[..8]);
            s.sectors = u64::from_be_bytes(last) + 1;
            size = u32::from_be_bytes([answer[8], answer[9], answer[10], answer[11]]);
        }
        if size != SECTOR {
            return Err("its sectors are not 512 bytes");
        }

        // Write-protected? MODE SENSE(6), all pages, the header only: its
        // byte 2's top bit. A device that refuses is taken to be writable
        // - a WRITE will say otherwise.
        if s.command(controller, &[0x1A, 0, 0x3F, 0, 4, 0], Data::In(4)).is_ok() {
            s.read_only = controller.storage_data(4)[2] & 0x80 != 0;
        }
        Ok(s)
    }

    /// Reads (or writes) `count` sectors from `sector` on, into (or from)
    /// the controller's storage buffer.
    pub fn transfer<B: Bus>(&mut self, controller: &mut Controller<B>, write: bool, sector: u64, count: u32) -> Result<(), Error> {
        let bytes = count as usize * SECTOR as usize;
        if count == 0 || bytes > layout::STORAGE_DATA_SIZE {
            return Err(Error::Transport("a transfer bigger than the buffer"));
        }
        let data = if write { Data::Out(bytes) } else { Data::In(bytes) };
        let n = match u32::try_from(sector) {
            Ok(lba) if sector + u64::from(count) <= 1 << 32 && count <= 0xFFFF => {
                let opcode = if write { 0x2A } else { 0x28 };
                let l = lba.to_be_bytes();
                let c = (count as u16).to_be_bytes();
                self.command(controller, &[opcode, 0, l[0], l[1], l[2], l[3], 0, c[0], c[1], 0], data)?
            }
            _ => {
                let mut cdb = [0u8; 16];
                cdb[0] = if write { 0x8A } else { 0x88 };
                cdb[2..10].copy_from_slice(&sector.to_be_bytes());
                cdb[10..14].copy_from_slice(&count.to_be_bytes());
                self.command(controller, &cdb, data)?
            }
        };
        if n != bytes {
            return Err(Error::Transport("the device moved fewer bytes than asked"));
        }
        Ok(())
    }

    /// One command: CBW, data, CSW, and what to do when any of them goes
    /// wrong. Returns how many bytes the data step moved.
    pub fn command<B: Bus>(&mut self, controller: &mut Controller<B>, cdb: &[u8], data: Data) -> Result<usize, Error> {
        match self.exchange(controller, cdb, data)? {
            (n, 0) => Ok(n),
            (_, 1) => Err(self.sense(controller)),
            _ => {
                self.reset_recovery(controller);
                Err(Error::Transport("the device lost track of the command (phase error)"))
            }
        }
    }

    /// The three steps; the CSW's status (0, 1 or 2) and the bytes moved.
    fn exchange<B: Bus>(&mut self, controller: &mut Controller<B>, cdb: &[u8], data: Data) -> Result<(usize, u8), Error> {
        let tag = self.tag;
        self.tag = self.tag.wrapping_add(1);
        let (length, flags) = match data {
            Data::None => (0, 0x00),
            Data::In(n) => (n, 0x80),
            Data::Out(n) => (n, 0x00),
        };
        let mut cbw = [0u8; CBW_LENGTH];
        cbw[0..4].copy_from_slice(&CBW_SIGNATURE.to_le_bytes());
        cbw[4..8].copy_from_slice(&tag.to_le_bytes());
        cbw[8..12].copy_from_slice(&(length as u32).to_le_bytes());
        cbw[12] = flags;
        cbw[13] = 0; // LUN 0
        cbw[14] = cdb.len() as u8;
        cbw[15..15 + cdb.len()].copy_from_slice(cdb);
        let cbw_at = layout::STORAGE_CBW;
        controller.dma_write(cbw_at, &cbw);
        if let Err(failure) = controller.bulk(self.slot, self.out_dci, cbw_at, CBW_LENGTH) {
            // The device would not even take the command: start again.
            self.reset_recovery(controller);
            return Err(Error::Transport(match failure {
                Failure::TimedOut => "the device did not take the command in time",
                Failure::Code(_) => "the device refused the command",
            }));
        }

        // The data step. A stall here is the device saying "no more":
        // clear it, then read the CSW.
        let mut moved = 0;
        if length > 0 {
            let dci = if matches!(data, Data::In(_)) { self.in_dci } else { self.out_dci };
            match controller.bulk(self.slot, dci, layout::STORAGE_DATA, length) {
                Ok(n) => moved = n,
                Err(Failure::Code(xhci::code::STALL)) => self.clear_stall(controller, dci)?,
                Err(Failure::Code(_)) | Err(Failure::TimedOut) => {
                    self.reset_recovery(controller);
                    return Err(Error::Transport("the data did not move"));
                }
            }
        }

        // The CSW - once more if its endpoint stalled the first time.
        let csw_at = layout::STORAGE_CSW;
        let mut got = controller.bulk(self.slot, self.in_dci, csw_at, CSW_LENGTH);
        if got == Err(Failure::Code(xhci::code::STALL)) {
            self.clear_stall(controller, self.in_dci)?;
            got = controller.bulk(self.slot, self.in_dci, csw_at, CSW_LENGTH);
        }
        let csw = controller.dma_read(csw_at, CSW_LENGTH);
        let valid = got == Ok(CSW_LENGTH)
            && u32::from_le_bytes([csw[0], csw[1], csw[2], csw[3]]) == CSW_SIGNATURE
            && u32::from_le_bytes([csw[4], csw[5], csw[6], csw[7]]) == tag
            && csw[12] <= 2;
        if !valid {
            self.reset_recovery(controller);
            return Err(Error::Transport("the device's status was not one"));
        }
        Ok((moved, csw[12]))
    }

    /// Why the last command failed: REQUEST SENSE's fixed-format answer.
    fn sense<B: Bus>(&mut self, controller: &mut Controller<B>) -> Error {
        match self.exchange(controller, &[0x03, 0, 0, 0, 18, 0], Data::In(18)) {
            Ok((n, 0)) if n >= 14 => {
                let s = controller.storage_data(14);
                Error::Failed { key: s[2] & 0x0F, code: s[12], qualifier: s[13] }
            }
            _ => Error::Failed { key: 0, code: 0, qualifier: 0 },
        }
    }

    /// A stalled endpoint: reset on the controller (if the controller
    /// halted it), then cleared on the device - which also starts the
    /// device's packet sequence again, as Reset Endpoint does the
    /// controller's.
    fn clear_stall<B: Bus>(&mut self, controller: &mut Controller<B>, dci: u8) -> Result<(), Error> {
        let address = if dci == self.in_dci { self.in_address } else { self.out_address };
        if controller.endpoint_halted(self.slot, dci) {
            controller.recover_endpoint(self.slot, dci);
        }
        controller
            .control(self.slot, crate::descriptor::clear_halt(address), &[])
            .map(|_| ())
            .map_err(|_| Error::Transport("the device would not clear a stalled endpoint"))
    }

    /// The Bulk-Only reset, and both endpoints cleared: the device is
    /// back where a new command can start.
    fn reset_recovery<B: Bus>(&mut self, controller: &mut Controller<B>) {
        let _ = controller.control(self.slot, crate::descriptor::bulk_only_reset(self.interface), &[]);
        for dci in [self.in_dci, self.out_dci] {
            let _ = self.clear_stall(controller, dci);
        }
    }
}
