//! Animation (M7): a `Target<T>`/`Animated<T>` pair plus a transient `Tween<T>`
//! that closes the gap between them (`ECS_ARCHITECTURE.md` §9, simplified per
//! the M7 planning discussion — see `ECS_IMPLEMENTATION_PLAN.md` §8).
//!
//! Only [`Opacity`] is implemented for now (`Translation`/`Scale` would also
//! need layout-stage integration, not needed by the current demo scope).
//! There is deliberately no `EnterTransition` component: a widget's `bundle()`
//! bakes the enter animation directly by spawning `Animated<T>` unequal to
//! `Target<T>` alongside a `Tween<T>` that closes the gap. `ExitTransition<T>`
//! does need to persist, since [`begin_or_continue_exit`] (called from
//! `view.rs`'s prune path, generically, without knowing the declaring widget)
//! is the only thing that can decide whether a about-to-be-pruned entity
//! should animate out instead of despawning immediately.

use bevy_ecs::{
    component::Component,
    entity::Entity,
    resource::Resource,
    system::{Commands, Query, Res},
    world::World,
};

mod sealed {
    pub trait Sealed {}
}

/// A property that can be tweened. Sealed: only core-provided types (currently
/// just [`Opacity`]) may implement it, so third-party code can write animation
/// *systems* but not new animatable property types (`ECS_ARCHITECTURE.md` §9.3).
pub trait Animatable: sealed::Sealed + Copy + Send + Sync + 'static {
    fn interpolate(from: &Self, to: &Self, t: f32) -> Self;
}

/// A widget's opacity, `0.0` (invisible) to `1.0` (fully visible).
#[derive(Component, Clone, Copy, PartialEq, Debug)]
pub struct Opacity(pub f32);

impl sealed::Sealed for Opacity {}
impl Animatable for Opacity {
    fn interpolate(from: &Self, to: &Self, t: f32) -> Self {
        Opacity(from.0 + (to.0 - from.0) * t)
    }
}

/// An easing curve applied to the raw `0.0..=1.0` tween progress before
/// interpolating.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Easing {
    Linear,
    EaseInOut,
}

impl Easing {
    pub fn apply(self, t: f32) -> f32 {
        match self {
            Easing::Linear => t,
            Easing::EaseInOut => t * t * (3.0 - 2.0 * t),
        }
    }
}

/// The value a property should end up at. Written by a widget's `bundle`/
/// `patch` (declaring its normal, resting value) or, transiently, by the exit
/// state machine (declaring the exit destination while `ToBeDespawn`).
#[derive(Component, Clone, Copy, PartialEq, Debug)]
pub struct Target<T: Animatable>(pub T);

/// The property's current, interpolated value. Read by rendering (and, for
/// future `Animatable` types, layout); written only by [`advance_tweens`].
/// Also the resting value when no `Tween<T>` is in flight, so it must persist
/// even for entities that never animate.
#[derive(Component, Clone, Copy, PartialEq, Debug)]
pub struct Animated<T: Animatable>(pub T);

/// An in-flight interpolation from `from` to the entity's current
/// `Target<T>`, computed fresh from absolute elapsed time every frame (never
/// stepped from the previous frame's value, so non-monotonic easings are
/// exact, not accumulated). Removed by [`advance_tweens`] on completion.
#[derive(Component, Clone, Copy)]
#[component(storage = "SparseSet")]
pub struct Tween<T: Animatable> {
    pub from: T,
    pub start: web_time::Instant,
    pub duration: std::time::Duration,
    pub easing: Easing,
}

/// Declares that this entity should animate out (instead of despawning
/// immediately) when pruned from the view. Optional per-instance (SparseSet),
/// read generically by [`begin_or_continue_exit`].
#[derive(Component, Clone, Copy)]
#[component(storage = "SparseSet")]
pub struct ExitTransition<T: Animatable> {
    pub to: T,
    pub duration: std::time::Duration,
    pub easing: Easing,
}

/// Marks an entity that the view no longer declares but whose exit animation
/// hasn't finished yet. Its `ViewChildren` slot is kept alive by `view.rs`'s
/// prune loop until [`despawn_completed_exits`] (in `view.rs`) removes it.
#[derive(Component, Clone, Copy)]
#[component(storage = "SparseSet")]
pub struct ToBeDespawn {
    pub started_at: web_time::Instant,
}

/// The current frame's timestamp, refreshed once per `Application::render`
/// call (before the render schedule runs) so every `advance_tweens` query
/// this frame agrees on "now".
#[derive(Resource, Clone, Copy)]
pub struct FrameTime(pub web_time::Instant);

/// Flat system: advance every in-flight `Tween<T>` by elapsed time, writing
/// `Animated<T>`. On completion, snaps `Animated<T>` exactly to `Target<T>`
/// and removes `Tween<T>` (via `Commands`, since this system doesn't have
/// exclusive `&mut World` access). Register once per `Animatable` type
/// (currently just `Opacity`) in `MatchaSet::Animation`.
pub fn advance_tweens<T: Animatable>(
    mut query: Query<(Entity, &mut Animated<T>, &Target<T>, &Tween<T>)>,
    frame_time: Res<FrameTime>,
    mut commands: Commands,
) {
    for (entity, mut animated, target, tween) in query.iter_mut() {
        let elapsed = frame_time
            .0
            .duration_since(tween.start)
            .as_secs_f32();
        let raw_t = (elapsed / tween.duration.as_secs_f32().max(f32::EPSILON)).clamp(0.0, 1.0);

        if raw_t >= 1.0 {
            animated.0 = target.0;
            commands.entity(entity).remove::<Tween<T>>();
        } else {
            let t = tween.easing.apply(raw_t);
            animated.0 = T::interpolate(&tween.from, &target.0, t);
        }
    }
}

/// Called from `view.rs`'s prune path for every child no longer declared by
/// the view this pass. Returns `true` if the entity should be kept alive
/// (either just started, or still running, an exit animation) or `false` if
/// it should be despawned immediately as before (no `ExitTransition<Opacity>`
/// configured — the overwhelming majority of widgets).
///
/// Only checks `Opacity`: `Animatable` is sealed and this is the only type
/// implemented so far. Add a check here for each new `Animatable` type
/// (`Translation`/`Scale`) if/when they're introduced.
pub fn begin_or_continue_exit(world: &mut World, entity: Entity) -> bool {
    if world.get::<ToBeDespawn>(entity).is_some() {
        // Already exiting from an earlier pass: don't re-trigger, just keep
        // the slot alive while `advance_tweens` finishes the job.
        return true;
    }

    let Some(exit) = world.get::<ExitTransition<Opacity>>(entity).copied() else {
        return false;
    };

    let current = world
        .get::<Animated<Opacity>>(entity)
        .copied()
        .unwrap_or(Animated(Opacity(1.0)));
    let now = web_time::Instant::now();

    world.entity_mut(entity).insert((
        ToBeDespawn { started_at: now },
        Target(exit.to),
        Tween::<Opacity> {
            from: current.0,
            start: now,
            duration: exit.duration,
            easing: exit.easing,
        },
    ));

    true
}
