//! The one order in which a view tree is walked.
//!
//! # Why there is only one
//!
//! Painting and picking have to agree. Under the painter's algorithm the
//! entity drawn last is the one on top, so **the reverse of paint order is
//! exactly the right order to pick in** — and any second ordering, however
//! reasonable on its own, is a way for what the user clicks to stop being what
//! the user sees.
//!
//! Both therefore come through [`walk`]. The sets they care about are
//! different — a `ScrollView` is pickable but paints nothing, a decorative box
//! paints but is not pickable — so each filters as it goes, but neither
//! chooses an order.
//!
//! [`ancestors`] is the upward walk, and lives here for the same reason: what
//! a pick resolves to — the click target, the focus path, the hover chain — is
//! decided by going up from the one entity picking returned.
//!
//! # Stacking
//!
//! Ordering within a parent is declaration order, overridable per child by
//! [`ZIndex`]. Two restrictions keep this to a single stable sort, rather than
//! CSS's seven paint layers:
//!
//! 1. **Only an explicit `ZIndex` reorders anything.** In CSS, `opacity` and
//!    `transform` also establish stacking contexts, which means a widget's
//!    stacking can change for the duration of a fade. That surprise is not
//!    worth the fidelity.
//! 2. **A child is never painted behind its parent**, however negative its
//!    `ZIndex`. It reaches the back of its siblings and stops. Going further
//!    would require interleaving a parent's own drawing with its descendants',
//!    which is precisely the layered structure this avoids.
//!
//! A subtree stays contiguous, so `ZIndex` moves a whole widget, not just its
//! own box.

use bevy_ecs::{entity::Entity, hierarchy::ChildOf, world::World};

use crate::components::{layout::Hidden, render::ZIndex, view::ViewChildren};

/// Visit `entity` and its descendants in paint order, back to front.
///
/// `visit` receives the state its parent produced and returns the state its
/// own children should see — a clip rectangle, an arena index, a transform,
/// whatever the caller is accumulating. Returning `None` prunes the subtree
/// without visiting it.
///
/// [`Hidden`] subtrees (`display: none`) are skipped here rather than by each
/// caller: they take no part in layout, so whatever transform they still carry
/// is stale, and neither painting nor picking them would be meaningful.
pub fn walk<S>(
    world: &World,
    entity: Entity,
    state: S,
    visit: &mut impl FnMut(&World, Entity, &S) -> Option<S>,
) where
    S: Clone,
{
    if world.get::<Hidden>(entity).is_some() {
        return;
    }
    let Some(child_state) = visit(world, entity, &state) else {
        return;
    };
    for child in paint_ordered_children(world, entity) {
        walk(world, child, child_state.clone(), visit);
    }
}

/// `entity`'s declared children in paint order: declaration order, then stably
/// by [`ZIndex`].
///
/// The sort is skipped entirely when no child declares one, which is very
/// nearly always — so an ordinary subtree costs exactly the plain walk it did
/// before stacking existed.
pub fn paint_ordered_children(world: &World, entity: Entity) -> Vec<Entity> {
    let Some(children) = world.get::<ViewChildren>(entity) else {
        return Vec::new();
    };
    let mut out: Vec<Entity> = children.slots.iter().map(|(_, e)| *e).collect();

    if out.iter().any(|e| world.get::<ZIndex>(*e).is_some()) {
        // Stable, so children sharing a `ZIndex` keep declaration order.
        out.sort_by_key(|e| world.get::<ZIndex>(*e).map(|z| z.0).unwrap_or(0));
    }
    out
}

/// Walk from `entity` up to the view root, yielding each ancestor (starting
/// with `entity` itself).
///
/// The upward counterpart to [`walk`], and the traversal everything downstream
/// of a pick is built on: picking hands over one entity, and click routing,
/// focus resolution, hover chains and `:active` are all decided by going up
/// from it. Unlike [`walk`] this does not skip [`Hidden`] subtrees — an entity
/// you already hold is by definition one you got from somewhere that filtered.
pub fn ancestors(world: &World, entity: Entity) -> impl Iterator<Item = Entity> + '_ {
    let mut current = Some(entity);
    std::iter::from_fn(move || {
        let e = current?;
        current = world.get::<ChildOf>(e).map(|c| c.parent());
        Some(e)
    })
}
