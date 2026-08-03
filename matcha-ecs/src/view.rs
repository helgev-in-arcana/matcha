//! View layer: `Widget` trait, `Scope`, and the child reconciliation core.
//!
//! Authoring model is **execution-as-declaration** (IMP): a view function
//! returns `()` and declares UI by *running* — each `leaf`/`node` call is
//! reconciled into the ECS world immediately and consumed. Heterogeneous
//! siblings are stored by the world (uniform archetypes), never in a Rust-side
//! container, so no `Box`/`dyn` appears on the common path.
//!
//! ```ignore
//! fn counter(model: &Model, s: &mut Scope) {
//!     s.node(Column, |s| {
//!         s.leaf(Text::new(format!("count: {}", model.count)));
//!         s.node(Row, |s| {
//!             s.leaf(Button::new("-").on(Msg::Dec));
//!             s.leaf(Button::new("+").on(Msg::Inc));
//!         });
//!     });
//! }
//! ```

use std::{any::TypeId, collections::HashMap};

use bevy_ecs::{
    bundle::Bundle, entity::Entity, hierarchy::ChildOf, world::{EntityWorldMut, World},
};

use crate::components::view::{Key, ManualDespawn, SlotKey, ViewChildren, WidgetType};

/// A widget: a fixed component bundle (= one archetype) plus an in-place patch
/// for the same-type re-visit. Behaviour lives in ECS systems, not here.
pub trait Widget: 'static {
    /// Intrinsic reconciliation key. Default `Key::Auto` = positional.
    /// List items override this (e.g. via a `.key(id)` builder) so their
    /// identity survives reordering.
    fn key(&self) -> Key {
        Key::Auto
    }

    /// Components inserted when the entity is first spawned.
    fn bundle(&self) -> impl Bundle;

    /// In-place update when an existing entity of the same widget type is
    /// re-visited. Use `set_if_neq` so `Changed<T>` stays honest.
    fn patch(&self, entity: &mut EntityWorldMut);

    /// Optional one-time setup that runs right after `bundle()` is spawned.
    /// Default: no-op. Exists for conditionally attaching components (e.g. an
    /// exit animation's tween) that can't be expressed as part of `bundle()`'s
    /// fixed return type — `Option<T>` does not itself implement `Bundle`, so
    /// a widget that only sometimes wants such a component inserts it here.
    fn after_spawn(&self, _entity: &mut EntityWorldMut) {}
}

/// "You are currently building the direct children of `parent`."
///
/// `Scope` carries the position witness (parent entity + a per-parent
/// `Cursor`). Child blocks get a fresh `Scope` via a world reborrow, so a
/// parent's cursor is confined to its own block and siblings cannot observe
/// each other. On drop the cursor is flushed: untouched children are pruned and
/// the new child ordering is written back to `ViewChildren`.
pub struct Scope<'a> {
    world: &'a mut World,
    parent: Entity,
    cursor: Cursor,
}

/// Per-parent reconciliation scratch for a single view pass.
struct Cursor {
    /// Slots from the previous pass (taken out of `ViewChildren`), consumed as
    /// they are matched.
    prev: HashMap<SlotKey, Entity>,
    /// Occurrence counter per `Key` for this pass (disambiguates repeated keys).
    seen: HashMap<Key, u32>,
    /// Slots established this pass, in declaration order.
    next: Vec<(SlotKey, Entity)>,
}

impl<'a> Scope<'a> {
    /// Open a scope over `parent`, taking its existing child slots as the
    /// baseline to reconcile against. `parent` must already have a
    /// `ViewChildren` component.
    fn open(world: &'a mut World, parent: Entity) -> Self {
        let prev = world
            .get_mut::<ViewChildren>(parent)
            .map(|mut vc| std::mem::take(&mut vc.slots))
            .unwrap_or_default()
            .into_iter()
            .collect();
        Scope {
            world,
            parent,
            cursor: Cursor {
                prev,
                seen: HashMap::new(),
                next: Vec::new(),
            },
        }
    }

    /// Declare a child with no children of its own.
    pub fn leaf<W: Widget>(&mut self, w: W) {
        reconcile(&mut *self.world, &mut self.cursor, self.parent, &w);
    }

    /// Declare a child that itself has children, built inside `build`.
    pub fn node<W: Widget>(&mut self, w: W, build: impl FnOnce(&mut Scope)) {
        let entity = reconcile(&mut *self.world, &mut self.cursor, self.parent, &w);
        // Fresh child scope over a world reborrow; flushes on drop.
        let mut child = Scope::open(&mut *self.world, entity);
        build(&mut child);
    }

