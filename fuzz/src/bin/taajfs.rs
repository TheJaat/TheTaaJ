//! TaajFS (taajfs/src/): a disk is untrusted - it may come from anywhere,
//! or have been damaged. Raw 4-KiB blocks are too big for a fuzzer to
//! find a valid volume in, so each input changes a valid one: a volume
//! made fresh, with a few files and a directory, then edits from the
//! input - (block, offset, byte), five bytes each - then mounted and
//! walked: every file listed, looked up and read.
#![no_main]
use taajfs::device::{BlockDevice, MemDevice};
use taajfs::format::BLOCK_SIZE;
use taajfs::volume::{format, Volume};

const BLOCKS: u32 = 64;

fn volume() -> MemDevice {
    let mut device = MemDevice::new(BLOCKS);
    format(&mut device, BLOCKS).expect("format");
    let mut volume = Volume::mount(device).expect("mount");
    volume.put("hello", b"Hello from TaajFS\n").expect("put");
    volume.make_directory("docs").expect("mkdir");
    volume.put("docs/long", &[7u8; 3 * BLOCK_SIZE + 100]).expect("put");
    volume.into_device()
}

fn walk<D: BlockDevice>(volume: &mut Volume<D>, path: &str, depth: u32) {
    let Ok(files) = (if path.is_empty() { volume.list() } else { volume.list_in(path) }) else { return };
    for file in files.into_iter().take(64) {
        let full = if path.is_empty() { file.name.clone() } else { format!("{path}/{}", file.name) };
        let _ = volume.lookup(&full);
        let _ = volume.stamp(&file);
        let _ = volume.time(&file);
        let _ = volume.read_all(&file);
        if depth < 4 {
            walk(volume, &full, depth + 1);
        }
    }
}

fuzz::fuzz_target!(|data: &[u8]| {
    let mut device = volume();
    for edit in data.chunks_exact(5) {
        let block = u16::from_le_bytes([edit[0], edit[1]]) as usize % BLOCKS as usize;
        let offset = u16::from_le_bytes([edit[2], edit[3]]) as usize % BLOCK_SIZE;
        device.blocks[block][offset] = edit[4];
    }
    let Ok(mut volume) = Volume::mount(device) else { return 0 };
    let _ = volume.free_blocks();
    walk(&mut volume, "", 0);
    // Writing to a damaged volume must not panic either.
    let _ = volume.put("new", b"written after mounting");
    let _ = volume.delete("hello");
});
