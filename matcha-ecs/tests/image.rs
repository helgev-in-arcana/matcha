//! Headless verification of `Image` (Tier-1 HTML/CSS widgets batch):
//! mandatory-size geometry and `RenderItem`-cache-invalidation-on-patch,
//! keyed on *source identity* (path equality / `Arc` pointer identity), never
//! a deep byte compare — same GPU-free style as `tests/render_item_reuse.rs`.
//! `Image`'s `RenderItem` is built in `after_spawn` (it needs the `ImageCtx`
//! resource), which `run_view` already runs on first spawn; the builder
//! closure itself (decode/resize/upload) is never invoked here, so no real
//! image bytes or `wgpu::Device` are needed.

use std::sync::Arc;

use bevy_ecs::{entity::Entity, world::World};

use matcha_ecs::components::{render::RenderItem, view::ViewChildren};
use matcha_ecs_widgets::{color_rect::RectGeometry, Image};

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
        .expect("Image carries a RenderItem")
        .cache
        .clone()
}

#[test]
fn size_is_the_mandatory_constructor_box_not_a_natural_image_size() {
    let (mut world, root) = setup();
    let bytes: Arc<[u8]> = Arc::from(vec![0u8; 4]);
    matcha_ecs::view::run_view(&mut world, root, |s| {
        s.leaf(Image::from_bytes(bytes.clone(), 200.0, 150.0));
    });
    let child = first_child(&world, root);
    assert_eq!(world.get::<RectGeometry>(child).copied(), Some(RectGeometry { w: 200.0, h: 150.0 }));
}

#[test]
fn re_declaring_the_same_arc_bytes_does_not_invalidate_cache() {
    let (mut world, root) = setup();
    let bytes: Arc<[u8]> = Arc::from(vec![1u8; 4]);
    let build = |s: &mut matcha_ecs::view::Scope| {
        s.leaf(Image::from_bytes(bytes.clone(), 100.0, 100.0));
    };
    matcha_ecs::view::run_view(&mut world, root, build);
    let child = first_child(&world, root);
    let before = cache(&world, child);

    matcha_ecs::view::run_view(&mut world, root, build);
    let after = cache(&world, child);

    assert!(
        Arc::ptr_eq(&before, &after),
        "cache Arc must be unchanged when the same Arc<[u8]> (same pointer identity) is re-declared"
    );
}

#[test]
fn a_different_arc_with_identical_byte_content_still_invalidates_cache() {
    // Source identity is pointer-based, not a deep byte compare (documented
    // deliberate design) — a fresh Arc with the same bytes is treated as a
    // different source.
    let (mut world, root) = setup();
    matcha_ecs::view::run_view(&mut world, root, |s| {
        s.leaf(Image::from_bytes(Arc::from(vec![2u8; 4]), 100.0, 100.0));
    });
    let child = first_child(&world, root);
    let before = cache(&world, child);

    matcha_ecs::view::run_view(&mut world, root, |s| {
        s.leaf(Image::from_bytes(Arc::from(vec![2u8; 4]), 100.0, 100.0));
    });
    let after = cache(&world, child);

    assert!(
        !Arc::ptr_eq(&before, &after),
        "cache Arc must change for a different Arc allocation, even with identical byte content"
    );
}

#[test]
fn changed_path_invalidates_cache() {
    let (mut world, root) = setup();
    matcha_ecs::view::run_view(&mut world, root, |s| {
        s.leaf(Image::from_path("a.png", 100.0, 100.0));
    });
    let child = first_child(&world, root);
    let before = cache(&world, child);

    matcha_ecs::view::run_view(&mut world, root, |s| {
        s.leaf(Image::from_path("b.png", 100.0, 100.0));
    });
    let after = cache(&world, child);

    assert!(!Arc::ptr_eq(&before, &after), "cache Arc must change when the path changed");
}

#[test]
fn changed_size_invalidates_cache() {
    let (mut world, root) = setup();
    matcha_ecs::view::run_view(&mut world, root, |s| {
        s.leaf(Image::from_path("a.png", 100.0, 100.0));
    });
    let child = first_child(&world, root);
    let before = cache(&world, child);

    matcha_ecs::view::run_view(&mut world, root, |s| {
        s.leaf(Image::from_path("a.png", 200.0, 100.0));
    });
    let after = cache(&world, child);

    assert!(!Arc::ptr_eq(&before, &after), "cache Arc must change when the display size changed");
}
