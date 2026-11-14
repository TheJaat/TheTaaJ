//! Capabilities: a process's handle table.
//!
//! In a capability system, holding a handle *is* the permission. There is
//! no global namespace to look things up in and no access-control list to
//! consult: if a process has a handle to a channel with the SEND right,
//! it may send on it; if it has no handle, the channel does not exist as
//! far as it is concerned. Authority flows only along handles, so who can
//! do what is visible in who holds what.
//!
//! A handle is a number naming a slot in the holding process's own
//! table. Three properties make that safe:
//!
//!   - unforgeable: the number means something only in this table, and
//!     is checked on every use. Guessing a number gets InvalidHandle, or
//!     at worst one of the process's own handles - never someone else's.
//!   - no reuse confusion: each slot has a generation, bumped when the
//!     slot is freed, and the handle includes it. A closed handle stays
//!     invalid even after its slot is reused for something new - the
//!     "use after close" bug becomes an error, not a silent wrong object.
//!   - rights only shrink: duplicating can drop rights, never add them,
//!     so a process can hand out a weaker version of what it holds.
//!
//! The table is generic over the object type, so its rules are tested
//! here with plain values; the kernel stores its real objects in it.

use alloc::vec::Vec;

use abi::Error;

/// A handle value, as a process sees it: generation << 32 | (slot + 1).
/// Zero is never a handle, so zeroed memory is not mistaken for one.
pub type Handle = u64;

/// A set of `abi::rights` bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rights(pub u32);

impl Rights {
    pub const ALL: Rights = Rights(abi::rights::ALL);

    pub fn contains(self, other: Rights) -> bool {
        self.0 & other.0 == other.0
    }
}

struct Slot<T> {
    generation: u32,
    entry: Option<(T, Rights)>,
}

pub struct CapTable<T> {
    slots: Vec<Slot<T>>,
    /// Free slot numbers, reused most recently freed first.
    free: Vec<u32>,
    used: usize,
    limit: usize,
}

impl<T> CapTable<T> {
    pub fn new(limit: usize) -> CapTable<T> {
        assert!(limit < u32::MAX as usize);
        CapTable { slots: Vec::new(), free: Vec::new(), used: 0, limit }
    }

    pub fn len(&self) -> usize {
        self.used
    }

    pub fn is_empty(&self) -> bool {
        self.used == 0
    }

    /// How many more handles fit.
    pub fn room(&self) -> usize {
        self.limit - self.used
    }

    fn encode(slot: u32, generation: u32) -> Handle {
        (u64::from(generation) << 32) | (u64::from(slot) + 1)
    }

    /// The slot a handle names, if its generation is current and the slot
    /// is in use.
    fn slot(&self, handle: Handle) -> Option<usize> {
        let index = (handle as u32).checked_sub(1)? as usize;
        let slot = self.slots.get(index)?;
        (slot.generation == (handle >> 32) as u32 && slot.entry.is_some()).then_some(index)
    }

    /// Stores `object`; returns its new handle.
    pub fn insert(&mut self, object: T, rights: Rights) -> Result<Handle, Error> {
        if self.used == self.limit {
            return Err(Error::TooManyHandles);
        }
        let index = match self.free.pop() {
            Some(index) => index,
            None => {
                self.slots.push(Slot { generation: 0, entry: None });
                (self.slots.len() - 1) as u32
            }
        };
        let slot = &mut self.slots[index as usize];
        slot.entry = Some((object, rights));
        self.used += 1;
        Ok(Self::encode(index, slot.generation))
    }

    /// The object behind `handle`, if the handle has every right in
    /// `needed`.
    pub fn get(&self, handle: Handle, needed: Rights) -> Result<&T, Error> {
        let index = self.slot(handle).ok_or(Error::InvalidHandle)?;
        let (object, rights) = self.slots[index].entry.as_ref().expect("checked");
        if !rights.contains(needed) {
            return Err(Error::AccessDenied);
        }
        Ok(object)
    }

    /// The rights `handle` carries.
    pub fn rights(&self, handle: Handle) -> Result<Rights, Error> {
        let index = self.slot(handle).ok_or(Error::InvalidHandle)?;
        Ok(self.slots[index].entry.as_ref().expect("checked").1)
    }

