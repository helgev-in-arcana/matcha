use std::hash::Hash;

pub struct PipelineCache<K, V> {
    inner: parking_lot::Mutex<fxhash::FxHashMap<K, V>>,
}

impl<K, V> PipelineCache<K, V>
where
    K: Eq + Hash + Clone,
    V: Clone,
{
    pub fn new(_capacity: u64) -> Self {
        Self {
            inner: parking_lot::Mutex::new(fxhash::FxHashMap::default()),
        }
    }

    /// Returns the cached value for `key`, inserting `init()`'s result first
    /// if the key is absent.
    pub fn get_with(&self, key: K, init: impl FnOnce() -> V) -> V {
        self.inner.lock().entry(key).or_insert_with(init).clone()
    }
}
