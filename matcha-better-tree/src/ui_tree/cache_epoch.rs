//! Cache epochs and stamped values. Advancing an epoch never touches cached values.
use std::sync::atomic::{AtomicU64, Ordering};

use crate::ui_tree::widget::UpdateResult;

/// A snapshot of one cache domain's current revision and validity floors.
/// Copy snapshots into scoped Contexts; mutate only the owner's copy.
/// `default()` is the standalone domain. Windows must use `new()` for isolation.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CacheEpoch {
    domain: u64,
    revision: u64,
    layout_since: u64,
    render_since: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CacheStamp {
    domain: u64,
    revision: u64,
}

/// A cache belongs to exactly one validity group; invalidation can select both.
#[derive(Clone, Copy)]
pub(super) enum CacheGroup {
    Layout,
    Render,
}

/// Owns the empty/occupied state, creation stamp, and replacement lifecycle of one value.
#[derive(Debug, Clone, Copy)]
pub(super) struct Cached<T> {
    entry: Option<(T, CacheStamp)>,
}

impl<T> Default for Cached<T> {
    fn default() -> Self {
        Self { entry: None }
    }
}

impl<T> Cached<T> {
    pub(super) fn new(value: T, epoch: CacheEpoch) -> Self {
        Self {
            entry: Some((value, epoch.stamp())),
        }
    }

    pub(super) fn get(&self, epoch: CacheEpoch, group: CacheGroup) -> Option<&T> {
        let (value, stamp) = self.entry.as_ref()?;
        epoch.accepts(*stamp, group).then_some(value)
    }

    pub(super) fn store(&mut self, value: T, epoch: CacheEpoch) {
        *self = Self::new(value, epoch);
    }

    /// Reuse the current value or replace it after successful generation.
    /// Failure leaves the previous entry (if any) and its stamp unchanged.
    pub(super) fn get_or_try_insert_with<E>(
        &mut self,
        epoch: CacheEpoch,
        group: CacheGroup,
        refresh: bool,
        create: impl FnOnce() -> Result<T, E>,
    ) -> Result<&T, E> {
        if refresh || self.get(epoch, group).is_none() {
            self.store(create()?, epoch);
        }
        Ok(self
            .get(epoch, group)
            .expect("cache populated after successful generation"))
    }

    /// Release the cached value and its stamp, returning to the empty state.
    pub(super) fn invalidate(&mut self) {
        self.entry = None;
    }

    #[cfg(test)]
    pub(super) fn stamp(&self) -> Option<CacheStamp> {
        self.entry.as_ref().map(|(_, stamp)| *stamp)
    }

    #[cfg(test)]
    pub(super) fn value(&self) -> Option<&T> {
        self.entry.as_ref().map(|(value, _)| value)
    }
}

impl CacheEpoch {
    /// Start an independent domain, including when a Window replaces another at the same UiId.
    pub fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let domain = NEXT
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
            .expect("cache domain exhausted");
        Self {
            domain,
            ..Self::default()
        }
    }

    /// Advance the revision and invalidate the selected groups. Unselected groups may reuse
    /// older results. Empty flags advance the revision without invalidating either group.
    pub fn advance(&mut self, flags: UpdateResult) {
        let revision = self.revision.checked_add(1).expect("cache epoch exhausted");
        if flags.contains(UpdateResult::LAYOUT) {
            self.layout_since = revision;
        }
        if flags.contains(UpdateResult::RENDER) {
            self.render_since = revision;
        }
        self.revision = revision;
    }

    pub fn revision(self) -> u64 {
        self.revision
    }

    pub(super) fn stamp(self) -> CacheStamp {
        CacheStamp {
            domain: self.domain,
            revision: self.revision,
        }
    }

    pub(super) fn accepts(self, stamp: CacheStamp, group: CacheGroup) -> bool {
        let since = match group {
            CacheGroup::Layout => self.layout_since,
            CacheGroup::Render => self.render_since,
        };
        stamp.domain == self.domain && (since..=self.revision).contains(&stamp.revision)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;

    #[test]
    fn empty_cache_reuses_replaces_and_releases_values_without_requiring_default() {
        struct Value;
        let epoch = CacheEpoch::new();
        let value = Rc::new(Value);
        let mut cache = Cached::<Rc<Value>>::default();
        assert!(cache.get(epoch, CacheGroup::Render).is_none());
        assert_eq!(cache.stamp(), None);
        cache.store(Rc::clone(&value), epoch);
        assert_eq!(Rc::strong_count(&value), 2);
        let reused = cache
            .get_or_try_insert_with(epoch, CacheGroup::Render, false, || {
                Err::<Rc<Value>, _>("must reuse")
            })
            .unwrap();
        assert!(Rc::ptr_eq(reused, &value));
        let replacement = Rc::new(Value);
        cache
            .get_or_try_insert_with(epoch, CacheGroup::Render, true, || {
                Ok::<_, ()>(Rc::clone(&replacement))
            })
            .unwrap();
        assert_eq!(Rc::strong_count(&value), 1);
        assert_eq!(Rc::strong_count(&replacement), 2);
        cache.invalidate();
        cache.invalidate();
        assert_eq!(Rc::strong_count(&replacement), 1);
        assert!(cache.get(epoch, CacheGroup::Render).is_none());
        assert_eq!(cache.stamp(), None);
    }

    #[test]
    fn failed_generation_preserves_empty_or_stale_state_and_can_be_retried() {
        let mut epoch = CacheEpoch::new();
        let mut cache = Cached::<String>::default();
        assert!(
            cache
                .get_or_try_insert_with(epoch, CacheGroup::Render, false, || Err(()))
                .is_err()
        );
        assert_eq!(cache.value(), None);
        assert_eq!(cache.stamp(), None);
        cache.store("old".into(), epoch);
        let stamp = cache.stamp();
        epoch.advance(UpdateResult::RENDER);
        assert!(
            cache
                .get_or_try_insert_with(epoch, CacheGroup::Render, false, || Err(()))
                .is_err()
        );
        assert_eq!(cache.value().map(String::as_str), Some("old"));
        assert_eq!(cache.stamp(), stamp);
        assert!(cache.get(epoch, CacheGroup::Render).is_none());
        assert_eq!(
            cache
                .get_or_try_insert_with(epoch, CacheGroup::Render, false, || Ok::<_, ()>(
                    "new".into()
                ))
                .unwrap(),
            "new"
        );
        assert_eq!(cache.stamp(), Some(epoch.stamp()));
    }
}
