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
    focus::focus_from_pick,
    pick::{ancestors, PickQuery, PickerResource, Picker},
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

/// What one pointer press resolved to.
pub struct PointerPress<Msg: Message> {
    /// The message to hand the reducer, if a click target with an assigned
    /// message was found.
    pub click_msg: Option<Msg>,
    /// Whether the focus path moved. Focus lives in the ECS world rather than
    /// in the app model, so a focus-only change needs a redraw but **not** a
    /// re-run of the view.
    pub focus_changed: bool,
}

/// Resolve one pointer press: pick once, then serve both click routing and
/// focus from that single hit.
///
/// A press is the only moment where clicking and focusing must agree, so they
/// share the pick rather than each running their own. Focus state is updated
/// here; the click message is returned for the caller to apply, since only the
/// caller owns the model and the reducer.
pub fn resolve_pointer_press<Msg: Message>(world: &mut World, q: &PickQuery) -> PointerPress<Msg> {
    let hit = {
        let picker = world.resource::<PickerResource>();
        picker.0.pick(world, q).map(|h| h.entity)
    };

    let click_msg = hit
        .and_then(|entity| bubble_to_click_target::<Msg>(world, entity))
        .and_then(|target| world.get::<OnClick<Msg>>(target).and_then(|c| c.0));

    let focus_changed = focus_from_pick(world, hit);

    PointerPress {
        click_msg,
        focus_changed,
    }
}
