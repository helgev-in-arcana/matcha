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
    components::{
        input::{Message, OnClick, PointerDispatch, PointerInput, PointerPhase},
        layout::GlobalTransform,
    },
    focus::focus_from_pick,
    pick::{ancestors, PickHit, PickQuery, Picker, PickerResource},
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
pub fn resolve_pointer_press<Msg: Message>(
    world: &mut World,
    q: &PickQuery,
    count: u32,
) -> PointerPress<Msg> {
    let hit = pick_entity(world, q);

    let click_msg = hit
        .and_then(|entity| bubble_to_click_target::<Msg>(world, entity))
        .and_then(|target| world.get::<OnClick<Msg>>(target).and_then(|c| c.0.clone()));

    let focus_changed = focus_from_pick(world, hit);

    // After focus, so a widget's pointer handler can assume it already has it.
    if let Some(hit) = hit {
        dispatch_pointer(world, hit, q.viewport_pos, PointerPhase::Press { count });
    }

    PointerPress {
        click_msg,
        focus_changed,
    }
}

/// Run the active picker for `q`.
pub fn pick_entity(world: &World, q: &PickQuery) -> Option<Entity> {
    let picker = world.resource::<PickerResource>();
    picker.0.pick(world, q).map(|h: PickHit| h.entity)
}

/// Deliver a positioned pointer event, bubbling leaf→root from `from` until an
/// entity with a [`PointerDispatch`] consumes it.
///
/// Each candidate receives the position in **its own** coordinate space, so a
/// handler never has to know where its entity sits on screen.
pub fn dispatch_pointer(
    world: &mut World,
    from: Entity,
    window_pos: [f32; 2],
    phase: PointerPhase,
) -> bool {
    let candidates: Vec<Entity> = ancestors(world, from).collect();

    for entity in candidates {
        let Some(dispatch) = world.get::<PointerDispatch>(entity).copied() else {
            continue;
        };
        // Translation-only, matching every current `Layout` impl (the same
        // assumption `RectZPicker` makes when building its rectangles).
        let origin = world
            .get::<GlobalTransform>(entity)
            .map(|t| t.affine.transform_point(&nalgebra::Point3::origin()))
            .map(|p| [p.x, p.y])
            .unwrap_or([0.0, 0.0]);
        let input = PointerInput {
            local_pos: [window_pos[0] - origin[0], window_pos[1] - origin[1]],
            phase,
        };
        let Ok(mut entity_mut) = world.get_entity_mut(entity) else {
            continue;
        };
        if dispatch.call(&mut entity_mut, &input) {
            return true;
        }
    }
    false
}

/// Deliver a drag to whatever is under `q`, if anything handles positioned
/// pointer input. Focus and click routing are untouched: a drag continues an
/// interaction that a press already started.
pub fn dispatch_pointer_drag(world: &mut World, q: &PickQuery) -> bool {
    let Some(hit) = pick_entity(world, q) else {
        return false;
    };
    dispatch_pointer(world, hit, q.viewport_pos, PointerPhase::Drag)
}

/// Messages produced from inside the world, waiting to reach the reducer.
///
/// Clicks can hand their message straight back to the caller
/// ([`resolve_pointer_press`]), but a keyboard or IME handler runs deep inside
/// dispatch behind a non-generic fn pointer, and a system runs inside the
/// schedule — neither can reach the model or the reducer. Both push here
/// instead, and [`UiEcs`](crate::ui_ecs::UiEcs) drains the queue once the
/// world is no longer borrowed.
///
/// Order is preserved: messages are applied in the order they were emitted.
#[derive(bevy_ecs::resource::Resource)]
pub struct MessageQueue<Msg: Message> {
    pending: Vec<Msg>,
}

impl<Msg: Message> Default for MessageQueue<Msg> {
    fn default() -> Self {
        Self {
            pending: Vec::new(),
        }
    }
}

impl<Msg: Message> MessageQueue<Msg> {
    /// Queue a message for the reducer.
    pub fn push(&mut self, msg: Msg) {
        self.pending.push(msg);
    }

    /// Take everything queued so far.
    pub fn drain(&mut self) -> Vec<Msg> {
        std::mem::take(&mut self.pending)
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}

/// Queue a message from anywhere that has world access, inserting the queue on
/// first use so a widget never has to be registered up front.
pub fn emit_message<Msg: Message>(world: &mut World, msg: Msg) {
    world
        .get_resource_or_insert_with(MessageQueue::<Msg>::default)
        .push(msg);
}
