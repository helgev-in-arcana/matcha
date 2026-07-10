//! Hit-test cache and click resolution (`ECS_ARCHITECTURE.md` §11).
//!
//! Rebuilt unconditionally every frame in `MatchaSet::Flush` — v0.1 keeps this
//! simple, no incremental/dirty tracking (`ECS_IMPLEMENTATION_PLAN.md` §7-6).
//! Assumes `GlobalTransform` is translation-only, matching every current
//! `Layout` impl (rotation/scale are not accounted for in the hit rect).

use bevy_ecs::{entity::Entity, resource::Resource, world::World};
use nalgebra::Point3;

use crate::{
    components::{
        input::{HitTestEnabled, Message, OnClick, ZOrder},
        layout::{GlobalTransform, LayoutOutput},
        view::ViewChildren,
    },
    resources::RenderWindowRoot,
};

/// One hit-testable entity's window-space bounds, z-order, and paint position.
struct HitTestEntry {
    entity: Entity,
    /// `[min_x, min_y, max_x, max_y]` in window space.
    rect: [f32; 4],
    z: i32,
    /// DFS/paint visitation order among hit-testable entities (higher = drawn
    /// later = visually on top of an equal-`z` sibling).
    paint_index: u32,
}

/// Per-frame flattened hit-test list, rebuilt by [`update_hit_test_cache`].
#[derive(Resource, Default)]
pub struct HitTestCache {
    entries: Vec<HitTestEntry>,
}

impl HitTestCache {
    /// Entities whose rect contains `pos` (window space), frontmost first:
    /// sorted by [`ZOrder`] descending, then paint order descending.
    pub fn topmost_at(&self, pos: [f32; 2]) -> impl Iterator<Item = Entity> + '_ {
        let mut hits: Vec<&HitTestEntry> = self
            .entries
            .iter()
            .filter(|e| {
                pos[0] >= e.rect[0]
                    && pos[0] < e.rect[2]
                    && pos[1] >= e.rect[1]
                    && pos[1] < e.rect[3]
            })
            .collect();
        hits.sort_by(|a, b| b.z.cmp(&a.z).then(b.paint_index.cmp(&a.paint_index)));
        hits.into_iter().map(|e| e.entity)
    }
}

fn collect(world: &World, entity: Entity, entries: &mut Vec<HitTestEntry>, next_index: &mut u32) {
    if world.get::<HitTestEnabled>(entity).is_some() {
        if let (Some(layout), Some(transform)) = (
            world.get::<LayoutOutput>(entity),
            world.get::<GlobalTransform>(entity),
        ) {
            let origin = transform.affine.transform_point(&Point3::origin());
            let z = world.get::<ZOrder>(entity).map(|z| z.0).unwrap_or(0);
            entries.push(HitTestEntry {
                entity,
                rect: [
                    origin.x,
                    origin.y,
                    origin.x + layout.size[0],
                    origin.y + layout.size[1],
                ],
                z,
                paint_index: *next_index,
            });
            *next_index += 1;
        }
    }

    if let Some(children) = world.get::<ViewChildren>(entity) {
        for &(_, child) in &children.slots {
            collect(world, child, entries, next_index);
        }
    }
}

/// Testable core: walk `root`'s view tree in paint order and collect every
/// `HitTestEnabled` entity that has been laid out. No window/GPU needed.
pub fn build_hit_test_cache(world: &World, root: Entity) -> HitTestCache {
    let mut entries = Vec::new();
    let mut next_index = 0;
    collect(world, root, &mut entries, &mut next_index);
    HitTestCache { entries }
}

/// Exclusive system: rebuild the `HitTestCache` resource for the window root.
/// Registered in `MatchaSet::Flush`, after layout has run.
pub fn update_hit_test_cache(world: &mut World) {
    let Some(root) = world.get_resource::<RenderWindowRoot>().map(|r| r.entity) else {
        return;
    };
    let cache = build_hit_test_cache(world, root);
    world.insert_resource(cache);
}

/// Resolve `pos` to the frontmost entity (by [`HitTestCache::topmost_at`]
/// order) that carries `OnClick<Msg>`. Hit-testable entities without the
/// component are transparent to the scan.
pub fn resolve_click_target<Msg: Message>(
    world: &World,
    cache: &HitTestCache,
    pos: [f32; 2],
) -> Option<Entity> {
    cache
        .topmost_at(pos)
        .find(|&e| world.get::<OnClick<Msg>>(e).is_some())
}
