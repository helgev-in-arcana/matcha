use std::hash::Hash;

pub struct PipelineCache<K, V> {
    inner: moka::sync::Cache<K, V, fxhash::FxBuildHasher>,
}

impl<K, V> PipelineCache<K, V>
where
    K: Eq + Hash + Send + Sync + Clone + 'static,
    V: Clone + Send + Sync + 'static,
{
    pub fn new(capacity: u64) -> Self {
        Self {
            inner: moka::sync::Cache::builder()
                .max_capacity(capacity)
                .build_with_hasher(fxhash::FxBuildHasher::default()),
        }
    }

    /// Returns the cached value for `key`, inserting `init()`'s result first
    /// if the key is absent.
    pub fn get_with(&self, key: K, init: impl FnOnce() -> V) -> V {
        self.inner.get_with(key, init)
    }
}
