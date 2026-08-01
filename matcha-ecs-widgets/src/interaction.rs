//! State-driven colour: what a widget looks like when hovered or pressed, and
//! how it gets there.
//!
//! # Why this exists at all
//!
//! The core hands widgets `:hover`/`:active` as markers and as
//! [`RenderCtx`](matcha_ecs::components::render::RenderCtx) flags, and
//! invalidates the cached node on every transition. A widget that only wants to
//! *snap* between two colours therefore needs nothing from this module — it can
//! branch on `ctx.hovered` inside its own builder, exactly as `Button` already
//! branches on `ctx.focused` to draw its focus ring.
//!
//! What a builder cannot do is *interpolate*. It is a closure captured back at
//! `bundle()`/`patch()` time and it runs on the render thread with no world
//! access, so a value that changes on its own between frames has to reach it
//! through a shared cell — the same side-channel `Text`'s wrap width and
//! `ScrollView`'s offset already use. That cell, plus the system that drives
//! it, is all this module is.
//!
//! # Registration
//!
//! [`default_systems`] (or [`crate::default_systems`]) must be registered with
//! `UiEcs::with_pre_layout_systems`. Without it the cell never updates and a
//! widget stays at its base colour — degraded, but not broken.

use std::sync::{
    atomic::{AtomicU32, Ordering},
    Arc,
};
use std::time::Duration;

use bevy_ecs::{
    component::Component,
    entity::Entity,
    schedule::{IntoScheduleConfigs, ScheduleConfigs},
    system::{Query, Res, ResMut, ScheduleSystem},
    world::EntityWorldMut,
};

use matcha_ecs::{
    components::render::RenderItem,
    pointer::PointerState,
    resources::{FrameTime, RedrawRequest},
};

use crate::animation::Easing;

/// A colour written by a system between frames and read by a `RenderItem`
/// builder while it draws.
///
/// Four `f32`s stored as their bit patterns, so the whole thing is lock-free
/// and `Send + Sync` — a builder closure has to be both. The four loads are not
/// atomic *together*, but the worst case is one frame drawn with a colour
/// halfway between two steps of a transition, which is indistinguishable from
/// any other frame of that transition.
#[derive(Clone, Debug)]
pub struct ColorCell(Arc<[AtomicU32; 4]>);

impl ColorCell {
    pub fn new(color: [f32; 4]) -> Self {
        Self(Arc::new(color.map(|c| AtomicU32::new(c.to_bits()))))
    }

    pub fn get(&self) -> [f32; 4] {
        std::array::from_fn(|i| f32::from_bits(self.0[i].load(Ordering::Relaxed)))
    }

    pub fn set(&self, color: [f32; 4]) {
        for (slot, c) in self.0.iter().zip(color) {
            slot.store(c.to_bits(), Ordering::Relaxed);
        }
    }
}

/// The colour a widget takes on in each pointer state, and how long it takes to
/// get there.
///
/// `None` for a state means "keep the one below it": an `active` of `None`
/// falls back to `hover`, and a `hover` of `None` to `base`. That is what lets
/// a widget opt into just one of them.
#[derive(Component, Clone, Copy, PartialEq, Debug)]
pub struct InteractionColors {
    pub base: [f32; 4],
    pub hover: Option<[f32; 4]>,
    pub active: Option<[f32; 4]>,
    /// CSS `transition`. `None` snaps.
    pub transition: Option<(Duration, Easing)>,
}

impl InteractionColors {
    pub fn new(base: [f32; 4]) -> Self {
        Self {
            base,
            hover: None,
            active: None,
            transition: None,
        }
    }

    /// The colour this widget should be showing right now.
    pub fn target(&self, hovered: bool, active: bool) -> [f32; 4] {
        let hover = self.hover.unwrap_or(self.base);
        match (active, hovered) {
            (true, _) => self.active.unwrap_or(hover),
            (false, true) => hover,
            (false, false) => self.base,
        }
    }
}

/// The live colour, plus whatever interpolation is in flight.
///
/// Self-contained like `OpacityTween`: it carries both endpoints, so retargeting
/// mid-transition (the pointer leaves before the hover finished) is just an
/// overwrite starting from wherever the cell currently is.
#[derive(Component, Debug)]
pub struct InteractionColorState {
    cell: ColorCell,
    from: [f32; 4],
    to: [f32; 4],
    start: web_time::Instant,
}

impl InteractionColorState {
    /// The colour being shown right now — what the builder reads.
    pub fn color(&self) -> [f32; 4] {
        self.cell.get()
    }
}

