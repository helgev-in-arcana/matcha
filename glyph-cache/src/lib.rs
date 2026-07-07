//! A generic, fixed-capacity LRU cache with per-batch "protect" semantics,
//! originally designed for GPU glyph atlases (`suzuri`'s
//! `renderer::gpu_renderer::glyph_cache` — copied and generalised here rather
//! than kept as a dependency, since matcha's own atlas type
//! (`gpu_utils::texture_atlas::TextureAtlas`) already handles region
//! allocation and RAII-based deallocation; this crate only needs to decide
//! *which* entries to keep, not *where* they live in a texture).
//!
//! Deliberately **wgpu-independent**: `V` is whatever the caller wants to
//! cache per key (e.g. an `(AtlasRegion, size)` pair) — this crate has no
//! knowledge of GPU resources at all.
//!
//! ## Batch protection
//!
//! A single render frame can look up the same key many times (e.g. the same
//! glyph appearing several times in a line). Without protection, a
//! frame that touches more distinct keys than the cache has capacity for
//! could evict an entry *and then need it again later in the same frame*,
//! causing thrashing. [`GlyphCache::new_batch`] marks the start of a new
//! frame; any entry looked up since the last `new_batch` call is "protected"
//! and will not be evicted until the *next* batch begins.

use std::{collections::HashMap, hash::Hash, num::NonZeroUsize};

#[derive(Clone, Copy)]
struct LruNode<K> {
    key: Option<K>,
    newer: Option<usize>,
    older: Option<usize>,
    last_used_batch_id: usize,
}

impl<K> Default for LruNode<K> {
    fn default() -> Self {
        Self {
            key: None,
            newer: None,
            older: None,
            last_used_batch_id: 0,
        }
    }
}

/// A fixed-capacity LRU cache mapping `K` to a lazily-built `V`, with
/// batch-scoped eviction protection (see module docs).
pub struct GlyphCache<K, V> {
    capacity: usize,
    nodes: Vec<LruNode<K>>,
    values: Vec<Option<V>>,
    head: Option<usize>,
    tail: Option<usize>,
    map: HashMap<K, usize, fxhash::FxBuildHasher>,
    empties: Vec<usize>,
    current_batch_id: usize,
}

