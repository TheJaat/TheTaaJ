//! USB sticks: the Bulk-Only Transport and SCSI, against `sim::Disk`.

use super::sim::{make, Sim};
use super::start;
use crate::device::{self, Attached};
use crate::storage::{Error, Storage, SECTOR};
use crate::xhci::{layout, trb, Controller};
use std::vec::Vec;

/// A controller with `stick` on port 1 (a USB 2 port), attached.
fn with(stick: super::sim::Device) -> (Sim, Controller<Sim>, Attached) {
    let sim = Sim::new(4, 2);
    sim.plug(1, stick);
    let mut controller = start(&sim).expect("it starts");
    let attached = device::attach(&mut controller, 1, true).expect("the stick attaches");
    (sim, controller, attached)
}

fn disk(sim: &Sim) -> std::cell::RefMut<'_, super::sim::Disk> {
    std::cell::RefMut::map(sim.0.borrow_mut(), |s| s.devices[0].disk.as_mut().unwrap())
}

fn pattern(first: u64, count: usize) -> Vec<u8> {
    (0..count * SECTOR as usize).map(|n| ((first as usize * 7 + n * 13) % 251) as u8).collect()
}

fn write(controller: &mut Controller<Sim>, storage: &mut Storage, sector: u64, bytes: &[u8]) -> Result<(), Error> {
    controller.storage_data_mut(bytes.len()).copy_from_slice(bytes);
    storage.transfer(controller, true, sector, (bytes.len() / SECTOR as usize) as u32)
}

fn read(controller: &mut Controller<Sim>, storage: &mut Storage, sector: u64, count: u32) -> Result<Vec<u8>, Error> {
    controller.storage_data_mut(count as usize * SECTOR as usize).fill(0xEE);
    storage.transfer(controller, false, sector, count)?;
    Ok(controller.storage_data(count as usize * SECTOR as usize).to_vec())
}

#[test]
fn a_stick_is_configured_named_and_sized() {
    let (sim, _, attached) = with(make::stick(2048));
    assert!(attached.functions.iter().all(Option::is_none));
    let storage = attached.storage.expect("driven");
    assert_eq!(&storage.vendor, b"TaajSim ");
    assert_eq!(&storage.product, b"Simulated stick ");
    assert_eq!(storage.sectors, 2048);
    assert!(storage.removable && !storage.read_only);
    // INQUIRY; TEST UNIT READY failing with "unit attention", REQUEST
    // SENSE, TEST UNIT READY again; READ CAPACITY(10); MODE SENSE(6).
    assert_eq!(disk(&sim).commands, [0x12, 0x00, 0x03, 0x00, 0x25, 0x1A]);
    // Two bulk endpoints, OUT 0x02 at DCI 4 and IN 0x81 at DCI 3.
    let s = sim.0.borrow();
    assert!(s.commands.iter().any(|&(kind, _)| kind == trb::CONFIGURE_ENDPOINT));
}

#[test]
fn reads_and_writes_up_to_128_kib_each_across_64_kib_boundaries() {
    let (sim, mut controller, attached) = with(make::stick(4096));
    let mut storage = attached.storage.unwrap();
    // The buffer starts 0x6000 short of a 64 KiB boundary: 128 KiB is
    // three TRBs (the sim refuses one that crosses).
    assert_eq!(layout::STORAGE_DATA % 0x10000, 0x6000);
    let bytes = pattern(100, 256);
    write(&mut controller, &mut storage, 100, &bytes).expect("written");
    assert_eq!(disk(&sim).sectors[&100], bytes[..512]);
    assert_eq!(disk(&sim).sectors[&355], bytes[255 * 512..]);
    assert_eq!(read(&mut controller, &mut storage, 100, 256).unwrap(), bytes);
    // One sector, and sectors never written (zeroes).
    assert_eq!(read(&mut controller, &mut storage, 101, 1).unwrap(), bytes[512..1024]);
    assert_eq!(read(&mut controller, &mut storage, 4000, 2).unwrap(), std::vec![0; 1024]);
    // READ(10) and WRITE(10).
    assert!(disk(&sim).commands.ends_with(&[0x2A, 0x28, 0x28, 0x28]));
    // Too many at once, or none.
    assert!(storage.transfer(&mut controller, false, 0, 257).is_err());
    assert!(storage.transfer(&mut controller, false, 0, 0).is_err());
}

#[test]
fn past_the_end_fails_with_the_devices_reason() {
    let (_, mut controller, attached) = with(make::stick(64));
    let mut storage = attached.storage.unwrap();
    let error = read(&mut controller, &mut storage, 63, 2).unwrap_err();
    assert_eq!(error, Error::Failed { key: 0x5, code: 0x21, qualifier: 0 });
    assert_eq!(error.describe(), "illegal request");
    // And the stick is still usable.
    assert!(read(&mut controller, &mut storage, 63, 1).is_ok());
}

#[test]
fn a_write_protected_stick_says_so_and_refuses_writes() {
    let mut stick = make::stick(64);
    stick.disk.as_mut().unwrap().write_protected = true;
    let (sim, mut controller, attached) = with(stick);
    let mut storage = attached.storage.unwrap();
    assert!(storage.read_only);
    let error = write(&mut controller, &mut storage, 0, &pattern(0, 1)).unwrap_err();
    assert_eq!(error.describe(), "write-protected");
    assert!(disk(&sim).sectors.is_empty());
}