    /// Prune untouched children and write the new child ordering back.
    ///
    /// A child carrying [`ManualDespawn`] isn't despawned here: it is flagged
    /// as pruned and its slot is kept in `next` so it stays reachable (for
    /// layout/paint/hit-test) until whichever system owns its lifetime — an
    /// exit animation, typically — calls [`despawn_ui_entity`]. Everything
    /// else (the overwhelming majority) despawns on the spot.
    fn flush(&mut self) {
        for (slot, e) in self
            .cursor
            .prev
            .iter()
            .map(|(k, v)| (*k, *v))
            .collect::<Vec<_>>()
        {
            if let Some(mut manual) = self.world.get_mut::<ManualDespawn>(e) {
                // Write only on a real transition: a re-prune of an
                // already-pruned entity must not re-fire `Changed`, or an
                // exit animation would restart every view pass.
                if !manual.is_pruned() {
                    manual.set_pruned(true);
                }
                self.cursor.next.push((slot, e));
            } else {
                despawn_recursive(self.world, e);
            }
        }
        if let Some(mut vc) = self.world.get_mut::<ViewChildren>(self.parent) {
            vc.slots = std::mem::take(&mut self.cursor.next);
        }
    }
}

impl Drop for Scope<'_> {
    fn drop(&mut self) {
        self.flush();
    }
}

/// Reconcile a single declared widget against the parent's previous slots.
fn reconcile<W: Widget>(world: &mut World, cursor: &mut Cursor, parent: Entity, w: &W) -> Entity {
    let key = w.key();
    let occ = cursor.seen.entry(key).or_insert(0);
    let slot: SlotKey = (key, *occ);
    *occ += 1;

    let entity = match cursor.prev.remove(&slot) {
        // Same slot, same widget type -> in-place patch.
        Some(e) if world.get::<WidgetType>(e).map(|wt| wt.0) == Some(TypeId::of::<W>()) => {
            let mut em = world.entity_mut(e);
            // `patch` runs while `ManualDespawn::is_pruned()` is still `true`,
            // so a widget mid-exit can see it and reverse its exit animation
            // before core clears the flag below (a "revival").
            w.patch(&mut em);
            if let Some(mut manual) = em.get_mut::<ManualDespawn>() {
                if manual.is_pruned() {
                    manual.set_pruned(false);
                }
            }
            e
        }
        // Same slot, different type -> rebuild the entity from scratch.
        Some(e) => {
            despawn_recursive(world, e);
            spawn_new(world, parent, w)
        }
        // New slot.
        None => spawn_new(world, parent, w),
    };

    cursor.next.push((slot, entity));
    entity
}

fn spawn_new<W: Widget>(world: &mut World, parent: Entity, w: &W) -> Entity {
    let mut e = world.spawn(w.bundle());
    e.insert((
        WidgetType(TypeId::of::<W>()),
        ViewChildren::default(),
        ChildOf(parent),
    ));
    w.after_spawn(&mut e);
    e.id()
}

/// Despawn an entity and all of its view-managed descendants.
fn despawn_recursive(world: &mut World, entity: Entity) {
    if let Some(vc) = world.get::<ViewChildren>(entity) {
        let children: Vec<Entity> = vc.slots.iter().map(|(_, c)| *c).collect();
        for c in children {
            despawn_recursive(world, c);
        }
    }
    world.despawn(entity);
}

/// Run a root view function against `root`, reconciling its declared children
/// into the world. `root` acts as an invisible container; the view's top-level
/// `leaf`/`node` calls land directly beneath it.
pub fn run_view(world: &mut World, root: Entity, view: impl FnOnce(&mut Scope)) {
    if world.get::<ViewChildren>(root).is_none() {
        world.entity_mut(root).insert(ViewChildren::default());
    }
    let mut s = Scope::open(world, root);
    view(&mut s);
}

/// Despawn a view-managed entity and its descendants, detaching it from its
/// parent's [`ViewChildren`] so no future reconcile pass sees a dangling slot.
///
/// This is the sanctioned way for a system that owns a [`ManualDespawn`]
/// entity's lifetime (an exit animation, say) to finally tear it down: the
/// reconciler's slot bookkeeping is an internal invariant that such a system
/// must not have to reproduce by hand. A no-op if the entity is already gone.
pub fn despawn_ui_entity(world: &mut World, entity: Entity) {
    if world.get_entity(entity).is_err() {
        return;
    }
    if let Some(child_of) = world.get::<ChildOf>(entity).cloned() {
        if let Some(mut siblings) = world.get_mut::<ViewChildren>(child_of.parent()) {
            siblings.slots.retain(|(_, e)| *e != entity);
        }
    }
    despawn_recursive(world, entity);
}
