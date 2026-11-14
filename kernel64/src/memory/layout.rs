//! Finding room in physical memory before there is an allocator.
//!
//! The frame allocator needs somewhere to keep its bitmap, and the
//! bitmap has to be placed before the allocator exists. So the very
//! first placement decision is made by hand: the first spot, in memory
//! the firmware calls usable, that nothing else already occupies.

use core::ops::Range;

use super::addr::align_up;

fn overlaps(a: &Range<u64>, b: &Range<u64>) -> bool {
    a.start < b.end && b.start < a.end
}

/// The lowest address `a` such that `a..a + size`:
///   - lies inside one of the `usable` ranges
///   - overlaps none of the `reserved` ranges
///   - is aligned to `align` (a power of two)
///   - ends at or below `limit`
pub fn find_free_space<U>(
    usable: U,
    reserved: &[Range<u64>],
    size: u64,
    align: u64,
    limit: u64,
) -> Option<u64>
where
    U: IntoIterator<Item = Range<u64>>,
{
    for region in usable {
        let mut candidate = align_up(region.start, align);

        // Try a spot; if it hits a reservation, jump past that
        // reservation and try again. Each retry moves strictly forward,
        // so this ends.
        'retry: while candidate.checked_add(size)? <= region.end.min(limit) {
            let wanted = candidate..candidate + size;
            for taken in reserved {
                if overlaps(&wanted, taken) {
                    candidate = align_up(taken.end, align);
                    continue 'retry;
                }
            }
            return Some(candidate);
        }
    }
    None
}

#[cfg(test)]
// Arrays of one range are exactly what these tests mean.
#[allow(clippy::single_range_in_vec_init)]
mod tests {
    use super::*;

    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * KIB;

    #[test]
    fn takes_the_first_fit() {
        let found = find_free_space([0..MIB], &[], 4 * KIB, 4 * KIB, u64::MAX);
        assert_eq!(found, Some(0));
    }

    #[test]
    fn steps_over_reservations() {
        // The QEMU layout: low memory, then RAM from 1 MiB with the
        // kernel at its start.
        let usable = [0..0x9fc00, MIB..512 * MIB];
        let reserved = [0..MIB, MIB..MIB + 0x1a000];
        let found = find_free_space(usable, &reserved, 16 * KIB, 4 * KIB, u64::MAX);
        assert_eq!(found, Some(MIB + 0x1a000));
    }

    #[test]
    fn steps_over_several_reservations_in_any_order() {
        let reserved = [0x5000..0x6000, 0x0..0x2000, 0x2000..0x5000];
        let found = find_free_space([0..0x10000], &reserved, 0x1000, 0x1000, u64::MAX);
        assert_eq!(found, Some(0x6000));
    }

    #[test]
    fn respects_alignment_and_limit() {
        let found = find_free_space([0x1234..0x100000], &[], 0x1000, 0x10000, u64::MAX);
        assert_eq!(found, Some(0x10000));
        assert_eq!(find_free_space([0x1000..0x100000], &[], 0x1000, 0x1000, 0x1800), None);
    }

    #[test]
    fn moves_to_the_next_region_when_one_is_too_small() {
        let usable = [0..0x800, 0x10000..0x20000];
        let found = find_free_space(usable, &[], 0x1000, 0x1000, u64::MAX);
        assert_eq!(found, Some(0x10000));
    }

    #[test]
    fn reports_when_nothing_fits() {
        assert_eq!(find_free_space([0..0x1000], &[0..0x1000], 0x1000, 0x1000, u64::MAX), None);
        assert_eq!(find_free_space(Vec::<Range<u64>>::new(), &[], 1, 1, u64::MAX), None);
    }
}
