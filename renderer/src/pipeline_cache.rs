use std::hash::Hash;

// ---------------------------------------------------------------------------
// Native: backed by moka::sync::Cache
// ---------------------------------------------------------------------------

#[cfg(not(target_arch = "wasm32"))]
pub struct PipelineCache<K, V> {
    inner: moka::sync::Cache<K, V, fxhash::FxBuildHasher>,
}

#[cfg(not(target_arch = "wasm32"))]
impl<K, V> PipelineCache<K, V>
where
    K: Hash + Eq + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    pub fn new() -> Self {
        Self {
            inner: moka::sync::Cache::builder()
                .max_capacity(1024)
                .build_with_hasher(fxhash::FxBuildHasher::default()),
        }
    }

    pub fn get_or_insert<F: FnOnce() -> V>(&self, key: K, f: F) -> V {
        self.inner.get_with(key, f)
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl<K, V> Default for PipelineCache<K, V>
where
    K: Hash + Eq + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// WASM: backed by dashmap::DashMap (no thread-pool required)
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
pub struct PipelineCache<K, V> {
    inner: dashmap::DashMap<K, V, fxhash::FxBuildHasher>,
}

#[cfg(target_arch = "wasm32")]
impl<K, V> PipelineCache<K, V>
where
    K: Hash + Eq + 'static,
    V: Clone + 'static,
{
    pub fn new() -> Self {
        Self {
            inner: dashmap::DashMap::with_hasher(fxhash::FxBuildHasher::default()),
        }
    }

    pub fn get_or_insert<F: FnOnce() -> V>(&self, key: K, f: F) -> V {
        self.inner
            .entry(key)
            .or_insert_with(f)
            .value()
            .clone()
    }
}

#[cfg(target_arch = "wasm32")]
impl<K, V> Default for PipelineCache<K, V>
where
    K: Hash + Eq + 'static,
    V: Clone + 'static,
{
    fn default() -> Self {
        Self::new()
    }
}
