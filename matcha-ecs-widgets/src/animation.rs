//! Opacity fade animation: the widget-side plugin that drives
//! `matcha-ecs`'s neutral [`RenderOpacity`] extract contract.
//!
//! The core knows nothing about any of this. It offers four neutral hooks and
//! this module builds a fade out of them:
//!
//! - [`RenderOpacity`] — the value extract actually reads. We write it; it is
//!   applied per instance at draw time, so writing it every frame costs a
//!   redraw and nothing else.
//! - [`FrameTime`] — one timestamp per frame, so every tween agrees on "now".
//! - [`RedrawRequest`] — "this frame isn't the last one", so an in-flight fade
//!   keeps the event loop scheduling frames.
//! - [`ManualDespawn`] + [`despawn_ui_entity`] — the reconciler defers the
//!   despawn of an entity carrying `ManualDespawn` and merely flags it pruned;
//!   we fade it out and then despawn it ourselves.
//!
//! Register with [`default_systems`]:
//!
//! ```ignore
//! UiEcs::new(model, view, reduce)
//!     .with_pre_layout_systems(matcha_ecs_widgets::animation::default_systems())
//! ```
//!
//! Tweens are recomputed from absolute elapsed time every frame rather than
//! stepped from the previous frame's value, so a non-monotonic easing stays
//! exact and a dropped frame doesn't accumulate error.
//!
//! Only opacity is animated, and it is modelled concretely (no generic
//! `Animatable` trait): the core's contract is a concrete `RenderOpacity`
//! component, so a generic tween machinery would have nowhere general to write
//! its result. Add a second animated property by adding its own contract
//! component and its own system here.

use std::time::Duration;

use bevy_ecs::{
    change_detection::DetectChangesMut,
    component::Component,
    entity::Entity,
    query::Changed,
    schedule::{IntoScheduleConfigs, ScheduleConfigs},
    system::{Commands, Query, Res, ResMut, ScheduleSystem},
    world::World,
};

use matcha_ecs::{
    components::{render::RenderOpacity, view::ManualDespawn},
    resources::{FrameTime, RedrawRequest},
    view::despawn_ui_entity,
};

/// Re-exported for callers already reaching for this module; defined in
/// [`crate::easing`] because colour transitions ease too and do not otherwise
/// depend on fades.
pub use crate::easing::Easing;

/// An in-flight opacity interpolation. Self-contained (carries both endpoints),
/// so a reversal is just a fresh `OpacityTween` overwriting the old one.
/// Removed by [`advance_opacity_tweens`] on completion.
#[derive(Component, Clone, Copy, Debug)]
#[component(storage = "SparseSet")]
pub struct OpacityTween {
    pub from: f32,
    pub to: f32,
    pub start: web_time::Instant,
    pub duration: Duration,
    pub easing: Easing,
}

/// Declares that this entity fades out (to fully transparent) when the view
/// stops declaring it, instead of vanishing at once.
///
/// A widget that attaches this **must** also attach
/// [`ManualDespawn`](matcha_ecs::components::view::ManualDespawn), or the
/// reconciler will despawn the entity immediately and there will be nothing
/// left to fade. Conversely, `ManualDespawn` without a system that eventually
/// despawns the entity leaks it — [`default_systems`] is that system.
#[derive(Component, Clone, Copy, Debug)]
#[component(storage = "SparseSet")]
pub struct ExitFade {
    pub duration: Duration,
    pub easing: Easing,
}

/// Start the exit fade of every entity the reconciler has just flagged as
/// pruned.
///
/// `Changed<ManualDespawn>` also fires on spawn and on revival; the
/// `is_pruned()` check is what makes those cases no-ops.
pub fn begin_exit_fades(
    query: Query<(Entity, &ManualDespawn, &ExitFade, &RenderOpacity), Changed<ManualDespawn>>,
    frame_time: Res<FrameTime>,
    mut commands: Commands,
) {
    for (entity, manual, exit, opacity) in query.iter() {
        if !manual.is_pruned() {
            continue;
        }
        commands.entity(entity).insert(OpacityTween {
            from: opacity.0,
            to: 0.0,
            start: frame_time.0,
            duration: exit.duration,
            easing: exit.easing,
        });
    }
}

/// Advance every in-flight [`OpacityTween`], writing [`RenderOpacity`].
///
/// On completion, snaps to the tween's `to` and removes the tween. An entity
/// that is *still* pruned at that moment (i.e. it faded out and the view never
/// took it back) is despawned — this is the only thing that ever tears down a
/// `ManualDespawn` entity in this plugin. A revived entity is not despawned,
/// even though the tween that finished may have been its exit tween: the
/// reconciler cleared the pruned flag, and that check happens here, at the end.
pub fn advance_opacity_tweens(
    mut query: Query<(
        Entity,
        &mut RenderOpacity,
        &OpacityTween,
        Option<&ManualDespawn>,
    )>,
    frame_time: Res<FrameTime>,
    mut redraw: ResMut<RedrawRequest>,
    mut commands: Commands,
) {
    for (entity, mut opacity, tween, manual) in query.iter_mut() {
        let elapsed = frame_time.0.duration_since(tween.start).as_secs_f32();
        let raw_t = (elapsed / tween.duration.as_secs_f32().max(f32::EPSILON)).clamp(0.0, 1.0);

        if raw_t >= 1.0 {
            opacity.set_if_neq(RenderOpacity(tween.to));
            commands.entity(entity).remove::<OpacityTween>();

            if manual.is_some_and(|m| m.is_pruned()) {
                commands.queue(move |world: &mut World| despawn_ui_entity(world, entity));
            }
        } else {
            let t = tween.easing.apply(raw_t);
            let value = tween.from + (tween.to - tween.from) * t;
            opacity.set_if_neq(RenderOpacity(value));
            // Not the final frame of this fade: keep the loop coming back.
            redraw.request();
        }
    }
}

/// This plugin's systems, ready to hand to
/// [`UiEcs::with_pre_layout_systems`](matcha_ecs::ui_ecs::UiEcs::with_pre_layout_systems).
///
/// Chained: a fade must be started before it is advanced, so that the frame the
/// view prunes an entity is already the first frame of its fade.
pub fn default_systems() -> ScheduleConfigs<ScheduleSystem> {
    (begin_exit_fades, advance_opacity_tweens)
        .chain()
        .into_configs()
}
