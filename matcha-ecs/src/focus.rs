//! Focus resolution: turning one picked entity into a root→leaf focus path.
//!
//! # Shape of the model
//!
//! Focus is a **path**, not a single entity. Picking ([`crate::pick`]) hands us
//! exactly one entity — the frontmost surface under the pointer — and
//! everything else is decided by traversing the view tree from there:
//!
//! 1. **Upward**: walk `ChildOf` to the root, producing the candidate path.
//! 2. **Downward**: walk that path root→leaf applying [`FocusPolicy`], so a
//!    parent can *claim* focus and cut its descendants out
//!    ([`FocusPolicy::Claim`] — how a text box owns its decorative children).
//! 3. **Extend**: if the resulting vertex asks to
//!    ([`FocusPolicy::RestoreLast`]), descend again through
//!    [`LastFocusedChild`] — clicking a panel's padding means "focus what's in
//!    here".
//!
//! Because resolution consumes only `Option<Entity>`, it is completely
//! independent of which picking backend is installed — a BVH or GPU ID buffer
//! changes nothing here.
//!
//! Note there is deliberately **no flat focus array**. The path itself contains
//! the non-focusable ancestors, so `:focus-within`
//! ([`Focus::is_focus_within`]) is answered by scanning a vector whose length
//! is the tree depth.

use bevy_ecs::{
    entity::Entity,
    hierarchy::ChildOf,
    query::With,
    resource::Resource,
    world::World,
};

use crate::{
    components::focus::{FocusDispatch, FocusPolicy, FocusWithin, Focused, LastFocusedChild},
    pick::ancestors,
};

/// Guards the [`FocusPolicy::RestoreLast`] descent against a pathological
/// tree. Real UI nesting is nowhere near this deep.
const MAX_RESTORE_DEPTH: usize = 64;

/// The application's focus state, globally readable.
///
/// The only thing that persists between frames is the vertex's identity; the
/// path is re-derived whenever the tree changes ([`validate_focus`]). Nothing
/// here is an index into any backend structure — indices would not survive a
/// picker swap or a frame rebuild, entity ids do.
#[derive(Resource, Default, Debug)]
pub struct Focus {
    top: Option<Entity>,
    /// Root → leaf. `path.last() == top` whenever `top` is `Some`.
    path: Vec<Entity>,
}

impl Focus {
    /// The focused entity, if any.
    pub fn top(&self) -> Option<Entity> {
        self.top
    }

    /// The full focus path, root first, vertex last. Empty when nothing has
    /// focus.
    pub fn path(&self) -> &[Entity] {
        &self.path
    }

    /// `true` only for the focus vertex itself (CSS `:focus`).
    pub fn is_focused(&self, entity: Entity) -> bool {
        self.top == Some(entity)
    }

    /// `true` if `entity` is the vertex or an ancestor of it (CSS
    /// `:focus-within`). Holds for non-focusable ancestors too — a plain
    /// `Column` wrapping a focused text box needs no opt-in.
    pub fn is_focus_within(&self, entity: Entity) -> bool {
        self.path.contains(&entity)
    }
}

/// What to do when a press resolves to no focusable entity.
#[derive(Resource, Debug, Clone, Copy)]
pub struct FocusConfig {
    /// Clear focus when a press lands on nothing focusable — either on empty
    /// background (picking returned nothing) or on an entity with no focusable
    /// ancestor. `true` matches how text inputs behave everywhere.
    ///
    /// Set `false` to make focus sticky until something else claims it.
    pub clear_on_miss: bool,
}

impl Default for FocusConfig {
    fn default() -> Self {
        Self {
            clear_on_miss: true,
        }
    }
}

fn policy(world: &World, entity: Entity) -> Option<FocusPolicy> {
    world.get::<FocusPolicy>(entity).copied()
}