/// Attach (or refresh) the interaction colours on `entity` and hand back the
/// cell its `RenderItem` builder should read.
///
/// Called from a widget's `after_spawn` **and** its `patch`: the cell has to
/// outlive a builder rebuild, so an existing one is reused rather than replaced
/// — otherwise a transition would restart from the base colour every time an
/// unrelated prop changed.
pub fn interaction_cell(entity: &mut EntityWorldMut, colors: InteractionColors) -> ColorCell {
    entity.insert(colors);
    if let Some(state) = entity.get::<InteractionColorState>() {
        return state.cell.clone();
    }

    // First time: start settled at whatever the current state calls for, so a
    // widget spawned under the cursor comes up hovered rather than fading in.
    let (hovered, active) = pointer_flags(entity);
    let initial = colors.target(hovered, active);
    let cell = ColorCell::new(initial);
    entity.insert(InteractionColorState {
        cell: cell.clone(),
        from: initial,
        to: initial,
        start: web_time::Instant::now(),
    });
    cell
}

fn pointer_flags(entity: &mut EntityWorldMut) -> (bool, bool) {
    let id = entity.id();
    entity.world_scope(|world| match world.get_resource::<PointerState>() {
        Some(pointer) => (pointer.is_hovered(id), pointer.is_active(id)),
        None => (false, false),
    })
}

/// Drive every [`InteractionColorState`] toward the colour its current pointer
/// state calls for.
///
/// Reads the [`PointerState`] resource rather than the `Hovered`/`Active`
/// markers: the markers are synced in `PreExtract` and this runs in
/// `PreLayout`, so the markers would be a frame stale, whereas the resource is
/// already up to date from event time.
pub fn advance_interaction_colors(
    mut query: Query<(
        Entity,
        &InteractionColors,
        &mut InteractionColorState,
        Option<&mut RenderItem>,
    )>,
    pointer: Option<Res<PointerState>>,
    frame_time: Res<FrameTime>,
    mut redraw: ResMut<RedrawRequest>,
) {
    for (entity, colors, mut state, item) in query.iter_mut() {
        let (hovered, active) = match pointer.as_deref() {
            Some(pointer) => (pointer.is_hovered(entity), pointer.is_active(entity)),
            None => (false, false),
        };
        let target = colors.target(hovered, active);

        if state.to != target {
            state.from = state.cell.get();
            state.to = target;
            state.start = frame_time.0;
        }

        let (value, in_flight) = match colors.transition {
            Some((duration, easing)) => {
                let elapsed = frame_time.0.duration_since(state.start).as_secs_f32();
                let raw = (elapsed / duration.as_secs_f32().max(f32::EPSILON)).clamp(0.0, 1.0);
                if raw >= 1.0 {
                    (state.to, false)
                } else {
                    let t = easing.apply(raw);
                    (
                        std::array::from_fn(|i| state.from[i] + (state.to[i] - state.from[i]) * t),
                        true,
                    )
                }
            }
            None => (target, false),
        };

        if state.cell.get() != value {
            state.cell.set(value);
            // Colour is baked into the atlas by the builder, so a new colour
            // means a rebuild. (Unlike opacity, which is applied at draw time.)
            if let Some(mut item) = item {
                item.invalidate();
            }
        }
        if in_flight {
            redraw.request();
        }
    }
}

/// This module's systems, for
/// [`UiEcs::with_pre_layout_systems`](matcha_ecs::ui_ecs::UiEcs::with_pre_layout_systems).
pub fn default_systems() -> ScheduleConfigs<ScheduleSystem> {
    advance_interaction_colors.into_configs()
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: [f32; 4] = [0.1, 0.1, 0.1, 1.0];
    const HOVER: [f32; 4] = [0.2, 0.2, 0.2, 1.0];
    const ACTIVE: [f32; 4] = [0.3, 0.3, 0.3, 1.0];

    fn colors() -> InteractionColors {
        InteractionColors {
            base: BASE,
            hover: Some(HOVER),
            active: Some(ACTIVE),
            transition: None,
        }
    }

    #[test]
    fn each_state_selects_its_own_colour() {
        let c = colors();
        assert_eq!(c.target(false, false), BASE);
        assert_eq!(c.target(true, false), HOVER);
        assert_eq!(c.target(true, true), ACTIVE);
    }

    #[test]
    fn an_unset_state_falls_back_to_the_one_below_it() {
        let mut c = colors();
        c.active = None;
        assert_eq!(c.target(true, true), HOVER, "active falls back to hover");

        c.hover = None;
        assert_eq!(c.target(true, false), BASE, "hover falls back to base");
        assert_eq!(c.target(true, true), BASE, "and active follows it down");
    }

    #[test]
    fn a_cell_round_trips_every_component() {
        let cell = ColorCell::new([0.25, 0.5, 0.75, 1.0]);
        assert_eq!(cell.get(), [0.25, 0.5, 0.75, 1.0]);
        cell.set([1.0, 0.0, 0.5, 0.25]);
        assert_eq!(cell.get(), [1.0, 0.0, 0.5, 0.25]);
    }

    #[test]
    fn a_clone_of_a_cell_sees_the_same_writes() {
        // What makes the cell usable as a side channel at all: the builder holds
        // a clone, the system holds another, and they are the same storage.
        let cell = ColorCell::new(BASE);
        let captured = cell.clone();
        cell.set(HOVER);
        assert_eq!(captured.get(), HOVER);
    }
}
