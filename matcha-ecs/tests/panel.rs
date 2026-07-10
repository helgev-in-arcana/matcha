//! Headless verification of `Panel` (Tier-1 HTML/CSS widgets batch): fixed
//! own size (unlike `Padding`, which auto-sizes to its child), single-child
//! inset-by-`border_width` arrangement, and `RenderItem`-cache-invalidation-
//! on-patch. Same GPU-free style as `tests/layout.rs`/`tests/render_item_reuse.rs`.

use std::sync::Arc;

use bevy_ecs::{entity::Entity, world::World};

use matcha_ecs::{
    components::{layout::LayoutOutput, render::RenderItem, view::ViewChildren},
    layout::{layout_root, Constraints},
    view::run_view,
};
use matcha_ecs_widgets::{ColorRect, Panel};

fn setup() -> (World, Entity) {
    let mut world = World::new();
    let root = world.spawn(ViewChildren::default()).id();
    (world, root)
}

fn children(world: &World, e: Entity) -> Vec<Entity> {
    world
        .get::<ViewChildren>(e)
        .map(|vc| vc.slots.iter().map(|(_, c)| *c).collect())
        .unwrap_or_default()
}

fn output(world: &World, e: Entity) -> LayoutOutput {
    *world
        .get::<LayoutOutput>(e)
        .unwrap_or_else(|| panic!("entity {e:?} has no LayoutOutput after layout_root"))
}

#[test]
fn own_size_is_fixed_regardless_of_a_much_smaller_child() {
    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        s.node(Panel::new(200.0, 100.0), |s| {
            s.leaf(ColorRect::new(10.0, 10.0));
        });
    });
    layout_root(&mut world, root, Constraints::from_max_size([800.0, 600.0]));

    let panel = children(&world, root)[0];
    assert_eq!(output(&world, panel).size, [200.0, 100.0]);
}

#[test]
fn child_is_inset_by_border_width_on_all_sides() {
    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        s.node(Panel::new(200.0, 100.0).border_width(5.0), |s| {
            s.leaf(ColorRect::new(50.0, 50.0));
        });
    });
    layout_root(&mut world, root, Constraints::from_max_size([800.0, 600.0]));

    let panel = children(&world, root)[0];
    let child = children(&world, panel)[0];
    assert_eq!(output(&world, child).origin, [5.0, 5.0]);
    assert_eq!(output(&world, child).size, [50.0, 50.0]);
}

#[test]
fn unchanged_props_do_not_invalidate_cache() {
    let (mut world, root) = setup();
    let build = |s: &mut matcha_ecs::view::Scope| {
        s.leaf(Panel::new(100.0, 50.0).background_color([0.1, 0.1, 0.1, 1.0]).border_width(2.0));
    };
    run_view(&mut world, root, build);
    let child = children(&world, root)[0];
    let before = world.get::<RenderItem>(child).unwrap().cache.clone();

    run_view(&mut world, root, build);
    let after = world.get::<RenderItem>(child).unwrap().cache.clone();

    assert!(Arc::ptr_eq(&before, &after), "cache Arc must be unchanged when no draw-relevant prop changed");
}

#[test]
fn changed_background_color_invalidates_cache() {
    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        s.leaf(Panel::new(100.0, 50.0).background_color([0.1, 0.1, 0.1, 1.0]));
    });
    let child = children(&world, root)[0];
    let before = world.get::<RenderItem>(child).unwrap().cache.clone();

    run_view(&mut world, root, |s| {
        s.leaf(Panel::new(100.0, 50.0).background_color([0.9, 0.1, 0.1, 1.0]));
    });
    let after = world.get::<RenderItem>(child).unwrap().cache.clone();

    assert!(!Arc::ptr_eq(&before, &after), "cache Arc must change when background_color changed");
}

#[test]
fn changed_border_width_invalidates_cache_and_layout() {
    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        s.leaf(Panel::new(100.0, 50.0).border_width(0.0));
    });
    let child = children(&world, root)[0];
    let before = world.get::<RenderItem>(child).unwrap().cache.clone();

    run_view(&mut world, root, |s| {
        s.leaf(Panel::new(100.0, 50.0).border_width(10.0));
    });
    let after = world.get::<RenderItem>(child).unwrap().cache.clone();

    assert!(!Arc::ptr_eq(&before, &after), "cache Arc must change when border_width changed");
}