/// Resolve a candidate path (root → leaf, as produced by the upward walk) into
/// a final focus path by applying [`FocusPolicy`].
///
/// Runs the downward `Claim` pass and the `RestoreLast` extension until they
/// reach a fixed point. Returns an empty vector if nothing on the path is
/// focusable at all.
fn finalize_path(world: &World, mut path: Vec<Entity>) -> Vec<Entity> {
    let mut previous_vertex: Option<Entity> = None;

    for _ in 0..MAX_RESTORE_DEPTH {
        // Downward pass: the shallowest claimer wins and everything below it
        // is cut away. This is the parent's dominance over its subtree.
        if let Some(i) = path
            .iter()
            .position(|&e| policy(world, e) == Some(FocusPolicy::Claim))
        {
            path.truncate(i + 1);
        }

        // The vertex is the deepest focusable entity left on the path.
        let Some(vertex_index) = path.iter().rposition(|&e| policy(world, e).is_some()) else {
            return Vec::new();
        };
        path.truncate(vertex_index + 1);
        let vertex = path[vertex_index];

        // Fixed point: the last extension did not move the vertex, so it held
        // nothing focusable and repeating would produce the same chain again.
        if previous_vertex == Some(vertex) {
            return path;
        }
        previous_vertex = Some(vertex);

        if policy(world, vertex) != Some(FocusPolicy::RestoreLast) {
            return path;
        }

        // Extend back down through remembered children. The chain may pass
        // through non-focusable containers (a `Panel` remembers the `Column`
        // that holds the text box), so descend as far as the memory goes and
        // let the next iteration pick the deepest focusable entity out of it.
        let extension = restore_chain(world, vertex);
        if extension.is_empty() {
            return path;
        }
        path.extend(extension);
    }
    path
}

/// Follow [`LastFocusedChild`] downward from `from`, validating each step.
///
/// Entity ids are reused after despawn and the reconciler rebuilds an entity
/// outright when a slot's widget type changes, so a remembered child is only
/// trusted if it is still alive *and* still a direct child of the entity that
/// remembered it.
fn restore_chain(world: &World, from: Entity) -> Vec<Entity> {
    let mut out = Vec::new();
    let mut current = from;
    for _ in 0..MAX_RESTORE_DEPTH {
        let Some(next) = world
            .get::<LastFocusedChild>(current)
            .and_then(|last| last.0)
        else {
            break;
        };
        if world.get_entity(next).is_err() {
            break;
        }
        if world.get::<ChildOf>(next).map(|c| c.parent()) != Some(current) {
            break;
        }
        out.push(next);
        current = next;
    }
    out
}

/// Resolve the focus path for a pick result, without writing anything.
///
/// Testable core: no resources, no window, no GPU. `hit` is whatever
/// [`crate::pick::Picker::pick`] returned.
pub fn resolve_focus_path(world: &World, hit: Option<Entity>) -> Vec<Entity> {
    let Some(hit) = hit else {
        return Vec::new();
    };
    // Upward walk, then reverse: the downward pass and every consumer wants
    // root-first order (a parent must be processed before its children).
    let mut chain: Vec<Entity> = ancestors(world, hit).collect();
    chain.reverse();
    finalize_path(world, chain)
}

/// Write a resolved path into the [`Focus`] resource and record it in each
/// participating entity's [`LastFocusedChild`]. Returns whether focus moved.
fn commit_path(world: &mut World, path: Vec<Entity>) -> bool {
    // Element-local memory: every entity on the path that opted in remembers
    // which child the path continued into. Written only on a real change so
    // `Changed<LastFocusedChild>` stays meaningful.
    for pair in path.windows(2) {
        let (parent, child) = (pair[0], pair[1]);
        if let Some(mut last) = world.get_mut::<LastFocusedChild>(parent) {
            if last.0 != Some(child) {
                last.0 = Some(child);
            }
        }
    }

    let top = path.last().copied();
    let mut focus = world.resource_mut::<Focus>();
    if focus.top == top && focus.path == path {
        return false;
    }
    focus.top = top;
    focus.path = path;
    true
}

/// Apply a pick result to the focus state. Returns whether focus moved.
///
/// A miss (nothing picked, or nothing focusable above what was picked) clears
/// focus or leaves it alone per [`FocusConfig::clear_on_miss`].
pub fn focus_from_pick(world: &mut World, hit: Option<Entity>) -> bool {
    let path = resolve_focus_path(world, hit);
    if path.is_empty() {
        let clear = world
            .get_resource::<FocusConfig>()
            .copied()
            .unwrap_or_default()
            .clear_on_miss;
        if !clear {
            return false;
        }
    }
    commit_path(world, path)
}

/// Focus an entity directly, bypassing the pointer. Applies the same
/// `Claim`/`RestoreLast` rules as a click would, so a programmatic focus of a
/// container behaves like clicking it.
pub fn request_focus(world: &mut World, entity: Entity) -> bool {
    focus_from_pick(world, Some(entity))
}

/// Clear focus entirely.
pub fn clear_focus(world: &mut World) -> bool {
    commit_path(world, Vec::new())
}

