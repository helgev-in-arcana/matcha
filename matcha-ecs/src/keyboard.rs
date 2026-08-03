//! Keyboard and IME delivery along the focus path.
//!
//! Unlike pointer input, which starts from a picked entity, keyboard input has
//! no spatial origin: it goes to whatever holds focus. Delivery walks
//! [`Focus::path`] **root to leaf**, so every ancestor is offered the event
//! before its descendants and may swallow it by returning `true`. This is the
//! capture direction the focus model was built for — the path is stored
//! root-first precisely so a parent can exercise full control over its subtree.
//!
//! Nothing here knows anything about text shaping or editing. The event types
//! come from `matcha-window` and carry only owned strings and byte offsets, so
//! the widget behind the dispatch is free to use any text engine.

use bevy_ecs::{entity::Entity, world::World};
use matcha_window::event::device_event::{ImeEvent, KeyInput};

use crate::{
    components::input::{ImeCursorArea, ImeDispatch, KeyDispatch},
    focus::Focus,
    resources::ui_root_window,
};

/// Deliver `input` down the focus path. Returns whether any entity consumed it.
pub fn dispatch_key(world: &mut World, input: &KeyInput) -> bool {
    dispatch_along_focus_path(world, |entity| {
        entity
            .get::<KeyDispatch>()
            .copied()
            .map(|dispatch| dispatch.call(entity, input))
    })
}

/// Deliver `event` down the focus path. Returns whether any entity consumed it.
pub fn dispatch_ime(world: &mut World, event: &ImeEvent) -> bool {
    dispatch_along_focus_path(world, |entity| {
        entity
            .get::<ImeDispatch>()
            .copied()
            .map(|dispatch| dispatch.call(entity, event))
    })
}

/// Walk the focus path root→leaf, calling `handle` on each entity that is still
/// alive. `handle` returns `None` when the entity has no handler of the kind
/// being dispatched, or `Some(consumed)`. Stops at the first `Some(true)`.
fn dispatch_along_focus_path(
    world: &mut World,
    mut handle: impl FnMut(&mut bevy_ecs::world::EntityWorldMut) -> Option<bool>,
) -> bool {
    // Copy the path first: the handlers get `&mut World` access through
    // `EntityWorldMut`, so the resource borrow cannot be held across the walk.
    let path: Vec<Entity> = world.resource::<Focus>().path().to_vec();

    for entity in path {
        let Ok(mut entity_mut) = world.get_entity_mut(entity) else {
            // A handler may despawn part of the tree; skip whatever is gone.
            continue;
        };
        if handle(&mut entity_mut) == Some(true) {
            return true;
        }
    }
    false
}

/// Tracks what has already been pushed to the OS window, so the platform is
/// only told about genuine changes.
#[derive(bevy_ecs::resource::Resource, Default)]
pub struct ImeWindowState {
    allowed: bool,
    cursor_area: Option<ImeCursorArea>,
}

/// Exclusive system: mirror the focused widget's IME needs onto the OS window.
///
/// The OS IME is off by default, so nothing can be typed in a non-Latin script
/// until it is switched on. The core's rule is deliberately shallow: **if the
/// focus vertex carries [`ImeDispatch`], IME is allowed** — it never looks at
/// the text being edited, or at what the widget uses to edit it.
///
/// Registered in `MatchaSet::PreExtract` after the focus systems. The cursor
/// area it publishes was written by the widget during `PreLayout`, so it lags
/// the caret by one frame; that is imperceptible for candidate-list placement
/// and it keeps this system free of any ordering constraint against the widget.
pub fn sync_ime_state(world: &mut World) {
    let focus_top = world.resource::<Focus>().top();
    let wants_ime = focus_top.is_some_and(|e| world.get::<ImeDispatch>(e).is_some());
    let cursor_area = focus_top.and_then(|e| world.get::<ImeCursorArea>(e).copied());

    let previous = world.get_resource_or_insert_with(ImeWindowState::default);
    let allowed_changed = previous.allowed != wants_ime;
    let area_changed = previous.cursor_area != cursor_area;
    if !allowed_changed && !area_changed {
        return;
    }

    let Some((_, window_comp)) = ui_root_window(world) else {
        return;
    };
    if allowed_changed {
        window_comp.window.set_ime_allowed(wants_ime);
    }
    // Publish the area whenever IME is on: a freshly-enabled IME has no
    // position yet, so re-sending an unchanged area after enabling is correct.
    if wants_ime {
        if let Some(ImeCursorArea([min_x, min_y, max_x, max_y])) = cursor_area {
            window_comp.window.set_ime_cursor_area(
                [min_x, min_y],
                [(max_x - min_x).max(0.0), (max_y - min_y).max(0.0)],
            );
        }
    }

    let mut state = world.resource_mut::<ImeWindowState>();
    state.allowed = wants_ime;
    state.cursor_area = cursor_area;
}
