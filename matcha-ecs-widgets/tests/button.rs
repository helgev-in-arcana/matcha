//! Headless verification of `Button` (Tier-1 HTML/CSS widgets batch): the
//! `RenderItem`-cache-invalidation-on-patch contract, in the same style as
//! `tests/render_item_reuse.rs`/`tests/text.rs`. No GPU/window is needed —
//! `RenderItem::builder` is never invoked, only its `cache` `Arc` identity is
//! asserted. Actual label shaping/rasterisation needs a real `wgpu::Device`
//! and is left to manual/demo verification, matching this suite's
//! established GPU-free approach.
//!
//! `Button`'s `RenderItem` is built in `after_spawn` (it needs the `FontCtx`
//! resource for the label), not `bundle()` like `ColorRect` — `run_view`
//! already runs `after_spawn` right after `bundle()` on first spawn (`Text`'s
//! tests rely on the same thing), so this needs no special setup here.

use std::sync::Arc;

use bevy_ecs::{entity::Entity, world::World};

use matcha_ecs::components::{render::RenderItem, view::ViewChildren};
use matcha_ecs_widgets::{color_rect::RectColor, Button};

#[derive(Clone, Copy, PartialEq, Debug)]
enum Msg {
    Clicked,
}

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

fn cache(world: &World, e: Entity) -> Arc<parking_lot::Mutex<Option<Arc<renderer::RenderNode>>>> {
    world
        .get::<RenderItem>(e)
        .expect("Button carries a RenderItem")
        .cache
        .clone()
}

#[test]
fn unchanged_props_do_not_invalidate_cache() {
    let (mut world, root) = setup();
    let build = |s: &mut matcha_ecs::view::Scope| {
        s.leaf(Button::<Msg>::new("ok").on(Msg::Clicked).color([0.3, 0.3, 0.4, 1.0]));
    };
    matcha_ecs::view::run_view(&mut world, root, build);
    let child = first_child(&world, root);
    let before = cache(&world, child);

    matcha_ecs::view::run_view(&mut world, root, build);
    let after = cache(&world, child);

    assert!(
        Arc::ptr_eq(&before, &after),
        "cache Arc must be unchanged when no draw-relevant prop changed"
    );
}

#[test]
fn changed_label_invalidates_cache() {
    let (mut world, root) = setup();
    matcha_ecs::view::run_view(&mut world, root, |s| {
        s.leaf(Button::<Msg>::new("ok").on(Msg::Clicked));
    });
    let child = first_child(&world, root);
    let before = cache(&world, child);

    matcha_ecs::view::run_view(&mut world, root, |s| {
        s.leaf(Button::<Msg>::new("cancel").on(Msg::Clicked));
    });
    let after = cache(&world, child);

    assert!(!Arc::ptr_eq(&before, &after), "cache Arc must change when the label changed");
}

#[test]
fn changed_color_only_invalidates_cache() {
    // Regression test: `Button` previously did not carry `RectColor`, so a
    // colour-only `.color()` change went undetected by `patch` and never
    // rebuilt the cached render item.
    let (mut world, root) = setup();
    matcha_ecs::view::run_view(&mut world, root, |s| {
        s.leaf(Button::<Msg>::new("ok").on(Msg::Clicked).color([0.3, 0.3, 0.4, 1.0]));
    });
    let child = first_child(&world, root);
    let before = cache(&world, child);

    matcha_ecs::view::run_view(&mut world, root, |s| {
        s.leaf(Button::<Msg>::new("ok").on(Msg::Clicked).color([0.9, 0.1, 0.1, 1.0]));
    });
    let after = cache(&world, child);

    assert!(
        !Arc::ptr_eq(&before, &after),
        "cache Arc must change when colour changed, even with the label/geometry unchanged"
    );
    assert_eq!(
        world.get::<RectColor>(child).copied(),
        Some(RectColor([0.9, 0.1, 0.1, 1.0])),
        "RectColor component must reflect the new colour"
    );
}

#[test]
fn changed_font_size_and_label_color_invalidate_cache() {
    let (mut world, root) = setup();
    matcha_ecs::view::run_view(&mut world, root, |s| {
        s.leaf(Button::<Msg>::new("ok").on(Msg::Clicked));
    });
    let child = first_child(&world, root);
    let before = cache(&world, child);

    matcha_ecs::view::run_view(&mut world, root, |s| {
        s.leaf(Button::<Msg>::new("ok").on(Msg::Clicked).font_size(20.0).label_color([1.0, 0.0, 0.0, 1.0]));
    });
    let after = cache(&world, child);

    assert!(
        !Arc::ptr_eq(&before, &after),
        "cache Arc must change when font_size/label_color changed"
    );
}
