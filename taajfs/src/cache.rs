//! A block cache: recently used blocks kept in memory, in front of a
//! slower device.
//!
//! A file system reads the same few blocks over and over - the
//! superblock's tree, the directory, a file's inode - for every request
//! it serves. With a disk behind it, each of those reads is a round trip
//! to the driver and the device. The cache answers them from memory.
//!
//! Two rules keep it honest:
//!
//!   - Reads: a block is in the cache only if it was read from the
//!     device, or written to it successfully. So what the cache says is
//!     always what the device holds.
//!   - Writes go through at once ("write-through"): to the device first,
//!     and into the cache only if the device took it. Nothing waits in
//!     memory to be written later. That costs speed - a cache that held
//!     writes back could merge and reorder them - but TaajFS's promise
//!     depends on the order: every block of a change is on the disk
//!     before the superblock that points to them. A cache that delayed
//!     writes would have to know that order too. This one does not need
//!     to: it never changes when a write happens, only saves reads.
//!
//! This is safe only while nothing else writes the device. In TheTaaJ
//! the file service is the only process holding the disk's read-write
//! channel, so it is.
//!
//! When full, the block used longest ago makes room (LRU: least
//! recently used). Finding it scans every entry - fine for a few
//! hundred, and far cheaper than the disk read it saves.

use alloc::boxed::Box;
use alloc::collections::BTreeMap;

use crate::device::{BlockDevice, DeviceError};
use crate::format::Block;

struct Entry {
    data: Box<Block>,
    /// The cache's clock when this block was last read or written.
    used: u64,
}

pub struct Cache<D: BlockDevice> {
    device: D,
    blocks: BTreeMap<u32, Entry>,
    capacity: usize,
    clock: u64,
    hits: u64,
    misses: u64,
}

impl<D: BlockDevice> Cache<D> {
    /// A cache of up to `capacity` blocks (at least one) over `device`.
    pub fn new(device: D, capacity: usize) -> Cache<D> {
        Cache { device, blocks: BTreeMap::new(), capacity: capacity.max(1), clock: 0, hits: 0, misses: 0 }
    }

    /// Reads answered from memory, and reads that went to the device.
    pub fn stats(&self) -> (u64, u64) {
        (self.hits, self.misses)
    }

    pub fn device(&self) -> &D {
        &self.device
    }

    /// Drops the cache, handing the device back.
    pub fn into_device(self) -> D {
        self.device
    }

    /// Remembers `data` as block `number`, evicting the least recently
    /// used block if the cache is full.
    fn remember(&mut self, number: u32, data: &Block) {
        self.clock += 1;
        if let Some(entry) = self.blocks.get_mut(&number) {
            *entry.data = *data;
            entry.used = self.clock;
            return;
        }
        if self.blocks.len() >= self.capacity {
            let oldest = self.blocks.iter().min_by_key(|(_, entry)| entry.used).map(|(&number, _)| number);
            if let Some(oldest) = oldest {
                self.blocks.remove(&oldest);
            }
        }
        self.blocks.insert(number, Entry { data: Box::new(*data), used: self.clock });
    }
}

impl<D: BlockDevice> BlockDevice for Cache<D> {
    fn read_block(&mut self, number: u32, buffer: &mut Block) -> Result<(), DeviceError> {
        if let Some(entry) = self.blocks.get_mut(&number) {
            self.clock += 1;
            entry.used = self.clock;
            *buffer = *entry.data;
            self.hits += 1;
            return Ok(());
        }
        self.device.read_block(number, buffer)?;
        self.misses += 1;
        self.remember(number, buffer);
        Ok(())
    }

    /// Blocks already here come from memory; each run of the others is
    /// one read of the device - and is not kept: a run is a file's data,
    /// read once, and keeping it would push out the blocks every request
    /// needs (the tree, the directories, the inodes).
    fn read_blocks(&mut self, first: u32, buffer: &mut [u8]) -> Result<(), DeviceError> {
        let size = core::mem::size_of::<Block>();
        let count = buffer.len() / size;
        let mut n = 0;
        while n < count {
            if let Some(entry) = self.blocks.get_mut(&(first + n as u32)) {
                self.clock += 1;
                entry.used = self.clock;
                buffer[n * size..(n + 1) * size].copy_from_slice(&entry.data[..]);
                self.hits += 1;
                n += 1;
                continue;
            }
            let mut end = n + 1;
            while end < count && !self.blocks.contains_key(&(first + end as u32)) {
                end += 1;
            }
            self.device.read_blocks(first + n as u32, &mut buffer[n * size..end * size])?;
            self.misses += (end - n) as u64;
            n = end;
        }
        Ok(())
    }

