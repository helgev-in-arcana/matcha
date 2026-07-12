//! Framework systems.

use bevy_ecs::{query::Changed, system::Query};

use crate::components::{
    layout::LayoutOutput,
    render::{RenderItem, RenderOpacity},
};

/// Drop the cached render node of every entity whose [`LayoutOutput`] changed
/// this frame (new placement/size), so the next extract rebuilds it.
/// Registered in `MatchaSet::PreExtract`, after layout and before extract
/// (`ECS_ARCHITECTURE.md` §8.5).
pub fn invalidate_on_layout_change(mut query: Query<&mut RenderItem, Changed<LayoutOutput>>) {
    for mut item in query.iter_mut() {
        item.invalidate();
    }
}

/// Drop the cached render node of every entity whose [`RenderOpacity`] changed
/// this frame: colour is baked into the atlas at build time, so a fade must
/// rebuild every frame it progresses. Registered in `MatchaSet::PreExtract`,
/// same pattern as [`invalidate_on_layout_change`].
///
/// Core-side wiring for an extract-contract component — the *animating* of
/// opacity lives outside the core (see `matcha-ecs-widgets`'s `animation`
/// module); this system only reacts to the resulting writes.
pub fn invalidate_on_opacity_change(mut query: Query<&mut RenderItem, Changed<RenderOpacity>>) {
    for mut item in query.iter_mut() {
        item.invalidate();
    }
}
