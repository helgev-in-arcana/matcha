//! CPU-only placement within one fixed-size page or buffer.
//!
//! These allocators know neither content IDs nor GPU resources. The owner must
//! decide when an allocation is safe to free; freeing here immediately permits
//! reuse. Tokens belong to one allocator and are never reused, so stale tokens
//! cannot accidentally release a later allocation. They are not GPU lifetimes.
//!
//! Range allocation uses first fit in a sorted, coalesced free list. This keeps
//! the initial implementation inspectable and permits a different search policy
//! without changing allocation handles. Rectangle packing delegates to
//! guillotiere; its allocation IDs remain private to this adapter.

use std::{
    collections::HashMap,
    ops::Range,
    sync::atomic::{AtomicU64, Ordering},
};

use guillotiere::{AllocId, AtlasAllocator, Size};

// Only construction uses an atomic. The allocators themselves have exclusive
// mutable ownership, with no locks or reference-counted allocation handles.
static NEXT_ALLOCATOR: AtomicU64 = AtomicU64::new(1);

fn allocator_identity() -> Result<u64, AllocationError> {
    NEXT_ALLOCATOR
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
        .map_err(|_| AllocationError::Overflow)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AllocationError {
    InvalidSize,
    /// Alignment must be a nonzero power of two.
    InvalidAlignment,
    /// The requested dimensions or internal identity cannot be represented.
    Overflow,
    OutOfSpace,
    /// The token was freed already or belongs to another allocator.
    InvalidToken,
}

impl std::fmt::Display for AllocationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidSize => "allocation dimensions and byte counts must be nonzero",
            Self::InvalidAlignment => "allocation alignment must be a nonzero power of two",
            Self::Overflow => "allocation dimensions or identity exceed the representable range",
            Self::OutOfSpace => "no free region can satisfy the allocation request",
            Self::InvalidToken => "allocation token is stale or belongs to another allocator",
        })
    }
}