    fn write_block(&mut self, number: u32, buffer: &Block) -> Result<(), DeviceError> {
        match self.device.write_block(number, buffer) {
            Ok(()) => {
                self.remember(number, buffer);
                Ok(())
            }
            Err(error) => {
                // What the device holds now is unknown - the old block,
                // the new, or part of each. Forget it: the next read
                // asks the device.
                self.blocks.remove(&number);
                Err(error)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::MemDevice;
    use crate::format::{empty_block, BLOCK_SIZE};
    use crate::volume::{format, Volume};

    /// Counts what reaches the device underneath.
    struct Counting {
        inner: MemDevice,
        reads: usize,
        writes: usize,
        /// Reads of several blocks at once, each one request.
        runs: usize,
    }

    impl Counting {
        fn new(blocks: u32) -> Counting {
            Counting { inner: MemDevice::new(blocks), reads: 0, writes: 0, runs: 0 }
        }
    }

    impl BlockDevice for Counting {
        fn read_blocks(&mut self, first: u32, buffer: &mut [u8]) -> Result<(), DeviceError> {
            self.runs += 1;
            self.inner.read_blocks(first, buffer)
        }
        fn read_block(&mut self, number: u32, buffer: &mut Block) -> Result<(), DeviceError> {
            self.reads += 1;
            self.inner.read_block(number, buffer)
        }
        fn write_block(&mut self, number: u32, buffer: &Block) -> Result<(), DeviceError> {
            self.writes += 1;
            self.inner.write_block(number, buffer)
        }
    }

    fn filled(byte: u8) -> Block {
        [byte; BLOCK_SIZE]
    }

    #[test]
    fn a_run_of_blocks_is_one_request_for_each_stretch_the_cache_lacks() {
        let mut cache = Cache::new(Counting::new(16), 8);
        for n in 0..16 {
            cache.write_block(n, &filled(n as u8)).unwrap();
        }
        let mut cache = Cache::new(cache.into_device(), 8);
        let mut block = empty_block();
        cache.read_block(5, &mut block).unwrap(); // 5 is now kept
        let before = cache.device().reads;

        let mut run = alloc::vec![0u8; 10 * BLOCK_SIZE];
        cache.read_blocks(2, &mut run).unwrap(); // 2..12
        for n in 0..10 {
            assert!(run[n * BLOCK_SIZE..(n + 1) * BLOCK_SIZE].iter().all(|&b| b == (n + 2) as u8));
        }
        // 2..5 and 6..12: two requests, and block 5 from memory.
        assert_eq!(cache.device().runs, 2);
        assert_eq!(cache.device().reads, before);
        assert_eq!(cache.stats(), (1, 10));
        // A run is not kept: block 3 is read from the device again.
        cache.read_block(3, &mut block).unwrap();
        assert_eq!(cache.stats().1, 11);
    }

    #[test]
    fn a_file_is_read_in_runs() {
        let mut device = Counting::new(256);
        format(&mut device, 256).unwrap();
        let mut volume = Volume::mount(device).unwrap();
        let data: alloc::vec::Vec<u8> = (0..100 * BLOCK_SIZE).map(|n| (n % 251) as u8).collect();
        volume.put("big", &data).unwrap();
        let mut volume = Volume::mount(Cache::new(volume.into_device(), 16)).unwrap();
        let file = volume.lookup("big").unwrap();
        assert_eq!(volume.read_all(&file).unwrap(), data);
        let device = volume.into_device().into_device();
        // 100 blocks, written one after another: a few runs, not 100 reads.
        assert!(device.runs <= 4, "{} runs", device.runs);
    }

    #[test]
    fn a_second_read_does_not_reach_the_device() {
        let mut cache = Cache::new(Counting::new(8), 4);
        cache.device.inner.blocks[3] = filled(7);
        let mut block = empty_block();
        cache.read_block(3, &mut block).unwrap();
        assert_eq!(block, filled(7));
        block = empty_block();
        cache.read_block(3, &mut block).unwrap();
        assert_eq!(block, filled(7));
        assert_eq!(cache.device().reads, 1);
        assert_eq!(cache.stats(), (1, 1));
    }

    #[test]
    fn writes_go_through_at_once_and_are_read_back_from_memory() {
        let mut cache = Cache::new(Counting::new(8), 4);
        cache.write_block(2, &filled(9)).unwrap();
        assert_eq!(cache.device().inner.blocks[2], filled(9), "on the device before write_block returned");
        let mut block = empty_block();
        cache.read_block(2, &mut block).unwrap();
        assert_eq!(block, filled(9));
        assert_eq!((cache.device().reads, cache.device().writes), (0, 1));
        // Overwriting a cached block replaces what is remembered.
        cache.write_block(2, &filled(10)).unwrap();
        cache.read_block(2, &mut block).unwrap();
        assert_eq!(block, filled(10));
    }

    #[test]
    fn the_least_recently_used_block_makes_room() {
        let mut cache = Cache::new(Counting::new(8), 3);
        let mut block = empty_block();
        for number in [0, 1, 2] {
            cache.read_block(number, &mut block).unwrap();
        }
        cache.read_block(0, &mut block).unwrap(); // 0 is fresh again; 1 is now the oldest
        cache.read_block(3, &mut block).unwrap(); // evicts 1
        assert_eq!(cache.device().reads, 4);
        for number in [0, 2, 3] {
            cache.read_block(number, &mut block).unwrap();
        }
        assert_eq!(cache.device().reads, 4, "0, 2 and 3 were still remembered");
        cache.read_block(1, &mut block).unwrap();
        assert_eq!(cache.device().reads, 5, "1 was not");
        assert!(cache.blocks.len() <= 3);
    }

    #[test]
    fn errors_pass_through_and_a_failed_write_is_forgotten() {
        let mut cache = Cache::new(MemDevice::new(4), 4);
        let mut block = empty_block();
        assert!(cache.read_block(99, &mut block).is_err());
        assert_eq!(cache.stats(), (0, 0), "a failed read is not remembered");

        cache.write_block(1, &filled(1)).unwrap();
        cache.device.writes_left = Some(0);
        assert!(cache.write_block(1, &filled(2)).is_err());
        cache.read_block(1, &mut block).unwrap();
        assert_eq!(block, filled(1), "asked the device, which still has the old block");
    }

    #[test]
    fn a_volume_works_the_same_through_a_cache_with_far_fewer_reads() {
        fn exercise<D: BlockDevice>(volume: &mut Volume<D>) -> alloc::vec::Vec<alloc::vec::Vec<u8>> {
            for n in 0..20u8 {
                volume.put(&alloc::format!("file{n}"), &[n; 5000]).unwrap();
            }
            (0..20u8)
                .map(|n| {
                    let file = volume.lookup(&alloc::format!("file{n}")).unwrap();
                    volume.read_all(&file).unwrap()
                })
                .collect()
        }
        let run = |capacity: Option<usize>| {
            let mut device = Counting::new(256);
            format(&mut device, 256).unwrap();
            let before = device.reads;
            let (contents, reads) = match capacity {
                Some(capacity) => {
                    let mut volume = Volume::mount(Cache::new(device, capacity)).unwrap();
                    (exercise(&mut volume), volume.into_device().into_device().reads)
                }
                None => {
                    let mut volume = Volume::mount(device).unwrap();
                    (exercise(&mut volume), volume.into_device().reads)
                }
            };
            (contents, reads - before)
        };
        let (plain, plain_reads) = run(None);
        let (cached, cached_reads) = run(Some(64));
        assert_eq!(plain, cached, "the same files either way");
        assert!(cached_reads * 5 < plain_reads, "{cached_reads} reads with the cache, {plain_reads} without");
    }

    #[test]
    fn after_a_power_cut_the_cache_holds_nothing_the_disk_does_not() {
        // Cut the power after every possible number of writes. Whatever
        // the cache still remembers must be exactly what is on the
        // device - or a remount would see a different disk than the
        // running system believed in.
        let mut base = MemDevice::new(128);
        format(&mut base, 128).unwrap();
        for allowed in 0.. {
            let device = MemDevice { blocks: base.blocks.clone(), writes_left: Some(allowed) };
            let mut volume = Volume::mount(Cache::new(device, 16)).unwrap();
            let done = volume.put("a", &[1; 9000]).is_ok() && volume.put("b", &[2; 300]).is_ok() && volume.delete("a").is_ok();
            let cache = volume.into_device();
            for (number, entry) in &cache.blocks {
                assert!(
                    *entry.data == cache.device.blocks[*number as usize],
                    "after {allowed} writes: cached block {number} differs from the disk"
                );
            }
            let mut device = cache.into_device();
            device.writes_left = None;
            Volume::mount(device).expect("always mountable");
            if done {
                break;
            }
        }
    }
}
