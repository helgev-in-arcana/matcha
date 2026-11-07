//! A single cached result, independent of UI phases, propagation, and resource preparation.

pub(super) struct Cache<K, V> {
    entry: Option<(K, V)>,
}

impl<K, V> Default for Cache<K, V> {
    fn default() -> Self {
        Self { entry: None }
    }
}

impl<K, V> Cache<K, V> {
    /// Publish a successfully computed value with the input that produced it.
    pub(super) fn store(&mut self, key: K, value: V) {
        self.entry = Some((key, value));
    }

    pub(super) fn invalidate(&mut self) {
        self.entry = None;
    }

    #[cfg(test)]
    pub(super) fn value(&self) -> Option<&V> {
        self.entry.as_ref().map(|(_, value)| value)
    }
}

impl<K: PartialEq, V> Cache<K, V> {
    pub(super) fn get(&self, key: &K) -> Option<&V> {
        let (stored_key, value) = self.entry.as_ref()?;
        (stored_key == key).then_some(value)
    }

    /// Reuse a matching entry unless refresh is requested. Failure preserves the previous
    /// key and value, which remain reusable only for their original input.
    pub(super) fn get_or_try_insert_with<E>(
        &mut self,
        key: K,
        refresh: bool,
        create: impl FnOnce() -> Result<V, E>,
    ) -> Result<&V, E> {
        if refresh || self.get(&key).is_none() {
            self.store(key, create()?);
        }
        Ok(&self
            .entry
            .as_ref()
            .expect("cache populated after successful generation")
            .1)
    }
}