    /// Removes `handle` from the table, returning what it held. The
    /// handle - and any copy of its number - is dead from now on.
    pub fn remove(&mut self, handle: Handle) -> Result<(T, Rights), Error> {
        let index = self.slot(handle).ok_or(Error::InvalidHandle)?;
        let slot = &mut self.slots[index];
        let entry = slot.entry.take().expect("checked");
        slot.generation = slot.generation.wrapping_add(1);
        self.free.push(index as u32);
        self.used -= 1;
        Ok(entry)
    }

    /// Removes several handles at once - all of them, or none if any
    /// fails a check. Each needs `needed`, may appear only once, and must
    /// pass `allowed`. This is what sending handles in a message uses: a
    /// half-moved set of handles would be lost or duplicated authority.
    pub fn remove_all(
        &mut self,
        handles: &[Handle],
        needed: Rights,
        allowed: impl Fn(&T) -> bool,
    ) -> Result<Vec<(T, Rights)>, Error> {
        for (n, &handle) in handles.iter().enumerate() {
            if !allowed(self.get(handle, needed)?) {
                return Err(Error::InvalidArgument);
            }
            if handles[..n].contains(&handle) {
                return Err(Error::InvalidArgument);
            }
        }
        Ok(handles.iter().map(|&h| self.remove(h).expect("checked")).collect())
    }

    /// Takes every object out - for a process that is ending.
    pub fn take_all(&mut self) -> Vec<T> {
        let objects = self
            .slots
            .iter_mut()
            .filter_map(|slot| {
                let (object, _) = slot.entry.take()?;
                slot.generation = slot.generation.wrapping_add(1);
                Some(object)
            })
            .collect();
        self.free = (0..self.slots.len() as u32).rev().collect();
        self.used = 0;
        objects
    }
}

impl<T: Clone> CapTable<T> {
    /// A second handle to the same object, with `rights`. The original
    /// needs DUPLICATE, and `rights` must be a subset of its rights.
    pub fn duplicate(&mut self, handle: Handle, rights: Rights) -> Result<Handle, Error> {
        let index = self.slot(handle).ok_or(Error::InvalidHandle)?;
        let (object, held) = self.slots[index].entry.as_ref().expect("checked");
        if !held.contains(Rights(abi::rights::DUPLICATE)) || !held.contains(rights) {
            return Err(Error::AccessDenied);
        }
        let object = object.clone();
        self.insert(object, rights)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use abi::rights::*;

    fn table() -> CapTable<&'static str> {
        CapTable::new(8)
    }

    #[test]
    fn a_handle_gives_back_its_object() {
        let mut t = table();
        let a = t.insert("a", Rights::ALL).unwrap();
        let b = t.insert("b", Rights::ALL).unwrap();
        assert_ne!(a, b);
        assert_eq!(t.get(a, Rights(SEND)), Ok(&"a"));
        assert_eq!(t.get(b, Rights::ALL), Ok(&"b"));
        assert_eq!(t.len(), 2);
    }

    /// The ABI promises startup handles are 1, 2, ... n: a fresh table
    /// must number its first entries that way.
    #[test]
    fn a_fresh_table_numbers_handles_from_one() {
        let mut t = table();
        let handles: Vec<_> = ["a", "b", "c"].iter().map(|&o| t.insert(o, Rights::ALL).unwrap()).collect();
        assert_eq!(handles, vec![1, 2, 3]);
        assert_eq!(t.rights(2), Ok(Rights::ALL));
    }

    #[test]
    fn made_up_numbers_are_not_handles() {
        let mut t = table();
        let a = t.insert("a", Rights::ALL).unwrap();
        for forged in [0, a + 1, a | 0xFF << 32, u64::MAX, 1 << 40] {
            assert_eq!(t.get(forged, Rights(0)), Err(Error::InvalidHandle), "{forged:#x}");
        }
    }

