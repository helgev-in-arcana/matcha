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
    clip::ClipArena,
    components::{
        input::Pickable,
        layout::{GlobalTransform, LayoutOutput},
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
    let Some(root) = crate::resources::ui_root(world) else {
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
    /// Innermost enclosing clip, as an index into the picker's own
    /// [`ClipArena`]. The clips it inherits are that one's ancestors.
    clip: Option<u32>,
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
    /// The clips those entries sit inside, in the same arena shape the renderer
    /// is handed. Picking used to carry a running rectangle intersection
    /// instead — cheaper, but it is a *different model* of the same thing, and
    /// one that cannot express a rotated clip, so the day a transform stops
    /// being translation-only what you can click and what you can see would
    /// disagree. Sharing the model is what makes that impossible.
    clips: ClipArena,
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
        let mut out = RectPicker::default();
        traversal::walk(world, root, None, &mut |world, entity, clip| {
            Some(collect_one(world, entity, *clip, &mut out))
        });
        *self = out;
    }

    fn pick(&self, _world: &World, q: &PickQuery) -> Option<PickHit> {
        let [x, y] = q.viewport_pos;
        self.entries
            .iter()
            .rev()
            // Own box first: it rejects almost everything, and costs no matrix
            // inversion. A widget only partly visible stays pickable over the
            // visible part, because the clip test runs on the point.
            .find(|e| {
                x >= e.rect[0]
                    && x < e.rect[2]
                    && y >= e.rect[1]
                    && y < e.rect[3]
                    && self.clips.contains(e.clip, q.viewport_pos)
            })
            .map(|e| PickHit { entity: e.entity })
    }
}

/// Record `entity` if it is pickable, and return the innermost clip its
/// children sit inside.
fn collect_one(
    world: &World,
    entity: Entity,
    clip: Option<u32>,
    out: &mut RectPicker,
) -> Option<u32> {
    // A `Clip` covers the declaring entity as well as its descendants, so the
    // index this returns is also the one `entity` is recorded with.
    let clip = crate::clip::descend(&mut out.clips, world, entity, clip);

    if world.get::<Pickable>(entity).is_some()
        && let (Some(layout), Some(transform)) = (
            world.get::<LayoutOutput>(entity),
            world.get::<GlobalTransform>(entity),
        )
    {
        let origin = transform.affine.transform_point(&Point3::origin());
        out.entries.push(PickEntry {
            entity,
            rect: [
                origin.x,
                origin.y,
                origin.x + layout.size[0],
                origin.y + layout.size[1],
            ],
            clip,
        });
    }

    clip
}
