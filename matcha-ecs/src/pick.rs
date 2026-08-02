//! Picking: "what is under the pointer?", abstracted behind a swappable
//! backend.
//!
//! # Why this is a trait
//!
//! The original implementation was a flat array of rectangles sorted by a
//! picking-only z order — fine
//! for a 2D UI, but this framework does not intend to give up 3D rendering.
//! Two other backends are plausible later: a BVH/AABB raycast, and a GPU ID
//! buffer. The decisive constraint is that **an ID buffer cannot return an
//! ordered list of candidates** — it can only report the frontmost fragment at
//! a pixel. So the contract here is deliberately narrow:
//!
//! > picking returns **at most one entity**; there is no "next candidate".
//!
//! Everything downstream (click routing, focus) therefore resolves by walking
//! **up** the tree from that one entity, never by falling through to whatever
//! is behind it. See [`crate::input::bubble_to_click_target`].
//!
//! Mirrors [`crate::render::RenderDriver`]'s shape: a `Box<dyn>` backend held
//! as a resource, swappable at construction via `UiEcs::with_picker`.

use bevy_ecs::{entity::Entity, resource::Resource, world::World};
use nalgebra::Point3;

use crate::{
    clip::intersect,
    components::{
        input::Pickable,
        layout::{Clip, GlobalTransform, LayoutOutput},
    },
    traversal,
};

/// One picking request. `viewport_pos` is the only input the OS actually gives
/// us; a 3D backend derives its own ray from it.
///
/// There is deliberately no window field: a picker is scoped to the root it
/// was last [`update`](Picker::update)d with, so the window is already implied.
/// When multi-window support lands, the natural shape is one picker per window
/// root — the same way `RenderWindowRoot` will stop being a singleton. This is
/// a struct rather than a bare `[f32; 2]` so depth/ray/window fields can be
/// added later without breaking callers.
#[derive(Debug, Clone, Copy)]
pub struct PickQuery {
    pub viewport_pos: [f32; 2],
}

/// What picking found. A struct (rather than a bare `Entity`) so a 3D backend
/// can add depth / local hit position later without breaking callers.
#[derive(Debug, Clone, Copy)]
pub struct PickHit {
    pub entity: Entity,
}

/// A picking backend.
///
/// [`update`](Self::update) refreshes whatever acceleration structure the
/// backend keeps, once per frame in `MatchaSet::PreExtract` (after layout).
/// [`pick`](Self::pick) is called at event time and reads that structure, so it
/// always answers against the *previous* frame's state — the same timing model
/// every backend needs (a GPU ID buffer can only be read back after the frame
/// it was drawn in).
pub trait Picker: Send + Sync + 'static {
    /// Rebuild for the frame. `root` is the window-root entity whose view tree
    /// to consider.
    fn update(&mut self, world: &World, root: Entity);

    /// Resolve a query against the last [`update`](Self::update). `world` is
    /// available for backends that need it (a GPU backend reads its resources
    /// from there); [`RectPicker`] does not use it.
    fn pick(&self, world: &World, q: &PickQuery) -> Option<PickHit>;
}

/// The active picking backend.
#[derive(Resource)]
pub struct PickerResource(pub Box<dyn Picker>);

impl Default for PickerResource {
    fn default() -> Self {
        Self(Box::new(RectPicker::default()))
    }
}

/// Exclusive system: refresh the active picker for the window root.
/// Registered in `MatchaSet::PreExtract`, after layout has run.
pub fn update_picker(world: &mut World) {
    let Some(root) = world.get_resource::<crate::resources::RenderWindowRoot>().map(|r| r.entity)
    else {
        return;
    };
    world.resource_scope::<PickerResource, _>(|world, mut picker| {
        picker.0.update(world, root);
    });
}

/// One pickable entity's window-space bounds.
struct PickEntry {
    entity: Entity,
    /// `[min_x, min_y, max_x, max_y]` in window space.
    rect: [f32; 4],
}

/// The 2D backend: a flat array of axis-aligned rectangles in paint order, so
/// the **last** element is frontmost and a query is a backward scan.
///
/// It keeps no ordering of its own. Paint order comes from
/// [`crate::traversal::walk`] — the same walk `extract_items` uses — and under
/// the painter's algorithm reversing it *is* front-to-back. That is what makes
/// clicking and seeing structurally unable to disagree.
///
/// Assumes `GlobalTransform` is translation-only, matching every current
/// `Layout` impl (rotation/scale are not accounted for in the rect).
#[derive(Default)]
pub struct RectPicker {
    /// Back to front.
    entries: Vec<PickEntry>,
}

impl RectPicker {
    /// Testable core: build a picker for `root`'s view tree directly, with no
    /// window, GPU or schedule involved. Same core/wrapper split as
    /// `layout_root`/`run_layout`.
    pub fn build(world: &World, root: Entity) -> Self {
        let mut picker = Self::default();
        picker.update(world, root);
        picker
    }
}

impl Picker for RectPicker {
    fn update(&mut self, world: &World, root: Entity) {
        let mut entries = Vec::new();
        traversal::walk(world, root, None, &mut |world, entity, clip| {
            collect_one(world, entity, *clip, &mut entries)
        });
        self.entries = entries;
    }

    fn pick(&self, _world: &World, q: &PickQuery) -> Option<PickHit> {
        let [x, y] = q.viewport_pos;
        self.entries
            .iter()
            .rev()
            .find(|e| x >= e.rect[0] && x < e.rect[2] && y >= e.rect[1] && y < e.rect[3])
            .map(|e| PickHit { entity: e.entity })
    }
}

/// Record `entity` if it is pickable, and return the clip its children sit
/// inside — the intersection of every [`Clip`] enclosing them, in window
/// space, or `None` when nothing does. `None` as the *return* prunes the
/// subtree, which is what an entity clipped entirely away does.
fn collect_one(
    world: &World,
    entity: Entity,
    clip: Option<[f32; 4]>,
    entries: &mut Vec<PickEntry>,
) -> Option<Option<[f32; 4]>> {
    let box_of = |entity: Entity| {
        let layout = world.get::<LayoutOutput>(entity)?;
        let transform = world.get::<GlobalTransform>(entity)?;
        let origin = transform.affine.transform_point(&Point3::origin());
        Some([
            origin.x,
            origin.y,
            origin.x + layout.size[0],
            origin.y + layout.size[1],
        ])
    };

    // A `Clip` covers the declaring entity as well as its descendants, so it
    // narrows this entity's own rectangle before it is recorded.
    let clip = match (world.get::<Clip>(entity).is_some(), box_of(entity)) {
        (true, Some(own)) => match clip {
            Some(outer) => match intersect(outer, own) {
                Some(narrowed) => Some(narrowed),
                // Entirely clipped away: nothing here or below can be picked.
                None => return None,
            },
            None => Some(own),
        },
        _ => clip,
    };

    if world.get::<Pickable>(entity).is_some()
        && let Some(rect) = box_of(entity)
    {
        // A widget only partly visible stays pickable over the visible part.
        let visible = match clip {
            Some(clip) => intersect(rect, clip),
            None => Some(rect),
        };
        if let Some(rect) = visible {
            entries.push(PickEntry { entity, rect });
        }
    }

    Some(clip)
}
