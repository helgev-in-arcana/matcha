//! Render-pipeline cache keyed by surface/target texture format.
//!
//! Native targets use `moka` for bounded, concurrent caching. wasm is
//! single-threaded and wgpu handles there are `!Send`, so a plain locked
//! map is used instead.

use std::hash::Hash;

#[cfg(not(web))]
pub struct PipelineCache<K, V> {
    inner: moka::sync::Cache<K, V, fxhash::FxBuildHasher>,
}

#[cfg(not(web))]
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

#[cfg(web)]
pub struct PipelineCache<K, V> {
    inner: parking_lot::Mutex<fxhash::FxHashMap<K, V>>,
}

#[cfg(web)]
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
