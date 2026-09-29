//! Headless verification of Button properties and RenderItem revisions.
//! Writers are not invoked; these tests check whether patching draw-relevant
//! properties advances the revision without requiring a GPU or window.
//!
//! `Button`'s `RenderItem` is built in `after_spawn` (it needs the `FontCtx`
//! resource for the label). `run_view` invokes that hook after `bundle()` when
//! spawning the entity.

use bevy_ecs::{entity::Entity, world::World};

use matcha_ecs::components::{render::RenderItem, view::ViewChildren};
use matcha_ecs_widgets::{Button, color_rect::RectColor};

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

fn cache(world: &World, e: Entity) -> u64 {
    world
        .get::<RenderItem>(e)
        .expect("Button carries a RenderItem")
        .revision
}

#[test]
fn unchanged_props_do_not_invalidate_cache() {
    let (mut world, root) = setup();
    let build = |s: &mut matcha_ecs::view::Scope| {
        s.leaf(
            Button::<Msg>::new("ok")
                .on(Msg::Clicked)
                .color([0.3, 0.3, 0.4, 1.0]),
        );
    };
    matcha_ecs::view::run_view(&mut world, root, build);
    let child = first_child(&world, root);
    let before = cache(&world, child);

    matcha_ecs::view::run_view(&mut world, root, build);
    let after = cache(&world, child);

    assert!(
        (before == after),
        "draw revision must be unchanged when no draw-relevant prop changed"
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

    assert!(
        (before != after),
        "draw revision must change when the label changed"
    );
}

#[test]
fn changed_color_only_invalidates_cache() {
    // A colour-only change must update RectColor and advance the draw revision.
    let (mut world, root) = setup();
    matcha_ecs::view::run_view(&mut world, root, |s| {
        s.leaf(
            Button::<Msg>::new("ok")
                .on(Msg::Clicked)
                .color([0.3, 0.3, 0.4, 1.0]),
        );
    });
    let child = first_child(&world, root);
    let before = cache(&world, child);

    matcha_ecs::view::run_view(&mut world, root, |s| {
        s.leaf(
            Button::<Msg>::new("ok")
                .on(Msg::Clicked)
                .color([0.9, 0.1, 0.1, 1.0]),
        );
    });
    let after = cache(&world, child);

    assert!(
        (before != after),
        "draw revision must change when colour changed, even with the label/geometry unchanged"
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
        s.leaf(
            Button::<Msg>::new("ok")
                .on(Msg::Clicked)
                .font_size(20.0)
                .label_color([1.0, 0.0, 0.0, 1.0]),
        );
    });
    let after = cache(&world, child);

    assert!(
        (before != after),
        "draw revision must change when font_size/label_color changed"
    );
}
