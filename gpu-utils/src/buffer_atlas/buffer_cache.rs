use std::collections::{BTreeSet, HashMap};
use std::hash::Hash;

/// A unique identifier for a `BufferAtlas`.
static ATLAS_ID: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BufferCacheId {
    id: usize,
}

#[allow(clippy::new_without_default)]
impl BufferCacheId {
    /// Creates a new, unique ID.
    pub fn new() -> Self {
        let id = ATLAS_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        BufferCacheId { id }
    }
}

pub struct BufferCache<K, const N: usize> {
    id: BufferCacheId,
    atlas: Option<wgpu::Buffer>,

    // allocated buffers and its keys
    allocations: HashMap<K, usize>,
    // indices of free buffers
    not_allocated: BTreeSet<usize>,
}

impl<K: Eq + Hash + Clone, const N: usize> Default for BufferCache<K, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: Eq + Hash + Clone, const N: usize> BufferCache<K, N> {
    pub fn new() -> Self {
        BufferCache {
            id: BufferCacheId::new(),
            atlas: None,
            allocations: HashMap::new(),
            not_allocated: BTreeSet::new(),
        }
    }

    pub fn update<'a>(
        &'a mut self,
        _device: &wgpu::Device,
        _queue: &wgpu::Queue,
        _requests: Vec<(&K, impl FnOnce() -> [u8; N])>,
    ) -> (&'a wgpu::Buffer, Vec<usize>) {
        todo!()
    }
}

impl<K: Eq + Hash + Clone, const N: usize> BufferCache<K, N> {
    // resize and create buffer
    fn resize(&mut self, _new_size: usize) {
        todo!()
    }
}
