//! Focus protocol: which entities can hold focus, how a parent may claim it
//! from its descendants, and the per-entity memory of where focus last was.
//!
//! Like `OnClick<Msg>`, this is a protocol several widgets share rather than
//! any one widget's business, so it lives in core (`ECS_IMPLEMENTATION_PLAN.md`
//! §3.1's crate-direction test). Resolution lives in [`crate::focus`].

use bevy_ecs::{component::Component, entity::Entity, world::EntityWorldMut};

/// Declares an entity focusable, and how it behaves when focus resolution
/// walks through it.
///
/// **Presence is what makes an entity focusable.** An entity *without* this
/// component is transparent to focus: it can never become the focus vertex,
/// but it still lies on the focus path when a descendant is focused, so
/// [`Focus::is_focus_within`](crate::focus::Focus::is_focus_within) is true
/// for it. That is the `:focus-within` case, and it is why a plain `Column`
/// wrapping a focused text box needs no opt-in.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum FocusPolicy {
    /// Can be the focus vertex. Descendants may be the vertex instead if the
    /// pick landed deeper.
    #[default]
    Normal,

    /// **Dominance.** Focus stops here: if the pick landed on a descendant,
    /// the downward pass truncates the path at this entity and this entity
    /// becomes the vertex.
    ///
    /// This is what lets a text box hold decorative children (icons, a clear
    /// button's backdrop, styled spans) without any of them stealing focus.
    Claim,

    /// Becoming the vertex extends the path back down through
    /// [`LastFocusedChild`], if that child is still alive and still reachable.
    ///
    /// The case this exists for: clicking a `Panel`'s padding, where the user
    /// means "focus what's in here", not "focus the panel".
    RestoreLast,
}

/// Element-local memory of which direct child the focus path last continued
/// into.
///
/// Deliberately **not** a global map. A global `HashMap<Entity, Entity>` would
/// go stale the moment an entity despawns and would need its own garbage
/// collection; a component's lifetime is exactly the entity's, so the problem
/// does not arise. Entities are only written here when the focus path actually
/// passes through them.
///
/// The stored entity must still be validated at use time: bevy reuses entity
/// ids after despawn, and the reconciler rebuilds an entity outright when a
/// slot's widget type changes. [`crate::focus`] checks liveness and parentage
/// before descending. Only attach this to entities that want
/// [`FocusPolicy::RestoreLast`] behaviour — nothing reads it otherwise.
#[derive(Component, Default, Debug)]
pub struct LastFocusedChild(pub(crate) Option<Entity>);

impl LastFocusedChild {
    pub fn get(&self) -> Option<Entity> {
        self.0
    }
}

/// Derived marker: this entity **is** the focus vertex.
///
/// Maintained by [`crate::focus::sync_focus_components`] from the [`Focus`]
/// resource, which is the source of truth. It exists so widgets can react with
/// `Changed<Focused>` (the established idiom for rebuilding a cached render
/// node — colours are baked into the atlas, so a focus ring needs a rebuild).
///
/// [`Focus`]: crate::focus::Focus
#[derive(Component)]
pub struct Focused;

/// Derived marker: the focus vertex is this entity or one of its descendants
/// (CSS `:focus-within`). Present on every entity along the focus path,
/// including ones that are not focusable themselves.
#[derive(Component)]
pub struct FocusWithin;

/// Notified when this entity gains or loses focus (`true` = gained).
///
/// Same fn-pointer dispatch idiom as [`KeyDispatch`](crate::components::input::KeyDispatch)
/// and [`LayoutDispatch`](crate::layout::LayoutDispatch). It lives here rather
/// than in the input module because focus transitions are not text-specific —
/// any widget may want to react to them.
///
/// Called from [`sync_focus_components`](crate::focus::sync_focus_components),
/// which is the only place that knows the exact transition set in **both**
/// directions. (`Changed<Focused>` cannot serve this purpose: it does not fire
/// when a component is removed, so losing focus would go unnoticed.)
#[derive(Component, Clone, Copy)]
pub struct FocusDispatch {
    handle: fn(&mut EntityWorldMut, bool),
}

impl FocusDispatch {
    pub fn new(handle: fn(&mut EntityWorldMut, bool)) -> Self {
        Self { handle }
    }

    pub(crate) fn call(&self, entity: &mut EntityWorldMut, gained: bool) {
        (self.handle)(entity, gained);
    }
}
