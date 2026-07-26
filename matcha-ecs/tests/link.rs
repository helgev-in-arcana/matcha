//! Headless verification of `Link` (Tier-1 HTML/CSS widgets batch): it
//! delegates `Widget::bundle`/`patch`/`after_spawn` to a wrapped `RichText`
//! while also carrying `OnClick`/`Pickable` — confirm both halves
//! (click dispatch membership and text-cache invalidation) actually work
//! through the delegation, not just compile. Same GPU-free style as
//! `tests/render_item_reuse.rs`; `Link`'s `RenderItem` is built in
//! `after_spawn` (inherited from `RichText`), which `run_view` already runs
//! on first spawn.

use std::sync::Arc;

use bevy_ecs::{entity::Entity, world::World};

use matcha_ecs::components::{
    input::{OnClick, Pickable},
    render::RenderItem,
    view::ViewChildren,
};
use matcha_ecs_widgets::Link;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Msg {
    Navigate,
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
        .expect("Link carries a RenderItem (delegated from RichText)")
        .cache
        .clone()
}

#[test]
fn carries_hit_test_membership_and_the_assigned_message() {
    let (mut world, root) = setup();
    matcha_ecs::view::run_view(&mut world, root, |s| {
        s.leaf(Link::<Msg>::new("click me").on(Msg::Navigate));
    });
    let child = first_child(&world, root);

    assert!(world.get::<Pickable>(child).is_some());
    assert_eq!(world.get::<OnClick<Msg>>(child).copied(), Some(OnClick(Some(Msg::Navigate))));
}

#[test]
fn unchanged_props_do_not_invalidate_cache() {
    let (mut world, root) = setup();
    let build = |s: &mut matcha_ecs::view::Scope| {
        s.leaf(Link::<Msg>::new("click me").on(Msg::Navigate));
    };
    matcha_ecs::view::run_view(&mut world, root, build);
    let child = first_child(&world, root);
    let before = cache(&world, child);

    matcha_ecs::view::run_view(&mut world, root, build);
    let after = cache(&world, child);

    assert!(Arc::ptr_eq(&before, &after), "cache Arc must be unchanged when no draw-relevant prop changed");
}

#[test]
fn changed_content_invalidates_cache_via_delegated_patch() {
    let (mut world, root) = setup();
    matcha_ecs::view::run_view(&mut world, root, |s| {
        s.leaf(Link::<Msg>::new("click me").on(Msg::Navigate));
    });
    let child = first_child(&world, root);
    let before = cache(&world, child);

    matcha_ecs::view::run_view(&mut world, root, |s| {
        s.leaf(Link::<Msg>::new("click here instead").on(Msg::Navigate));
    });
    let after = cache(&world, child);

    assert!(
        !Arc::ptr_eq(&before, &after),
        "cache Arc must change when content changed, delegated through RichText::patch"
    );
}

#[test]
fn changed_message_updates_on_click_without_requiring_a_content_change() {
    let (mut world, root) = setup();
    matcha_ecs::view::run_view(&mut world, root, |s| {
        s.leaf(Link::<Msg>::new("click me").on(Msg::Navigate));
    });
    let child = first_child(&world, root);

    matcha_ecs::view::run_view(&mut world, root, |s| {
        s.leaf(Link::<Msg>::new("click me"));
    });

    assert_eq!(
        world.get::<OnClick<Msg>>(child).copied(),
        Some(OnClick(None)),
        "OnClick must be re-patched to None even though the text content didn't change"
    );
}
