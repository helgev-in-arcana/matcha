//! Node-specific cache keys, validity groups, and drawing resource preparation.
use super::cache_epoch::{CacheGroup, Cached};
use crate::CacheEpoch;
use crate::metrics::{Bounds, Constraints};
use crate::ui_tree::widget::{Draw, UiError, UpdateResult};

mod resources;

/// Selects node-specific keys and invalidation groups; Cached owns each value's lifecycle.
/// Placement and the widget remain independent of cache lifetime.
#[derive(Default)]
pub(super) struct NodeCache {
    measure: Cached<(Constraints, Bounds)>,
    layout: Cached<Bounds>,
    draw: Cached<Draw>,
}

impl NodeCache {
    pub(super) fn measured(&self, constraints: Constraints, epoch: CacheEpoch) -> Option<Bounds> {
        let &(key, bounds) = self.measure.get(epoch, CacheGroup::Layout)?;
        (key == constraints).then_some(bounds)
    }

    pub(super) fn cache_measure(
        &mut self,
        constraints: Constraints,
        bounds: Bounds,
        epoch: CacheEpoch,
    ) {
        self.measure.store((constraints, bounds), epoch);
    }

    pub(super) fn laid_out(&self, bounds: Bounds, epoch: CacheEpoch) -> bool {
        self.layout.get(epoch, CacheGroup::Layout) == Some(&bounds)
    }

    pub(super) fn cache_layout(&mut self, bounds: Bounds, epoch: CacheEpoch) {
        self.layout.store(bounds, epoch);
    }

    /// Return valid drawing, replacing the previous value only after a successful render.
    /// Refresh bypasses reuse for draws that depend on the current scene's backdrop.
    pub(super) fn drawing(
        &mut self,
        epoch: CacheEpoch,
        refresh: bool,
        render: impl FnOnce() -> Draw,
    ) -> Result<&Draw, UiError> {
        self.draw
            .get_or_try_insert_with(epoch, CacheGroup::Render, refresh, || {
                resources::manage(render())
            })
    }

    pub(super) fn invalidate(&mut self, flags: UpdateResult) {
        if flags.contains(UpdateResult::LAYOUT) {
            self.measure.invalidate();
            self.layout.invalidate();
        }
        if flags.contains(UpdateResult::RENDER) {
            self.draw.invalidate();
        }
    }

    #[cfg(test)]
    pub(super) fn measure(&self) -> &Cached<(Constraints, Bounds)> {
        &self.measure
    }

    #[cfg(test)]
    pub(super) fn layout(&self) -> &Cached<Bounds> {
        &self.layout
    }

    #[cfg(test)]
    pub(super) fn draw(&self) -> &Cached<Draw> {
        &self.draw
    }

    #[cfg(test)]
    pub(super) fn drawn(&self, epoch: CacheEpoch) -> bool {
        self.draw.get(epoch, CacheGroup::Render).is_some()
    }
}