/// Testable core: re-derive the focus path against the current tree.
///
/// This is what keeps focus synchronised with a UI that changes underneath it.
/// The focused entity can be despawned, can lose its [`FocusPolicy`], or can be
/// rebuilt from scratch by the reconciler on a widget-type change; a new
/// `Claim` ancestor can appear. Recovery walks the stored path from the leaf
/// end and restarts resolution from the deepest survivor.
pub fn validate_focus(world: &mut World, root: Entity) {
    let path = world.resource::<Focus>().path.clone();
    if path.is_empty() {
        return;
    }

    let survivor = path.iter().rev().copied().find(|&e| {
        world.get_entity(e).is_ok()
            && policy(world, e).is_some()
            && ancestors(world, e).any(|a| a == root)
    });

    let Some(survivor) = survivor else {
        clear_focus(world);
        return;
    };

    let new_path = resolve_focus_path(world, Some(survivor));
    commit_path(world, new_path);
}

/// Exclusive system wrapper for [`validate_focus`], registered in
/// `MatchaSet::PreExtract`.
pub fn run_validate_focus(world: &mut World) {
    let Some(root) = world
        .get_resource::<crate::resources::RenderWindowRoot>()
        .map(|r| r.entity)
    else {
        return;
    };
    validate_focus(world, root);
}

/// Exclusive system: bring the derived [`Focused`]/[`FocusWithin`] markers in
/// line with the [`Focus`] resource, and invalidate the cached render node of
/// every entity whose focus state changed.
///
/// Invalidation happens *here*, rather than in a separate `Changed<Focused>`
/// system alongside [`crate::systems::invalidate_on_opacity_change`], for a
/// specific reason: `Changed<T>` does not fire when a component is **removed**,
/// so an entity *losing* focus would never rebuild and would keep painting its
/// focus ring forever. This system already knows the exact set of entities that
/// transitioned in either direction, so it does the invalidation itself.
///
/// Widgets reading focus should therefore prefer `Has<Focused>` or the
/// [`Focus`] resource over `Changed<Focused>`, for the same reason.
pub fn sync_focus_components(world: &mut World) {
    let (top, path) = {
        let focus = world.resource::<Focus>();
        (focus.top, focus.path.clone())
    };

    let mut focused_query = world.query_filtered::<Entity, With<Focused>>();
    let stale_focused: Vec<Entity> = focused_query
        .iter(world)
        .filter(|e| Some(*e) != top)
        .collect();

    let mut within_query = world.query_filtered::<Entity, With<FocusWithin>>();
    let stale_within: Vec<Entity> = within_query
        .iter(world)
        .filter(|e| !path.contains(e))
        .collect();

    for entity in stale_focused {
        if let Ok(mut e) = world.get_entity_mut(entity) {
            e.remove::<Focused>();
            notify_focus_dispatch(&mut e, false);
        }
        invalidate_render_item(world, entity);
    }
    for entity in stale_within {
        if let Ok(mut e) = world.get_entity_mut(entity) {
            e.remove::<FocusWithin>();
        }
        invalidate_render_item(world, entity);
    }

    for &entity in &path {
        let Ok(mut e) = world.get_entity_mut(entity) else {
            continue;
        };
        if !e.contains::<FocusWithin>() {
            e.insert(FocusWithin);
            invalidate_render_item(world, entity);
        }
    }
    if let Some(top) = top {
        if let Ok(mut e) = world.get_entity_mut(top) {
            if !e.contains::<Focused>() {
                e.insert(Focused);
                notify_focus_dispatch(&mut e, true);
                invalidate_render_item(world, top);
            }
        }
    }
}

/// Tell an entity it just gained or lost the focus vertex, if it asked to know.
///
/// Only vertex transitions are reported, not `:focus-within` ones: this exists
/// for widgets that own an input session (starting or ending an IME
/// composition, say), which is a property of *being* focused, not of containing
/// something focused.
fn notify_focus_dispatch(entity: &mut bevy_ecs::world::EntityWorldMut, gained: bool) {
    if let Some(dispatch) = entity.get::<FocusDispatch>().copied() {
        dispatch.call(entity, gained);
    }
}

/// Drop `entity`'s cached render node, if it has one. Colours are baked into
/// the atlas at build time, so any focus-dependent painting needs a rebuild.
fn invalidate_render_item(world: &mut World, entity: Entity) {
    if let Some(mut item) = world.get_mut::<crate::components::render::RenderItem>(entity) {
        item.invalidate();
    }
}
