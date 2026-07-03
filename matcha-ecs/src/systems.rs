//! Framework systems.

use bevy_ecs::{query::Changed, system::Query};

use crate::components::{layout::LayoutOutput, render::RenderItem};

/// Drop the cached render node of every entity whose [`LayoutOutput`] changed
/// this frame (new placement/size), so the next extract rebuilds it.
/// Registered in `MatchaSet::Flush`, after layout and before extract
/// (`ECS_ARCHITECTURE.md` §8.5).
pub fn invalidate_on_layout_change(mut query: Query<&mut RenderItem, Changed<LayoutOutput>>) {
    for mut item in query.iter_mut() {
        item.invalidate();
    }
}
