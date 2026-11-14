//! A lock-free queue between exactly one producer and one consumer.
//!
//! The keyboard's interrupt handler produces bytes; the kernel's main
//! loop consumes them. A lock between the two would be a trap: if the
//! main loop held it when the interrupt arrived, the handler would spin
//! forever waiting for code that cannot run until the handler returns.
//!
//! So no lock. A ring of N slots with two counters: the producer only
//! ever writes `tail`, the consumer only ever writes `head`. Each reads
//! the other's counter to know how far it may go. The orderings carry
//! the data: the producer's Release store of `tail` happens after it
//! wrote the slot, and the consumer's Acquire load of `tail` happens
//! before it reads the slot - so a consumer that sees the new `tail`
//! also sees the byte. The same pairing on `head` hands slots back.
//!
//! The counters run freely and wrap; `tail - head` is the number of
//! items even across the wrap, because the subtraction wraps too.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicUsize, Ordering};

pub struct Queue<const N: usize> {
    slots: UnsafeCell<[u8; N]>,
    head: AtomicUsize, // next slot to read
    tail: AtomicUsize, // next slot to write
}

// SAFETY: one producer and one consumer, each touching only the slots
// the counters give it, as described above. Callers must keep to one of
// each - which `push` and `pop` document.
unsafe impl<const N: usize> Sync for Queue<N> {}

impl<const N: usize> Queue<N> {
    pub const fn new() -> Queue<N> {
        Queue { slots: UnsafeCell::new([0; N]), head: AtomicUsize::new(0), tail: AtomicUsize::new(0) }
    }

    /// Adds a byte; false (and the byte is dropped) if the queue is full.
    /// Only ever called by the single producer.
    pub fn push(&self, byte: u8) -> bool {
        let tail = self.tail.load(Ordering::Relaxed); // only we write it
        let head = self.head.load(Ordering::Acquire);
        if tail.wrapping_sub(head) == N {
            return false;
        }
        // SAFETY: slot `tail % N` is outside head..tail, so the consumer
        // is not reading it.
        unsafe { (*self.slots.get())[tail % N] = byte };
        self.tail.store(tail.wrapping_add(1), Ordering::Release);
        true
    }

    /// Takes the oldest byte, if any. Only ever called by the single
    /// consumer.
    pub fn pop(&self) -> Option<u8> {
        let head = self.head.load(Ordering::Relaxed); // only we write it
        let tail = self.tail.load(Ordering::Acquire);
        if head == tail {
            return None;
        }
        // SAFETY: slot `head % N` is inside head..tail: written, and the
        // producer will not touch it until `head` moves past.
        let byte = unsafe { (*self.slots.get())[head % N] };
        self.head.store(head.wrapping_add(1), Ordering::Release);
        Some(byte)
    }

    pub fn len(&self) -> usize {
        self.tail.load(Ordering::Acquire).wrapping_sub(self.head.load(Ordering::Acquire))
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl<const N: usize> Default for Queue<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn first_in_first_out_and_full() {
        let q: Queue<4> = Queue::new();
        assert_eq!(q.pop(), None);
        for b in 1..=4 {
            assert!(q.push(b));
        }
        assert!(!q.push(5), "full");
        assert_eq!(q.len(), 4);
        assert_eq!(q.pop(), Some(1));
        assert!(q.push(5), "room again");
        assert_eq!((2..=5).map(|_| q.pop().unwrap()).collect::<Vec<_>>(), vec![2, 3, 4, 5]);
        assert!(q.is_empty());
    }

    /// A producer thread and a consumer thread, a million bytes, a tiny
    /// queue: every byte must arrive, once, in order.
    #[test]
    fn concurrent_producer_and_consumer() {
        const COUNT: usize = 1_000_000;
        let q: Arc<Queue<16>> = Arc::new(Queue::new());
        let producer = {
            let q = Arc::clone(&q);
            thread::spawn(move || {
                for i in 0..COUNT {
                    while !q.push(i as u8) {
                        std::hint::spin_loop();
                    }
                }
            })
        };
        for i in 0..COUNT {
            let byte = loop {
                if let Some(b) = q.pop() {
                    break b;
                }
                std::hint::spin_loop();
            };
            assert_eq!(byte, i as u8, "byte {i} out of order");
        }
        producer.join().unwrap();
        assert!(q.is_empty());
    }
}
