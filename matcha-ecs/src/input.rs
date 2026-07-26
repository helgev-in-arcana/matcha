//! Click routing: turning one picked entity into a click target.
//!
//! Picking itself lives in [`crate::pick`] and is a swappable backend. What is
//! left here is what happens *after* a hit: since picking is contractually
//! limited to a single entity (an ID-buffer backend cannot produce a candidate
//! list — see `pick.rs`'s module docs), a hit on an entity that carries no
//! handler resolves by **bubbling up** the tree, exactly like DOM event
//! bubbling, rather than by falling through to whatever is painted behind it.
//!
//! Concretely this means [`Pickable`](crate::components::input::Pickable)
//! declares *opacity to picking*, not "I want events": an entity that should
//! let clicks through simply does not carry it (which is already the default
//! for every container).

use bevy_ecs::{entity::Entity, world::World};

use crate::{
    components::input::{Message, OnClick},
    pick::{ancestors, PickQuery, Picker},
};

/// Walk up from `from` (inclusive) and return the first entity carrying
/// `OnClick<Msg>`.
///
/// An `OnClick(None)` still counts: the widget declared itself a click target
/// and simply has no message assigned, so the click stops there rather than
/// being handed to an ancestor that would react to it.
pub fn bubble_to_click_target<Msg: Message>(world: &World, from: Entity) -> Option<Entity> {
    ancestors(world, from).find(|&e| world.get::<OnClick<Msg>>(e).is_some())
}

/// Pick at `q`, then bubble to the nearest click target. `None` if nothing was
/// under the pointer, or if nothing from there up to the root handles clicks.
pub fn resolve_click_at<Msg: Message>(
    world: &World,
    picker: &dyn Picker,
    q: &PickQuery,
) -> Option<Entity> {
    let hit = picker.pick(world, q)?;
    bubble_to_click_target::<Msg>(world, hit.entity)
}
