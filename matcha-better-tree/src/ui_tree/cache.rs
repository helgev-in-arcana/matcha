//! Node-specific cache keys, invalidation, and drawing resource preparation.
use crate::metrics::{Bounds, Constraints};
use crate::ui_tree::widget::{Draw, UiError, UpdateResult};

mod resources;

/// Owns cached results and their replacement lifecycle.
/// Placement and the widget remain independent of cache lifetime.
#[derive(Default)]
pub(super) struct NodeCache {
    measure: Option<(Constraints, Bounds)>,
    layout: Option<Bounds>,
    draw: Option<Draw>,
}

impl NodeCache {
    pub(super) fn measured(&self, constraints: Constraints) -> Option<Bounds> {
        let &(key, bounds) = self.measure.as_ref()?;
        (key == constraints).then_some(bounds)
    }

    pub(super) fn cache_measure(&mut self, constraints: Constraints, bounds: Bounds) {
        self.measure = Some((constraints, bounds));
    }

    pub(super) fn laid_out(&self, bounds: Bounds) -> bool {
        self.layout == Some(bounds)
    }

    pub(super) fn cache_layout(&mut self, bounds: Bounds) {
        self.layout = Some(bounds);
    }

    /// Return valid drawing, replacing the previous value only after a successful render.
    /// Refresh bypasses reuse for draws that depend on the current scene's backdrop.
    pub(super) fn drawing(
        &mut self,
        refresh: bool,
        render: impl FnOnce() -> Draw,
    ) -> Result<&Draw, UiError> {
        if refresh || self.draw.is_none() {
            self.draw = Some(resources::manage(render())?);
        }
        Ok(self
            .draw
            .as_ref()
            .expect("drawing populated after successful render"))
    }

    pub(super) fn invalidate(&mut self, flags: UpdateResult) {
        if flags.contains(UpdateResult::LAYOUT) {
            self.measure = None;
            self.layout = None;
        }
        if flags.contains(UpdateResult::RENDER) {
            self.draw = None;
        }
    }

    #[cfg(test)]
    pub(super) fn measure(&self) -> Option<&(Constraints, Bounds)> {
        self.measure.as_ref()
    }

    #[cfg(test)]
    pub(super) fn layout(&self) -> Option<&Bounds> {
        self.layout.as_ref()
    }

    #[cfg(test)]
    pub(super) fn draw(&self) -> Option<&Draw> {
        self.draw.as_ref()
    }

    #[cfg(test)]
    pub(super) fn drawn(&self) -> bool {
        self.draw.is_some()
    }
}