#[test]
fn a_stalled_read_is_cleared_and_its_reason_asked() {
    let (sim, mut controller, attached) = with(make::stick(64));
    let mut storage = attached.storage.unwrap();
    disk(&sim).stall_next_data = true;
    let error = read(&mut controller, &mut storage, 0, 4).unwrap_err();
    assert_eq!(error, Error::Failed { key: 0x3, code: 0x11, qualifier: 0 }, "unrecovered read error");
    // The controller's endpoint reset, the device's stall cleared, and
    // no Bulk-Only reset: the CSW came.
    let resets = sim.0.borrow().commands.iter().filter(|&&(k, _)| k == trb::RESET_ENDPOINT).count();
    let halts = disk(&sim).clear_halts;
    let bulk_resets = disk(&sim).bulk_resets;
    assert_eq!((resets, halts, bulk_resets), (1, 1, 0));
    assert!(!controller.endpoint_halted(storage.slot, 3));
    assert_eq!(read(&mut controller, &mut storage, 0, 4).unwrap(), std::vec![0; 2048]);
}

#[test]
fn a_status_that_is_not_one_resets_the_device() {
    let (sim, mut controller, attached) = with(make::stick(64));
    let mut storage = attached.storage.unwrap();
    disk(&sim).garble_next_status = true;
    assert_eq!(read(&mut controller, &mut storage, 0, 1).unwrap_err(), Error::Transport("the device's status was not one"));
    let bulk_resets = disk(&sim).bulk_resets;
    let halts = disk(&sim).clear_halts;
    assert_eq!((bulk_resets, halts), (1, 2), "the reset, and both endpoints cleared");
    assert!(read(&mut controller, &mut storage, 0, 1).is_ok());
}

#[test]
fn a_device_gone_quiet_is_stopped_and_reset() {
    let (sim, mut controller, attached) = with(make::stick(64));
    let mut storage = attached.storage.unwrap();
    disk(&sim).hang_next_data = true;
    let before = sim.0.borrow().time;
    assert_eq!(read(&mut controller, &mut storage, 0, 1).unwrap_err(), Error::Transport("the data did not move"));
    assert!(sim.0.borrow().time - before >= crate::xhci::BULK_WAIT, "waited the bulk time");
    // Its endpoint stopped (not reset: it had not halted), pointed past
    // the TRB that never finished, and the device reset.
    assert_eq!(sim.0.borrow().stops, 1);
    assert_eq!(disk(&sim).bulk_resets, 1);
    assert!(read(&mut controller, &mut storage, 0, 1).is_ok());
}

#[test]
fn a_disk_past_2_to_the_32_sectors_uses_the_16_byte_commands() {
    let (sim, mut controller, attached) = with(make::stick((1 << 32) + 16));
    let mut storage = attached.storage.unwrap();
    assert_eq!(storage.sectors, (1 << 32) + 16);
    assert!(disk(&sim).commands.contains(&0x9E), "READ CAPACITY(16)");
    let bytes = pattern(9, 2);
    write(&mut controller, &mut storage, (1 << 32) + 3, &bytes).unwrap();
    assert_eq!(read(&mut controller, &mut storage, (1 << 32) + 3, 2).unwrap(), bytes);
    // Straddling 2^32 needs them too; below it, the 10-byte ones.
    read(&mut controller, &mut storage, (1 << 32) - 1, 2).unwrap();
    read(&mut controller, &mut storage, 5, 1).unwrap();
    assert!(disk(&sim).commands.ends_with(&[0x8A, 0x88, 0x88, 0x28]));
    assert_eq!(disk(&sim).sectors[&((1 << 32) + 4)], bytes[512..]);
}

#[test]
fn sticks_that_cannot_be_driven_say_why() {
    let mut empty = make::stick(64);
    empty.disk.as_mut().unwrap().no_medium = true;
    let (_, _, attached) = with(empty);
    assert_eq!((attached.storage, attached.storage_problem), (None, Some("no medium in it")));

    let mut big_sectors = make::stick(64);
    big_sectors.disk.as_mut().unwrap().sector_size = 4096;
    let (_, _, attached) = with(big_sectors);
    assert_eq!(attached.storage_problem, Some("its sectors are not 512 bytes"));

    // A second stick, when one is driven already.
    let sim = Sim::new(4, 2);
    sim.plug(1, make::stick(64));
    let mut controller = start(&sim).unwrap();
    let attached = device::attach(&mut controller, 1, false).unwrap();
    assert_eq!(attached.storage_problem, Some("a disk is driven already"));
    assert!(sim.0.borrow().devices[0].configured.is_none(), "left unconfigured");

    // Too little DMA memory for the buffers.
    let sim = Sim::new(4, 2);
    sim.plug(1, make::stick(64));
    sim.0.borrow_mut().memory.truncate(layout::STORAGE_DATA);
    let mut controller = start(&sim).unwrap();
    let attached = device::attach(&mut controller, 1, true).unwrap();
    assert_eq!(attached.storage_problem, Some("no DMA memory for a disk's buffers"));
}

#[test]
fn a_keyboard_and_a_stick_side_by_side() {
    let sim = Sim::new(4, 2);
    let keyboard = sim.plug(1, make::keyboard(3));
    sim.plug(2, make::stick(64));
    let mut controller = start(&sim).unwrap();
    let k = device::attach(&mut controller, 1, true).unwrap();
    let s = device::attach(&mut controller, 2, true).unwrap();
    let mut storage = s.storage.unwrap();
    // A key pressed while the stick is read: the report waits as an
    // event, the read is not confused by it.
    sim.report(keyboard, 0x81, &[0, 0, 4, 0, 0, 0, 0, 0]);
    assert!(read(&mut controller, &mut storage, 0, 8).is_ok());
    let reports = super::reports(&mut controller, &k);
    assert_eq!(reports.len(), 1);
}