impl std::error::Error for AllocationError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct RangeToken {
    allocator: u64,
    serial: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RangeAllocation {
    pub token: RangeToken,
    /// Exactly the requested bytes; alignment padding remains available.
    pub range: Range<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RangeStats {
    pub capacity_bytes: u64,
    pub used_bytes: u64,
    pub free_bytes: u64,
    /// Largest free span before applying a particular requested alignment.
    pub largest_free_range: u64,
    pub free_ranges: usize,
    pub allocations: usize,
}

pub(crate) struct RangeAllocator {
    identity: u64,
    next_serial: u64,
    capacity: u64,
    used: u64,
    free: Vec<Range<u64>>,
    live: HashMap<u64, Range<u64>>,
}

impl RangeAllocator {
    pub fn new(capacity_bytes: u64) -> Result<Self, AllocationError> {
        if capacity_bytes == 0 {
            return Err(AllocationError::InvalidSize);
        }
        Ok(Self {
            identity: allocator_identity()?,
            next_serial: 1,
            capacity: capacity_bytes,
            used: 0,
            free: std::iter::once(0..capacity_bytes).collect(),
            live: HashMap::new(),
        })
    }

    pub fn allocate(
        &mut self,
        bytes: u64,
        alignment: u64,
    ) -> Result<RangeAllocation, AllocationError> {
        if bytes == 0 {
            return Err(AllocationError::InvalidSize);
        }
        if !alignment.is_power_of_two() {
            return Err(AllocationError::InvalidAlignment);
        }
        let next_serial = self
            .next_serial
            .checked_add(1)
            .ok_or(AllocationError::Overflow)?;
        let candidate = self.free.iter().enumerate().find_map(|(index, span)| {
            // An overflowing aligned address cannot fit in this span. Avoid
            // computing end until subtraction has established that it fits.
            let start = span.start.checked_add(alignment - 1)? & !(alignment - 1);
            if start > span.end || bytes > span.end - start {
                return None;
            }
            Some((index, start..start + bytes))
        });
        let (index, range) = candidate.ok_or(AllocationError::OutOfSpace)?;
        let used = self
            .used
            .checked_add(bytes)
            .ok_or(AllocationError::Overflow)?;
        let span = self.free[index].clone();
        match (span.start < range.start, range.end < span.end) {
            (true, true) => {
                self.free[index].end = range.start;
                self.free.insert(index + 1, range.end..span.end);
            }
            (true, false) => self.free[index].end = range.start,
            (false, true) => self.free[index].start = range.end,
            (false, false) => {
                self.free.remove(index);
            }
        }
        let token = RangeToken {
            allocator: self.identity,
            serial: self.next_serial,
        };
        self.live.insert(token.serial, range.clone());
        self.next_serial = next_serial;
        self.used = used;
        Ok(RangeAllocation { token, range })
    }

    pub fn free(&mut self, token: RangeToken) -> Result<(), AllocationError> {
        if token.allocator != self.identity {
            return Err(AllocationError::InvalidToken);
        }
        let range = self
            .live
            .remove(&token.serial)
            .ok_or(AllocationError::InvalidToken)?;
        self.used -= range.end - range.start;
        let mut index = self.free.partition_point(|span| span.start < range.start);
        if index > 0 && self.free[index - 1].end == range.start {
            index -= 1;
            self.free[index].end = range.end;
        } else {
            self.free.insert(index, range);
        }
        if index + 1 < self.free.len() && self.free[index].end == self.free[index + 1].start {
            self.free[index].end = self.free[index + 1].end;
            self.free.remove(index + 1);
        }
        Ok(())
    }

    pub fn stats(&self) -> RangeStats {
        RangeStats {
            capacity_bytes: self.capacity,
            used_bytes: self.used,
            free_bytes: self.capacity - self.used,
            largest_free_range: self
                .free
                .iter()
                .map(|span| span.end - span.start)
                .max()
                .unwrap_or(0),
            free_ranges: self.free.len(),
            allocations: self.live.len(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct RectangleToken {
    allocator: u64,
    serial: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RectangleAllocation {
    pub token: RectangleToken,
    pub origin: [u32; 2],
    pub extent: [u32; 2],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RectangleStats {
    pub extent: [u32; 2],
    pub capacity_area: u64,
    pub used_area: u64,
    /// Total free area, not a promise that an equally sized rectangle fits.
    pub free_area: u64,
    pub allocations: usize,
}

pub(crate) struct RectangleAllocator {
    identity: u64,
    next_serial: u64,
    extent: [u32; 2],
    used_area: u64,
    inner: AtlasAllocator,
    live: HashMap<u64, (AllocId, u64)>,
}

fn rectangle_size([width, height]: [u32; 2]) -> Result<Size, AllocationError> {
    if width == 0 || height == 0 {
        return Err(AllocationError::InvalidSize);
    }
    let width = i32::try_from(width).map_err(|_| AllocationError::Overflow)?;
    let height = i32::try_from(height).map_err(|_| AllocationError::Overflow)?;
    Ok(Size::new(width, height))
}

impl RectangleAllocator {
    pub fn new(extent: [u32; 2]) -> Result<Self, AllocationError> {
        let size = rectangle_size(extent)?;
        Ok(Self {
            identity: allocator_identity()?,
            next_serial: 1,
            extent,
            used_area: 0,
            inner: AtlasAllocator::new(size),
            live: HashMap::new(),
        })
    }

    /// Reserve an exact rectangle without padding or rotation. Texture gutters
    /// and format-specific block alignment belong to the page manager above.
    pub fn allocate(&mut self, extent: [u32; 2]) -> Result<RectangleAllocation, AllocationError> {
        let size = rectangle_size(extent)?;
        let next_serial = self
            .next_serial
            .checked_add(1)
            .ok_or(AllocationError::Overflow)?;
        let area = u64::from(extent[0]) * u64::from(extent[1]);
        let used_area = self
            .used_area
            .checked_add(area)
            .ok_or(AllocationError::Overflow)?;
        let allocation = self
            .inner
            .allocate(size)
            .ok_or(AllocationError::OutOfSpace)?;
        let token = RectangleToken {
            allocator: self.identity,
            serial: self.next_serial,
        };
        self.live.insert(token.serial, (allocation.id, area));
        self.next_serial = next_serial;
        self.used_area = used_area;
        Ok(RectangleAllocation {
            token,
            origin: [
                allocation.rectangle.min.x as u32,
                allocation.rectangle.min.y as u32,
            ],
            extent,
        })
    }

    pub fn free(&mut self, token: RectangleToken) -> Result<(), AllocationError> {
        if token.allocator != self.identity {
            return Err(AllocationError::InvalidToken);
        }
        let (id, area) = self
            .live
            .remove(&token.serial)
            .ok_or(AllocationError::InvalidToken)?;
        self.inner.deallocate(id);
        self.used_area -= area;
        Ok(())
    }

    pub fn stats(&self) -> RectangleStats {
        let capacity_area = u64::from(self.extent[0]) * u64::from(self.extent[1]);
        RectangleStats {
            extent: self.extent,
            capacity_area,
            used_area: self.used_area,
            free_area: capacity_area - self.used_area,
            allocations: self.live.len(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn next(seed: &mut u64) -> u64 {
        *seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        *seed
    }

    #[test]
    fn range_alignment_padding_is_reusable_and_free_coalesces_both_neighbors() {
        let mut allocator = RangeAllocator::new(64).expect("valid capacity");
        let first = allocator.allocate(3, 1).expect("first allocation fits");
        let aligned = allocator.allocate(8, 16).expect("aligned allocation fits");
        assert_eq!(aligned.range, 16..24);
        let padding = allocator
            .allocate(13, 1)
            .expect("alignment padding stays free");
        assert_eq!(padding.range, 3..16);
        allocator.free(first.token).expect("live first token");
        allocator.free(aligned.token).expect("live aligned token");
        assert_eq!(allocator.stats().free_ranges, 2);
        allocator.free(padding.token).expect("live padding token");
        assert_eq!(allocator.stats().largest_free_range, 64);
        assert_eq!(allocator.stats().free_ranges, 1);
        assert_eq!(
            allocator
                .allocate(64, 1)
                .expect("whole page recovered")
                .range,
            0..64
        );
    }

    #[test]
    fn invalid_range_requests_and_overflow_do_not_change_allocations() {
        assert!(matches!(
            RangeAllocator::new(0),
            Err(AllocationError::InvalidSize)
        ));
        let mut allocator = RangeAllocator::new(u64::MAX).expect("maximum representable end");
        assert_eq!(allocator.allocate(0, 1), Err(AllocationError::InvalidSize));
        assert_eq!(
            allocator.allocate(1, 0),
            Err(AllocationError::InvalidAlignment)
        );
        assert_eq!(
            allocator.allocate(1, 3),
            Err(AllocationError::InvalidAlignment)
        );
        let prefix = allocator
            .allocate(u64::MAX - 7, 1)
            .expect("large prefix fits");
        let before = allocator.stats();
        assert_eq!(allocator.allocate(1, 16), Err(AllocationError::OutOfSpace));
        assert_eq!(allocator.allocate(8, 1), Err(AllocationError::OutOfSpace));
        assert_eq!(allocator.stats(), before);
        let tail = allocator
            .allocate(7, 1)
            .expect("unaligned tail still available");
        assert_eq!(tail.range.end, u64::MAX);
        allocator.free(prefix.token).expect("prefix live");
        allocator.free(tail.token).expect("tail live");
        assert_eq!(allocator.stats().largest_free_range, u64::MAX);
    }

    #[test]
    fn range_tokens_reject_foreign_stale_and_exhausted_identities() {
        let mut a = RangeAllocator::new(8).expect("valid capacity");
        let mut b = RangeAllocator::new(8).expect("valid capacity");
        let old = a.allocate(8, 1).expect("fits");
        let other = b.allocate(8, 1).expect("fits");
        assert_eq!(b.free(old.token), Err(AllocationError::InvalidToken));
        a.free(old.token).expect("first free succeeds");
        let replacement = a.allocate(8, 1).expect("freed range reused");
        assert_ne!(old.token, replacement.token);
        assert_eq!(a.free(old.token), Err(AllocationError::InvalidToken));
        assert_eq!(a.stats().used_bytes, 8);
        b.free(other.token)
            .expect("foreign token attempt did not remove entry");
        b.next_serial = u64::MAX;
        let before = b.stats();
        assert_eq!(b.allocate(1, 1), Err(AllocationError::Overflow));
        assert_eq!(b.stats(), before);
    }

    #[test]
    fn range_seeded_churn_matches_independent_byte_occupancy() {
        const CAPACITY: usize = 257;
        let mut allocator = RangeAllocator::new(CAPACITY as u64).expect("valid capacity");
        let mut occupied = [false; CAPACITY];
        let mut live: Vec<RangeAllocation> = Vec::new();
        let mut seed = 0x78e5_01f3_429b_d60a;
        for _ in 0..4_000 {
            if !live.is_empty() && next(&mut seed) % 3 == 0 {
                let index = (next(&mut seed) as usize) % live.len();
                let allocation = live.swap_remove(index);
                allocator.free(allocation.token).expect("live token");
                occupied[allocation.range.start as usize..allocation.range.end as usize]
                    .fill(false);
            } else {
                let bytes = (next(&mut seed) % 33 + 1) as usize;
                let alignment = 1usize << (next(&mut seed) % 6);
                let expected = (0..=CAPACITY - bytes).find(|&start| {
                    start % alignment == 0 && occupied[start..start + bytes].iter().all(|v| !v)
                });
                match (allocator.allocate(bytes as u64, alignment as u64), expected) {
                    (Ok(allocation), Some(start)) => {
                        assert_eq!(allocation.range, start as u64..(start + bytes) as u64);
                        occupied[start..start + bytes].fill(true);
                        live.push(allocation);
                    }
                    (Err(AllocationError::OutOfSpace), None) => {}
                    (actual, expected) => {
                        panic!("allocation {actual:?} disagreed with oracle {expected:?}")
                    }
                }
            }
            let stats = allocator.stats();
            assert_eq!(
                stats.used_bytes as usize,
                occupied.iter().filter(|v| **v).count()
            );
            assert_eq!(stats.allocations, live.len());
            assert!(
                allocator
                    .free
                    .windows(2)
                    .all(|pair| pair[0].end < pair[1].start)
            );
        }
        for allocation in live {
            allocator
                .free(allocation.token)
                .expect("remaining live token");
        }
        assert_eq!(allocator.stats().largest_free_range, CAPACITY as u64);
    }

    #[test]
    fn rectangles_validate_boundaries_tokens_and_identity_exhaustion() {
        assert!(matches!(
            RectangleAllocator::new([0, 8]),
            Err(AllocationError::InvalidSize)
        ));
        assert!(matches!(
            RectangleAllocator::new([u32::MAX, 1]),
            Err(AllocationError::Overflow)
        ));
        let mut a = RectangleAllocator::new([8, 8]).expect("valid extent");
        let mut b = RectangleAllocator::new([8, 8]).expect("valid extent");
        assert_eq!(a.allocate([0, 1]), Err(AllocationError::InvalidSize));
        assert_eq!(a.allocate([u32::MAX, 1]), Err(AllocationError::Overflow));
        assert_eq!(a.allocate([9, 1]), Err(AllocationError::OutOfSpace));
        let old = a.allocate([8, 8]).expect("whole page fits");
        let other = b.allocate([8, 8]).expect("whole page fits");
        assert_eq!(old.origin, [0, 0]);
        assert_eq!(a.stats().used_area, 64);
        assert_eq!(a.allocate([1, 1]), Err(AllocationError::OutOfSpace));
        assert_eq!(b.free(old.token), Err(AllocationError::InvalidToken));
        a.free(old.token).expect("first free succeeds");
        let replacement = a.allocate([8, 8]).expect("freed rectangle reused");
        assert_ne!(old.token, replacement.token);
        assert_eq!(a.free(old.token), Err(AllocationError::InvalidToken));
        assert_eq!(a.stats().used_area, 64);
        b.free(other.token)
            .expect("foreign attempt did not remove entry");
        b.next_serial = u64::MAX;
        let before = b.stats();
        assert_eq!(b.allocate([1, 1]), Err(AllocationError::Overflow));
        assert_eq!(b.stats(), before);
    }

    #[test]
    fn rectangle_maximum_coordinates_keep_area_in_u64() {
        let side = i32::MAX as u32;
        let mut allocator =
            RectangleAllocator::new([side, side]).expect("maximum valid coordinates");
        let allocation = allocator
            .allocate([side, side])
            .expect("whole rectangle fits");
        assert_eq!(allocation.origin, [0, 0]);
        assert_eq!(
            allocator.stats().used_area,
            u64::from(side) * u64::from(side)
        );
        assert_eq!(allocator.stats().free_area, 0);
        allocator.free(allocation.token).expect("live rectangle");
        assert_eq!(allocator.stats().used_area, 0);
    }

    #[test]
    fn rectangle_seeded_churn_never_overlaps_and_recovers_whole_page() {
        const WIDTH: usize = 31;
        const HEIGHT: usize = 29;
        let mut allocator =
            RectangleAllocator::new([WIDTH as u32, HEIGHT as u32]).expect("valid extent");
        let mut occupied = [false; WIDTH * HEIGHT];
        let mut live: Vec<RectangleAllocation> = Vec::new();
        let mut seed = 0x815e_769d_f010_238c;
        for _ in 0..3_000 {
            if !live.is_empty() && next(&mut seed) % 3 == 0 {
                let index = (next(&mut seed) as usize) % live.len();
                let allocation = live.swap_remove(index);
                allocator.free(allocation.token).expect("live rectangle");
                for y in allocation.origin[1]..allocation.origin[1] + allocation.extent[1] {
                    for x in allocation.origin[0]..allocation.origin[0] + allocation.extent[0] {
                        assert!(occupied[y as usize * WIDTH + x as usize]);
                        occupied[y as usize * WIDTH + x as usize] = false;
                    }
                }
            } else {
                let extent = [
                    (next(&mut seed) % 10 + 1) as u32,
                    (next(&mut seed) % 10 + 1) as u32,
                ];
                match allocator.allocate(extent) {
                    Ok(allocation) => {
                        assert_eq!(allocation.extent, extent);
                        assert!(allocation.origin[0] + extent[0] <= WIDTH as u32);
                        assert!(allocation.origin[1] + extent[1] <= HEIGHT as u32);
                        for y in allocation.origin[1]..allocation.origin[1] + extent[1] {
                            for x in allocation.origin[0]..allocation.origin[0] + extent[0] {
                                assert!(!occupied[y as usize * WIDTH + x as usize]);
                                occupied[y as usize * WIDTH + x as usize] = true;
                            }
                        }
                        live.push(allocation);
                    }
                    Err(AllocationError::OutOfSpace) => {}
                    Err(other) => panic!("valid small rectangle failed: {other:?}"),
                }
            }
            let stats = allocator.stats();
            assert_eq!(
                stats.used_area as usize,
                occupied.iter().filter(|v| **v).count()
            );
            assert_eq!(stats.allocations, live.len());
            assert_eq!(stats.free_area + stats.used_area, (WIDTH * HEIGHT) as u64);
        }
        for allocation in live {
            allocator
                .free(allocation.token)
                .expect("remaining live rectangle");
        }
        assert_eq!(allocator.stats().used_area, 0);
        assert_eq!(
            allocator
                .allocate([WIDTH as u32, HEIGHT as u32])
                .expect("whole page recovered")
                .origin,
            [0, 0]
        );
    }
}