impl<K: Copy + Eq + Hash, V> GlyphCache<K, V> {
    pub fn new(capacity: NonZeroUsize) -> Self {
        let capacity = capacity.get();
        Self {
            capacity,
            nodes: vec![LruNode::default(); capacity],
            values: (0..capacity).map(|_| None).collect(),
            head: None,
            tail: None,
            map: HashMap::with_capacity_and_hasher(capacity, fxhash::FxBuildHasher::default()),
            empties: (0..capacity).collect(),
            current_batch_id: 0,
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn clear(&mut self) {
        self.map.clear();
        self.empties.clear();
        self.empties.extend(0..self.capacity);
        for value in &mut self.values {
            *value = None;
        }
        self.head = None;
        self.tail = None;
        self.current_batch_id = 0;
    }

    /// Mark the start of a new batch (frame). Entries looked up in the
    /// *previous* batch lose their protection and become evictable again.
    pub fn new_batch(&mut self) {
        self.current_batch_id = self.current_batch_id.wrapping_add(1);
    }

    /// Look up `key`; if absent, build a value via `build` and insert it,
    /// evicting the least-recently-used entry if the cache is full.
    ///
    /// Returns `None` only when every entry has already been protected in
    /// the current batch (the whole cache is in active use this frame and
    /// has no room for a new entry) — the caller should fall back to
    /// drawing this key uncached for the frame.
    pub fn get_or_insert_with(&mut self, key: K, build: impl FnOnce() -> V) -> Option<&V> {
        if let Some(&index) = self.map.get(&key) {
            self.nodes[index].last_used_batch_id = self.current_batch_id;
            self.move_to_front(index);
            return self.values[index].as_ref();
        }

        let index = self.allocate_slot(key)?;
        self.values[index] = Some(build());
        self.values[index].as_ref()
    }

    /// Find (or evict-and-reuse) a slot for a not-yet-present `key`, already
    /// linked into the list as the new head. Returns `None` if the cache is
    /// full and its LRU tail is protected in the current batch.
    fn allocate_slot(&mut self, key: K) -> Option<usize> {
        if let Some(index) = self.empties.pop() {
            self.attach_to_head(index, key);
            self.map.insert(key, index);
            return Some(index);
        }

        let tail_index = self
            .tail
            .expect("tail is set whenever capacity > 0 and no empty slots remain");
        if self.nodes[tail_index].last_used_batch_id == self.current_batch_id {
            return None;
        }

        self.detach(tail_index);
        if let Some(old_key) = self.nodes[tail_index].key.take() {
            self.map.remove(&old_key);
        }
        self.values[tail_index] = None;
        self.attach_to_head(tail_index, key);
        self.map.insert(key, tail_index);
        Some(tail_index)
    }

    /// Unlink `index` from wherever it currently sits in the list, patching
    /// `head`/`tail` and its neighbours. Leaves `nodes[index]` itself
    /// untouched (the caller is about to either discard or reinsert it).
    ///
    /// `older`/`newer` point away from/towards the head respectively (see
    /// `attach_to_head`), so a node with no `older` neighbour *is* the tail,
    /// and one with no `newer` neighbour *is* the head — not the reverse.
    fn detach(&mut self, index: usize) {
        let older = self.nodes[index].older;
        let newer = self.nodes[index].newer;

        match older {
            Some(o) => self.nodes[o].newer = newer,
            None => self.tail = newer,
        }
        match newer {
            Some(n) => self.nodes[n].older = older,
            None => self.head = older,
        }
    }

    fn attach_to_head(&mut self, index: usize, key: K) {
        self.nodes[index] = LruNode {
            key: Some(key),
            newer: None,
            older: self.head,
            last_used_batch_id: self.current_batch_id,
        };
        if let Some(old_head) = self.head {
            self.nodes[old_head].newer = Some(index);
        }
        self.head = Some(index);
        if self.tail.is_none() {
            self.tail = Some(index);
        }
    }

    fn move_to_front(&mut self, index: usize) {
        if self.head == Some(index) {
            return;
        }
        self.detach(index);
        let key = self.nodes[index].key.expect("an indexed, in-map node must carry its key");
        self.attach_to_head(index, key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache(capacity: usize) -> GlyphCache<u32, u32> {
        GlyphCache::new(NonZeroUsize::new(capacity).unwrap())
    }

    #[test]
    fn miss_then_hit_reuses_the_same_value() {
        let mut c = cache(4);
        let mut builds = 0;
        assert_eq!(*c.get_or_insert_with(1, || { builds += 1; 100 }).unwrap(), 100);
        assert_eq!(*c.get_or_insert_with(1, || { builds += 1; 999 }).unwrap(), 100);
        assert_eq!(builds, 1, "build must only run on the first (miss) lookup");
    }

    #[test]
    fn eviction_targets_the_least_recently_used_entry() {
        let mut c = cache(2);
        c.new_batch();
        c.get_or_insert_with(1, || 1);
        c.new_batch();
        c.get_or_insert_with(2, || 2);
        c.new_batch();
        // Neither 1 nor 2 has been touched in this fresh batch, so both are
        // evictable; 1 is the LRU entry, so inserting a third distinct key
        // must evict 1, not 2.
        c.get_or_insert_with(3, || 3);
        assert_eq!(c.len(), 2);

        let mut rebuilt_2 = false;
        c.get_or_insert_with(2, || {
            rebuilt_2 = true;
            2
        });
        assert!(!rebuilt_2, "2 must still be cached (not evicted)");

        // `3` was inserted (and `2` just re-touched) in the current batch, so
        // both are protected; start a new batch to release that protection
        // before checking that `1` is actually gone.
        c.new_batch();
        let mut rebuilt_1 = false;
        c.get_or_insert_with(1, || {
            rebuilt_1 = true;
            1
        });
        assert!(rebuilt_1, "1 must have been evicted and required a rebuild");
    }

    #[test]
    fn protected_entries_are_not_evicted_within_the_same_batch() {
        let mut c = cache(2);
        c.new_batch();
        c.get_or_insert_with(1, || 1);
        c.get_or_insert_with(2, || 2);
        // Both entries were touched in this same batch, so both are
        // protected: a third distinct key cannot find room.
        assert!(c.get_or_insert_with(3, || 3).is_none());
    }

    #[test]
    fn new_batch_releases_protection_so_eviction_can_proceed() {
        let mut c = cache(2);
        c.new_batch();
        c.get_or_insert_with(1, || 1);
        c.get_or_insert_with(2, || 2);
        c.new_batch();
        assert!(c.get_or_insert_with(3, || 3).is_some());
    }

    #[test]
    fn capacity_one_evicts_across_batches_but_protects_within_one() {
        let mut c = cache(1);
        c.new_batch();
        c.get_or_insert_with(1, || 1);
        c.new_batch();
        c.get_or_insert_with(2, || 2);
        assert_eq!(c.len(), 1);
        // Still batch 2: the single slot (now holding `2`) was already
        // touched this batch, so it's protected — no room to bring `1` back.
        assert!(c.get_or_insert_with(1, || 1).is_none());
        c.new_batch();
        // A new batch releases that protection.
        assert!(c.get_or_insert_with(1, || 1).is_some());
    }

    #[test]
    fn clear_forgets_every_entry() {
        let mut c = cache(2);
        c.get_or_insert_with(1, || 1);
        c.get_or_insert_with(2, || 2);
        c.clear();
        assert!(c.is_empty());
        let mut rebuilt = false;
        c.get_or_insert_with(1, || {
            rebuilt = true;
            1
        });
        assert!(rebuilt);
    }
}
