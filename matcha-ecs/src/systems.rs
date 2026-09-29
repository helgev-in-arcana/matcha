//! Framework systems.

use bevy_ecs::{query::Changed, system::Query};

use crate::components::{layout::LayoutOutput, render::RenderItem};

/// Advance the draw revision of every entity whose [`LayoutOutput`] changed
/// this frame (new placement/size), so observers see a new draw-property revision.
/// Registered in `MatchaSet::PreExtract`, after layout and before extraction.
pub fn invalidate_on_layout_change(mut query: Query<&mut RenderItem, Changed<LayoutOutput>>) {
    for mut item in query.iter_mut() {
        item.invalidate();
    }
}

// Opacity is extracted and applied to each Object at draw time. Its changes
// require a redraw, but no draw-revision invalidation or resource regeneration.
