//! Framework systems.

use bevy_ecs::{query::Changed, system::Query};

use crate::{
    animation::{Animated, Opacity},
    components::{layout::LayoutOutput, render::RenderItem},
};

/// Drop the cached render node of every entity whose [`LayoutOutput`] changed
/// this frame (new placement/size), so the next extract rebuilds it.
/// Registered in `MatchaSet::Flush`, after layout and before extract
/// (`ECS_ARCHITECTURE.md` §8.5).
pub fn invalidate_on_layout_change(mut query: Query<&mut RenderItem, Changed<LayoutOutput>>) {
    for mut item in query.iter_mut() {
        item.invalidate();
    }
}

/// Drop the cached render node of every entity whose [`Animated<Opacity>`]
/// changed this frame (M7): colour is baked into the atlas at build time, so
/// an in-flight fade must rebuild every frame it progresses. Registered in
/// `MatchaSet::Flush`, same pattern as [`invalidate_on_layout_change`].
pub fn invalidate_on_animated_opacity_change(
    mut query: Query<&mut RenderItem, Changed<Animated<Opacity>>>,
) {
    for mut item in query.iter_mut() {
        item.invalidate();
    }
}
