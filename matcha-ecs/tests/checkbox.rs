//! Headless verification of `Checkbox` (Tier-1 HTML/CSS widgets batch):
//! declarative `checked` state and `RenderItem`-cache-invalidation-on-patch,
//! same GPU-free style as `tests/render_item_reuse.rs`.

use std::sync::Arc;

use bevy_ecs::{entity::Entity, world::World};

use matcha_ecs::components::{render::RenderItem, view::ViewChildren};
use matcha_ecs_widgets::{color_rect::RectGeometry, Checkbox};

#[derive(Clone, Copy, PartialEq, Debug)]
enum Msg {
    Toggle,
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
        .expect("Checkbox carries a RenderItem")
        .cache
        .clone()
}

#[test]
fn defaults_to_a_20px_square_leaf() {
    let (mut world, root) = setup();
    matcha_ecs::view::run_view(&mut world, root, |s| {
        s.leaf(Checkbox::<Msg>::new(false).on(Msg::Toggle));
    });
    let child = first_child(&world, root);
    assert_eq!(world.get::<RectGeometry>(child).copied(), Some(RectGeometry { w: 20.0, h: 20.0 }));
}

#[test]
fn unchanged_checked_state_does_not_invalidate_cache() {
    let (mut world, root) = setup();
    let build = |s: &mut matcha_ecs::view::Scope| {
        s.leaf(Checkbox::<Msg>::new(true).on(Msg::Toggle));
    };
    matcha_ecs::view::run_view(&mut world, root, build);
    let child = first_child(&world, root);
    let before = cache(&world, child);

    matcha_ecs::view::run_view(&mut world, root, build);
    let after = cache(&world, child);

    assert!(Arc::ptr_eq(&before, &after), "cache Arc must be unchanged when checked is re-declared identically");
}

#[test]
fn toggling_checked_invalidates_cache() {
    // The declarative pattern this widget is built around: the app passes
    // the current checked bool on every view() call, and a change must be
    // detected and rebuild the cached render item — same contract as
    // `Button`'s `.color()`.
    let (mut world, root) = setup();
    matcha_ecs::view::run_view(&mut world, root, |s| {
        s.leaf(Checkbox::<Msg>::new(false).on(Msg::Toggle));
    });
    let child = first_child(&world, root);
    let before = cache(&world, child);

    matcha_ecs::view::run_view(&mut world, root, |s| {
        s.leaf(Checkbox::<Msg>::new(true).on(Msg::Toggle));
    });
    let after = cache(&world, child);

    assert!(!Arc::ptr_eq(&before, &after), "cache Arc must change when checked toggled");
}

#[test]
fn changed_size_invalidates_cache_and_updates_geometry() {
    let (mut world, root) = setup();
    matcha_ecs::view::run_view(&mut world, root, |s| {
        s.leaf(Checkbox::<Msg>::new(false).on(Msg::Toggle));
    });
    let child = first_child(&world, root);
    let before = cache(&world, child);

    matcha_ecs::view::run_view(&mut world, root, |s| {
        s.leaf(Checkbox::<Msg>::new(false).on(Msg::Toggle).size(32.0));
    });
    let after = cache(&world, child);

    assert!(!Arc::ptr_eq(&before, &after), "cache Arc must change when size changed");
    assert_eq!(world.get::<RectGeometry>(child).copied(), Some(RectGeometry { w: 32.0, h: 32.0 }));
}
