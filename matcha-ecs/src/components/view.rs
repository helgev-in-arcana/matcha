//! Framework components backing the view / reconcile layer.
//!
//! These are the "indispensable" components the reconciler manages on every
//! widget entity. They are separate from the data components a `Widget` bundle
//! carries (text content, layout kind, etc.).

use std::any::TypeId;

use bevy_ecs::{component::Component, entity::Entity};

/// The `Widget` type that produced this entity.
///
/// The reconciler compares this against the incoming widget's `TypeId` to
/// decide between an in-place patch (same type) and a full rebuild
/// (despawn + respawn on type change).
#[derive(Component)]
pub struct WidgetType(pub TypeId);

/// Widget-provided reconciliation key.
///
/// `Auto` is the default: such children are identified purely by their order
/// of appearance within the parent (the reconciler's per-pass occurrence
/// counter acts as a positional key). List items opt into a stable identity
/// with `Id`, which survives reordering.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Key {
    Auto,
    Id(u64),
}

impl Default for Key {
    fn default() -> Self {
        Key::Auto
    }
}

impl From<u64> for Key {
    fn from(v: u64) -> Self {
        Key::Id(v)
    }
}

impl From<u32> for Key {
    fn from(v: u32) -> Self {
        Key::Id(v as u64)
    }
}

/// A reconciliation slot: the widget key plus the occurrence index that
/// disambiguates repeated keys within one parent (always 0 for a unique `Id`,
/// running 0,1,2,... for `Auto` siblings).
pub type SlotKey = (Key, u32);

/// The ordered set of child slots the reconciler established for an entity on
/// the last view pass. This is the source of truth for child identity and
/// ordering (layout reads it later).
#[derive(Component, Default)]
pub struct ViewChildren {
    pub slots: Vec<(SlotKey, Entity)>,
}

/// Opts an entity out of the reconciler's automatic despawn, handing ownership
/// of its lifetime to whatever system manages it (typically an exit animation).
///
/// Normally, a child the view no longer declares is despawned on the spot by
/// the prune pass. An entity carrying this component is kept alive instead: the
/// reconciler only flips [`is_pruned`](Self::is_pruned) to `true` and keeps the
/// entity's slot (so it still lays out, paints and hit-tests), then walks away.
/// From that point it is the responsibility of a registered system to eventually
/// call [`despawn_ui_entity`](crate::view::despawn_ui_entity) — the core never
/// despawns it and never watches for the component's removal.
///
/// If the view re-declares the entity while it is pruned (a "revival"), the
/// reconciler flips the flag back to `false` — *after* running the widget's
/// `patch`, so `patch` can observe the still-pruned state and reverse an
/// in-flight exit animation. A system that despawns on animation completion
/// must therefore re-check `is_pruned()` at that moment rather than assuming
/// its own tween's existence implies the entity is still doomed.
///
/// **This is a leak footgun**: an entity carrying `ManualDespawn` with no
/// system that ever despawns it stays alive forever, since the prune pass will
/// keep skipping it. Only attach it when something is genuinely committed to
/// tearing the entity down.
#[derive(Component, Debug, Default)]
pub struct ManualDespawn {
    pruned: bool,
}

impl ManualDespawn {
    pub fn new() -> Self {
        Self { pruned: false }
    }

    /// `true` once the view has stopped declaring this entity. The entity is
    /// alive only because this component defers its despawn.
    pub fn is_pruned(&self) -> bool {
        self.pruned
    }

    pub(crate) fn set_pruned(&mut self, pruned: bool) {
        self.pruned = pruned;
    }
}
