//! Pointer state: which entities the pointer is over, and which are being
//! pressed — CSS's `:hover` and `:active`.
//!
//! # Shape of the model
//!
//! Like focus ([`crate::focus`]), this is derived from **one** pick and an
//! upward walk, so it is independent of which picking backend is installed.
//! Unlike focus it needs no policy pass and no memory: the pointer is simply
//! inside every box on the chain from the picked entity to the root, and that
//! chain *is* the answer.
//!
//! That is also exactly CSS's rule — `:hover` matches an element **and all its
//! ancestors** — so there is deliberately only one marker,
//! [`Hovered`](crate::components::input::Hovered), and no `:hover-within`
//! counterpart to focus's [`FocusWithin`](crate::components::focus::FocusWithin).
//! Focus needed the distinction because keyboard delivery targets the vertex
//! alone; nothing about hovering is vertex-specific.
//!
//! [`Active`](crate::components::input::Active) is the intersection of the
//! press chain with the current hover chain. Holding the button and dragging
//! off a button therefore releases its pressed look, and dragging back on
//! restores it — what every platform's buttons do.
//!
//! # Where it is resolved
//!
//! The resource is re-resolved twice: at event time (so a move reacts without
//! waiting for a frame) and again in `MatchaSet::PreExtract` via
//! [`sync_pointer_components`], after the picker has been refreshed against
//! this frame's layout. The second pass is what makes a menu that opens
//! *underneath* a stationary cursor come up already hovered.

use bevy_ecs::{entity::Entity, resource::Resource, query::With, world::World};

use crate::{
    components::input::{Active, Hovered},
    pick::{ancestors, PickQuery, PickerResource},
};

/// Where the pointer is and what it is touching.
///
/// The stored inputs are just the position and the press target; the two
/// chains are derived from them by [`resolve`] and re-derived whenever either
/// the inputs or the tree change.
#[derive(Resource, Default, Debug)]
pub struct PointerState {
    /// `None` while the pointer is outside the window.
    position: Option<[f32; 2]>,
    /// The entity a still-held press landed on, if any.
    pressed: Option<Entity>,
    /// Root → leaf, innermost last. Empty when the pointer is over nothing.
    hovered: Vec<Entity>,
    /// Root → leaf. The press chain, minus anything the pointer has since
    /// moved off.
    active: Vec<Entity>,
}

impl PointerState {
    /// The pointer's window-space position, or `None` while it is outside.
    pub fn position(&self) -> Option<[f32; 2]> {
        self.position
    }

    /// The hover chain, root first, innermost entity last.
    pub fn hover_path(&self) -> &[Entity] {
        &self.hovered
    }

    /// CSS `:hover` — the pointer is inside this entity's box, whether directly
    /// or because it is inside a descendant's.
    pub fn is_hovered(&self, entity: Entity) -> bool {
        self.hovered.contains(&entity)
    }

    /// CSS `:active` — a press landed inside this entity and the pointer has
    /// not left it since.
    pub fn is_active(&self, entity: Entity) -> bool {
        self.active.contains(&entity)
    }

    /// The innermost hovered entity, i.e. what the last pick returned.
    pub fn hit(&self) -> Option<Entity> {
        self.hovered.last().copied()
    }
}

/// Re-derive both chains from the stored position and press target. Returns
/// whether anything moved.
pub fn resolve(world: &mut World) -> bool {
    let (position, pressed) = {
        let state = world.get_resource_or_insert_with(PointerState::default);
        (state.position, state.pressed)
    };

    let hovered = match position {
        Some(pos) => chain_at(world, pos),
        None => Vec::new(),
    };

    // The press target can be despawned mid-press (a button that removes
    // itself), so its chain is only walked while it is still alive.
    let active = match pressed {
        Some(pressed) if world.entities().contains(pressed) => ancestors(world, pressed)
            .filter(|e| hovered.contains(e))
            .collect(),
        _ => Vec::new(),
    };

    let mut state = world.resource_mut::<PointerState>();
    if state.hovered == hovered && state.active == active {
        return false;
    }
    state.hovered = hovered;
    state.active = active;
    true
}

/// Pick at `pos` and return the root→leaf chain of what was found.
fn chain_at(world: &mut World, pos: [f32; 2]) -> Vec<Entity> {
    let Some(picker) = world.get_resource::<PickerResource>() else {
        return Vec::new();
    };
    let hit = picker.0.pick(
        world,
        &PickQuery {
            viewport_pos: pos,
        },
    );
    let Some(hit) = hit else {
        return Vec::new();
    };
    let mut chain: Vec<Entity> = ancestors(world, hit.entity).collect();
    chain.reverse();
    chain
}

/// Move the pointer, or take it out of the window with `None`. Returns whether
/// the derived state changed and a redraw is therefore worth asking for.
pub fn set_position(world: &mut World, position: Option<[f32; 2]>) -> bool {
    {
        let mut state = world.get_resource_or_insert_with(PointerState::default);
        if state.position == position {
            return false;
        }
        state.position = position;
    }
    resolve(world)
}

/// Record the entity a press landed on (or `None` on release). Returns whether
/// the derived state changed.
pub fn set_pressed(world: &mut World, pressed: Option<Entity>) -> bool {
    {
        let mut state = world.get_resource_or_insert_with(PointerState::default);
        if state.pressed == pressed {
            return false;
        }
        state.pressed = pressed;
    }
    resolve(world)
}

/// Exclusive system: re-resolve against this frame's layout, then bring the
/// [`Hovered`]/[`Active`] markers in line and invalidate the cached render node
/// of every entity that changed state.
///
/// Invalidation happens here rather than in a `Changed<Hovered>` system for the
/// same reason [`crate::focus::sync_focus_components`] does it inline:
/// `Changed<T>` never fires on component **removal**, so an entity *losing*
/// hover would keep painting its hover appearance forever. This pass already
/// knows the exact transition set in both directions.
pub fn sync_pointer_components(world: &mut World) {
    resolve(world);

    let (hovered, active) = {
        let state = world.resource::<PointerState>();
        (state.hovered.clone(), state.active.clone())
    };

    sync_marker(world, &hovered, Hovered);
    sync_marker(world, &active, Active);
}

/// Add `M` to everything in `wanted`, remove it from everything else, and
/// invalidate the render node of each entity that moved either way.
fn sync_marker<M: bevy_ecs::component::Component + Clone>(
    world: &mut World,
    wanted: &[Entity],
    marker: M,
) {
    let mut query = world.query_filtered::<Entity, With<M>>();
    let stale: Vec<Entity> = query
        .iter(world)
        .filter(|e| !wanted.contains(e))
        .collect();

    for entity in stale {
        if let Ok(mut e) = world.get_entity_mut(entity) {
            e.remove::<M>();
        }
        invalidate_render_item(world, entity);
    }

    for &entity in wanted {
        let Ok(mut e) = world.get_entity_mut(entity) else {
            continue;
        };
        if e.contains::<M>() {
            continue;
        }
        e.insert(marker.clone());
        invalidate_render_item(world, entity);
    }
}

/// Drop `entity`'s cached render node, if it has one.
fn invalidate_render_item(world: &mut World, entity: Entity) {
    if let Some(mut item) = world.get_mut::<crate::components::render::RenderItem>(entity) {
        item.invalidate();
    }
}
