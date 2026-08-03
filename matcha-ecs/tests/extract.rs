//! Headless verification of the M4 render extract step: build a view, run
//! layout, then call `extract_items` directly (no window/GPU) and assert paint
//! order, per-item transforms, and that the extracted cache `Arc` is shared with
//! the source entity's `RenderItem`. Same style as `tests/layout.rs`.

use std::sync::Arc;

use bevy_ecs::{entity::Entity, world::World};

use matcha_ecs::{
    components::{render::RenderItem, view::ViewChildren},
    layout::{layout_root, Constraints},
    render::extract_items,
    view::run_view,
};
use matcha_ecs_widgets::{ColorRect, Column, Row};

fn children(world: &World, e: Entity) -> Vec<Entity> {
    world
        .get::<ViewChildren>(e)
        .map(|vc| vc.slots.iter().map(|(_, c)| *c).collect())
        .unwrap_or_default()
}

/// `Column > (ColorRect A, Row > (ColorRect B, ColorRect C))`.
/// Only the three `ColorRect` leaves carry a `RenderItem`, so extract yields
/// exactly `[A, B, C]` in depth-first paint order.
#[test]
fn extract_collects_leaves_in_paint_order_with_window_space_transforms() {
    let mut world = World::new();
    let root = world.spawn(ViewChildren::default()).id();
    run_view(&mut world, root, |s| {
        s.node(Column::new().gap(20.0), |s| {
            s.leaf(ColorRect::new(300.0, 100.0));
            s.node(Row::new().gap(20.0), |s| {
                s.leaf(ColorRect::new(100.0, 100.0));
                s.leaf(ColorRect::new(100.0, 100.0));
            });
        });
    });
    layout_root(&mut world, root, Constraints::from_max_size([800.0, 600.0]));

    let column = children(&world, root)[0];
    let [rect_a, row]: [Entity; 2] = children(&world, column).try_into().unwrap();
    let [rect_b, rect_c]: [Entity; 2] = children(&world, row).try_into().unwrap();

    let items = extract_items(&world, root).items;

    // (a) paint order: containers contribute nothing, three leaves in DFS order.
    assert_eq!(items.len(), 3, "only the three ColorRect leaves are drawable");

    // (b) transforms are window-space (composed through the ancestor chain).
    let translation = |i: usize| {
        let t = items[i].transform.column(3);
        (t.x, t.y)
    };
    assert_eq!(translation(0), (0.0, 0.0)); // A: top of column
    assert_eq!(translation(1), (0.0, 120.0)); // B: row origin (0,120) + local (0,0)
    assert_eq!(translation(2), (120.0, 120.0)); // C: row origin + local (120,0)

    // (c) the extracted cache Arc is the *same* Arc as the source entity's, so an
    // invalidate that swaps the entity's cache is observed by later frames and the
    // node built on the render thread is shared, not deep-copied.
    let entity_cache = |e: Entity| world.get::<RenderItem>(e).unwrap().cache.clone();
    assert!(Arc::ptr_eq(&items[0].cache, &entity_cache(rect_a)));
    assert!(Arc::ptr_eq(&items[1].cache, &entity_cache(rect_b)));
    assert!(Arc::ptr_eq(&items[2].cache, &entity_cache(rect_c)));
}
