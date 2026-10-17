//! Tree-wide invalidation is lazy; widget-local invalidation still clears individual caches.
use super::*;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct CacheEpochs {
    pub(super) layout: u64,
    pub(super) render: u64,
}

impl UiTree {
    /// Invalidate the selected cache groups across the entire tree in O(1).
    /// LAYOUT affects measurement and child layout; RENDER affects drawing. Both can be set.
    /// Values are discarded on their next use, so unvisited nodes remain stale until visited.
    ///
    /// Target-change policy belongs to the caller: request RENDER when drawing must be rebuilt,
    /// or make no request to retain it. No target/device/DPI classification is performed here.
    /// Layout must be refreshed before rendering after LAYOUT invalidation.
    /// Panics if an epoch is exhausted rather than allowing old caches to become valid again.
    pub fn invalidate_caches(&mut self, flags: UpdateResult) {
        let layout = if flags.contains(UpdateResult::LAYOUT) {
            self.cache_epochs
                .layout
                .checked_add(1)
                .expect("layout cache epoch exhausted")
        } else {
            self.cache_epochs.layout
        };
        let render = if flags.contains(UpdateResult::RENDER) {
            self.cache_epochs
                .render
                .checked_add(1)
                .expect("render cache epoch exhausted")
        } else {
            self.cache_epochs.render
        };
        self.cache_epochs = CacheEpochs { layout, render };
    }

    /// Immediately drop the selected CPU cache groups in O(number of arena slots).
    /// Widget state and current placement are retained. Returned Scenes may still own cloned
    /// resources; GPU residency is managed separately by the renderer.
    pub fn clear_caches(&mut self, flags: UpdateResult) {
        if flags.is_empty() {
            return;
        }
        self.invalidate_caches(flags);
        let epochs = self.cache_epochs;
        for (_, node) in self.nodes.iter_mut() {
            if flags.contains(UpdateResult::LAYOUT) {
                node.sync_layout_epoch(epochs.layout);
            }
            if flags.contains(UpdateResult::RENDER) {
                node.sync_render_epoch(epochs.render);
            }
        }
    }
}

impl UiNode {
    pub(super) fn sync_layout_epoch(&mut self, epoch: u64) {
        if self.cache_epochs.layout != epoch {
            // Clear both caches before marking the group current. Layout need not call measure,
            // and measure need not call layout: the unvisited cache must not become valid again.
            self.measure_cache = None;
            self.layout_cache = None;
            self.cache_epochs.layout = epoch;
        }
    }

    pub(super) fn sync_render_epoch(&mut self, epoch: u64) {
        if self.cache_epochs.render != epoch {
            self.draw_cache = None;
            self.cache_epochs.render = epoch;
        }
    }
}