    #[test]
    fn a_closed_handle_stays_dead_when_its_slot_is_reused() {
        let mut t = table();
        let old = t.insert("old", Rights::ALL).unwrap();
        t.remove(old).unwrap();
        let new = t.insert("new", Rights::ALL).unwrap();
        assert_eq!(old as u32, new as u32, "same slot");
        assert_ne!(old, new, "different generation");
        assert_eq!(t.get(old, Rights(0)), Err(Error::InvalidHandle));
        assert_eq!(t.remove(old), Err(Error::InvalidHandle));
        assert_eq!(t.get(new, Rights(0)), Ok(&"new"));
    }

    #[test]
    fn rights_are_checked_and_only_ever_narrow() {
        let mut t = table();
        let full = t.insert("chan", Rights::ALL).unwrap();
        let receive_only = t.duplicate(full, Rights(RECEIVE)).unwrap();
        assert_eq!(t.get(receive_only, Rights(RECEIVE)), Ok(&"chan"));
        assert_eq!(t.get(receive_only, Rights(SEND)), Err(Error::AccessDenied));
        // No DUPLICATE right: cannot copy, let alone widen.
        assert_eq!(t.duplicate(receive_only, Rights(RECEIVE)), Err(Error::AccessDenied));
        // With DUPLICATE, still no widening beyond what is held.
        let dup_receive = t.duplicate(full, Rights(DUPLICATE | RECEIVE)).unwrap();
        assert_eq!(t.duplicate(dup_receive, Rights(SEND)), Err(Error::AccessDenied));
        assert!(t.duplicate(dup_receive, Rights(RECEIVE)).is_ok());
    }

    #[test]
    fn the_table_has_a_limit() {
        let mut t = table();
        let handles: Vec<_> = (0..8).map(|_| t.insert("x", Rights::ALL).unwrap()).collect();
        assert_eq!(t.insert("one too many", Rights::ALL), Err(Error::TooManyHandles));
        assert_eq!(t.duplicate(handles[0], Rights::ALL), Err(Error::TooManyHandles));
        t.remove(handles[3]).unwrap();
        assert_eq!(t.room(), 1);
        assert!(t.insert("fits again", Rights::ALL).is_ok());
    }

    #[test]
    fn moving_several_handles_is_all_or_nothing() {
        let mut t = table();
        let a = t.insert("a", Rights::ALL).unwrap();
        let b = t.insert("b", Rights::ALL).unwrap();
        let weak = t.insert("weak", Rights(SEND)).unwrap();
        let transfer = Rights(TRANSFER);

        // One bad handle, one without TRANSFER, a duplicate, a refused
        // object: each fails, and every handle is still there after.
        assert_eq!(t.remove_all(&[a, 12345], transfer, |_| true), Err(Error::InvalidHandle));
        assert_eq!(t.remove_all(&[a, weak], transfer, |_| true), Err(Error::AccessDenied));
        assert_eq!(t.remove_all(&[a, a], transfer, |_| true), Err(Error::InvalidArgument));
        assert_eq!(t.remove_all(&[a, b], transfer, |o| *o != "b"), Err(Error::InvalidArgument));
        assert_eq!(t.len(), 3);

        let moved = t.remove_all(&[b, a], transfer, |_| true).unwrap();
        assert_eq!(moved, vec![("b", Rights::ALL), ("a", Rights::ALL)]);
        assert_eq!(t.len(), 1);
        assert_eq!(t.get(a, Rights(0)), Err(Error::InvalidHandle));
    }

    #[test]
    fn take_all_empties_the_table_and_kills_every_handle() {
        let mut t = table();
        let handles: Vec<_> = ["a", "b", "c"].iter().map(|&o| t.insert(o, Rights::ALL).unwrap()).collect();
        t.remove(handles[1]).unwrap();
        let mut taken = t.take_all();
        taken.sort();
        assert_eq!(taken, vec!["a", "c"]);
        assert!(t.is_empty());
        assert_eq!(t.room(), 8);
        for h in handles {
            assert_eq!(t.get(h, Rights(0)), Err(Error::InvalidHandle));
        }
        let fresh: Vec<_> = (0..8).map(|_| t.insert("y", Rights::ALL).unwrap()).collect();
        assert_eq!(fresh.len(), 8);
    }
}
