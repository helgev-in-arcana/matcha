//! Headless verification of the `RenderItem` invalidation contract (M2-3):
//! re-running the view with unchanged draw-relevant props must leave the
//! cached render node untouched, and changing a prop must invalidate it.
//! No GPU/window is needed — `RenderItem::cache` is asserted by `Arc` identity,
//! never dereferenced through `builder`.

use std::sync::Arc;

use bevy_ecs::{entity::Entity, world::World};

use matcha_ecs::{
    components::{render::RenderItem, view::ViewChildren},
    view::run_view,
};
use matcha_ecs_widgets::ColorRect;

fn setup() -> (World, Entity) {
    let mut world = World::new();
    let root = world.spawn(ViewChildren::default()).id();
    (world, root)
}

fn first_child(world: &World, root: Entity) -> Entity {
    world
        .get::<ViewChildren>(root)
        .expect("root has ViewChildren")
        .slots[0]
        .1
}

#[test]
fn unchanged_props_do_not_invalidate_cache() {
    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        s.leaf(ColorRect::new(100.0, 50.0).color([1.0, 0.0, 0.0, 1.0]));
    });
    let child = first_child(&world, root);
    let cache_before = world
        .get::<RenderItem>(child)
        .expect("ColorRect carries a RenderItem")
        .cache
        .clone();

    // Same slot, same widget type, identical props -> patch() with no change.
    run_view(&mut world, root, |s| {
        s.leaf(ColorRect::new(100.0, 50.0).color([1.0, 0.0, 0.0, 1.0]));
    });
    let cache_after = world.get::<RenderItem>(child).unwrap().cache.clone();

    assert!(
        Arc::ptr_eq(&cache_before, &cache_after),
        "cache Arc must be unchanged when no draw-relevant prop changed"
    );
}

#[test]
fn changed_color_invalidates_cache() {
    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        s.leaf(ColorRect::new(100.0, 50.0).color([1.0, 0.0, 0.0, 1.0]));
    });
    let child = first_child(&world, root);
    let cache_before = world.get::<RenderItem>(child).unwrap().cache.clone();

    run_view(&mut world, root, |s| {
        s.leaf(ColorRect::new(100.0, 50.0).color([0.0, 1.0, 0.0, 1.0]));
    });
    let cache_after = world.get::<RenderItem>(child).unwrap().cache.clone();

    assert!(
        !Arc::ptr_eq(&cache_before, &cache_after),
        "cache Arc must change when a draw-relevant prop (color) changed"
    );
}

#[test]
fn changed_size_invalidates_cache() {
    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        s.leaf(ColorRect::new(100.0, 50.0).color([1.0, 0.0, 0.0, 1.0]));
    });
    let child = first_child(&world, root);
    let cache_before = world.get::<RenderItem>(child).unwrap().cache.clone();

    run_view(&mut world, root, |s| {
        s.leaf(ColorRect::new(120.0, 50.0).color([1.0, 0.0, 0.0, 1.0]));
    });
    let cache_after = world.get::<RenderItem>(child).unwrap().cache.clone();

    assert!(
        !Arc::ptr_eq(&cache_before, &cache_after),
        "cache Arc must change when a draw-relevant prop (size) changed"
    );
}
