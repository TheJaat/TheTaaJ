//! A ring of bytes waiting to be written to the debug log.
//!
//! The debug log is a serial line, and a serial line is slow: at
//! 115200 baud a byte takes 87 microseconds, a screenful of text a
//! quarter of a second. A program that prints must not wait for that,
//! and the kernel must not sit with interrupts off while it happens.
//!
//! So what programs print goes in here, and comes out when the CPU has
//! nothing better to do. The ring has a fixed size - it is the kernel's
//! memory, and a program must not be able to use it up - so it can fill.
//! When it does, a message is dropped: whole, never cut in the middle,
//! and counted, so the log can say that something is missing rather than
//! quietly pretend it is complete.
//!
//! The bookkeeping only; the lock around it, and the serial port after
//! it, are the kernel's (console.rs).

pub struct LogRing<const N: usize> {
    bytes: [u8; N],
    /// Index of the oldest byte.
    head: usize,
    length: usize,
    /// Bytes of dropped messages since `take_dropped`.
    dropped: usize,
}

impl<const N: usize> LogRing<N> {
    pub const fn new() -> LogRing<N> {
        LogRing { bytes: [0; N], head: 0, length: 0, dropped: 0 }
    }

    pub fn len(&self) -> usize {
        self.length
    }

    pub fn is_empty(&self) -> bool {
        self.length == 0
    }

    /// Adds `message` - all of it, or (if it does not fit) none of it.
    /// Returns whether it went in.
    pub fn push(&mut self, message: &[u8]) -> bool {
        if message.len() > N - self.length {
            self.dropped += message.len();
            return false;
        }
        let tail = (self.head + self.length) % N;
        let first = message.len().min(N - tail);
        self.bytes[tail..tail + first].copy_from_slice(&message[..first]);
        self.bytes[..message.len() - first].copy_from_slice(&message[first..]);
        self.length += message.len();
        true
    }

    /// Takes the oldest bytes, up to `out.len()`; returns how many.
    pub fn pop(&mut self, out: &mut [u8]) -> usize {
        let count = out.len().min(self.length);
        let first = count.min(N - self.head);
        out[..first].copy_from_slice(&self.bytes[self.head..self.head + first]);
        out[first..count].copy_from_slice(&self.bytes[..count - first]);
        self.head = (self.head + count) % N;
        self.length -= count;
        count
    }

    /// How many bytes were dropped since this was last asked; forgets
    /// the count.
    pub fn take_dropped(&mut self) -> usize {
        core::mem::take(&mut self.dropped)
    }
}

impl<const N: usize> Default for LogRing<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drain<const N: usize>(ring: &mut LogRing<N>, chunk: usize) -> Vec<u8> {
        let mut all = Vec::new();
        let mut piece = vec![0u8; chunk];
        loop {
            let n = ring.pop(&mut piece);
            if n == 0 {
                return all;
            }
            all.extend_from_slice(&piece[..n]);
        }
    }

    #[test]
    fn bytes_come_out_in_the_order_they_went_in() {
        let mut ring: LogRing<16> = LogRing::new();
        assert!(ring.push(b"hello, "));
        assert!(ring.push(b"world"));
        assert_eq!(ring.len(), 12);
        assert_eq!(drain(&mut ring, 5), b"hello, world");
        assert!(ring.is_empty());
    }

    #[test]
    fn it_wraps_around_its_end() {
        let mut ring: LogRing<8> = LogRing::new();
        let mut out = Vec::new();
        // Many more bytes than the ring holds, a few at a time, so the
        // head and tail pass the end over and over.
        for round in 0..50u8 {
            let message = [round, round.wrapping_add(1), round.wrapping_add(2)];
            assert!(ring.push(&message));
            if round % 2 == 1 {
                out.extend(drain(&mut ring, 4));
            }
        }
        out.extend(drain(&mut ring, 4));
        let expected: Vec<u8> = (0..50u8).flat_map(|r| [r, r.wrapping_add(1), r.wrapping_add(2)]).collect();
        assert_eq!(out, expected);
    }

    #[test]
    fn a_message_that_does_not_fit_is_dropped_whole_and_counted() {
        let mut ring: LogRing<8> = LogRing::new();
        assert!(ring.push(b"12345"));
        assert!(!ring.push(b"6789"), "four bytes into three free");
        assert!(!ring.push(b"abcdefghi"), "more than the ring holds at all");
        assert!(ring.push(b"678"), "exactly what is free");
        assert_eq!(ring.take_dropped(), 13);
        assert_eq!(ring.take_dropped(), 0, "counted once");
        assert_eq!(drain(&mut ring, 3), b"12345678", "nothing of the dropped messages got in");
    }

    #[test]
    fn an_empty_message_and_an_empty_pop_are_harmless() {
        let mut ring: LogRing<4> = LogRing::new();
        assert!(ring.push(b""));
        assert_eq!(ring.pop(&mut []), 0);
        assert!(ring.push(b"abcd"));
        assert_eq!(ring.pop(&mut []), 0);
        assert_eq!(drain(&mut ring, 8), b"abcd");
    }
}
