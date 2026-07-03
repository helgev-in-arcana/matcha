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
