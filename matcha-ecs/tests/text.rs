//! Headless verification of `Text` (M6): layout/wrap behaviour via
//! `layout_root` and `RenderItem` cache-invalidation-on-patch, in the same
//! style as `tests/layout.rs`/`tests/render_item_reuse.rs`. No GPU/window is
//! needed for any of these — like `tests/extract.rs`, `RenderItem::builder`
//! is never invoked, only its `cache` `Arc` identity is asserted. Actual
//! glyph rasterisation/atlas upload (`FontCtx::stencil_region`) needs a real
//! `wgpu::Device` and is therefore left to manual/demo verification, matching
//! the rest of this test suite's established GPU-free approach.

use std::sync::Arc;

use bevy_ecs::{entity::Entity, world::World};

use matcha_ecs::{
    animation::Easing,
    components::{layout::LayoutOutput, render::RenderItem, view::ViewChildren},
    layout::{layout_root, Constraints},
    view::run_view,
};
use matcha_ecs_widgets::Text;

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

fn output(world: &World, e: Entity) -> LayoutOutput {
    *world
        .get::<LayoutOutput>(e)
        .unwrap_or_else(|| panic!("entity {e:?} has no LayoutOutput after layout_root"))
}

const LONG_SENTENCE: &str =
    "the quick brown fox jumps over the lazy dog and keeps running past the hills";

#[test]
fn empty_content_measures_to_near_zero_size() {
    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        s.leaf(Text::new(""));
    });
    layout_root(&mut world, root, Constraints::from_max_size([800.0, 600.0]));

    let child = first_child(&world, root);
    let out = output(&world, child);
    assert_eq!(out.size, [0.0, 0.0]);
}

#[test]
fn narrow_constraint_wraps_to_a_taller_block_than_a_wide_one() {
    let build = |s: &mut matcha_ecs::view::Scope| {
        s.leaf(Text::new(LONG_SENTENCE).font_size(16.0));
    };

    let (mut world_wide, root_wide) = setup();
    run_view(&mut world_wide, root_wide, build);
    layout_root(&mut world_wide, root_wide, Constraints::from_max_size([2000.0, 2000.0]));
    let wide = output(&world_wide, first_child(&world_wide, root_wide));

    let (mut world_narrow, root_narrow) = setup();
    run_view(&mut world_narrow, root_narrow, build);
    layout_root(&mut world_narrow, root_narrow, Constraints::from_max_size([60.0, 2000.0]));
    let narrow = output(&world_narrow, first_child(&world_narrow, root_narrow));

    assert!(
        narrow.size[1] > wide.size[1],
        "wrapping at a narrow width must produce more lines (taller block): wide={:?} narrow={:?}",
        wide,
        narrow
    );
    assert!(
        narrow.size[0] <= 60.0 + 0.01,
        "wrapped width must respect the narrow constraint: {:?}",
        narrow
    );
}

#[test]
fn unchanged_props_do_not_invalidate_cache() {
    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        s.leaf(Text::new("hello").font_size(16.0).color([0.0, 0.0, 0.0, 1.0]));
    });
    let child = first_child(&world, root);
    let cache_before = world
        .get::<RenderItem>(child)
        .expect("Text carries a RenderItem")
        .cache
        .clone();

    run_view(&mut world, root, |s| {
        s.leaf(Text::new("hello").font_size(16.0).color([0.0, 0.0, 0.0, 1.0]));
    });
    let cache_after = world.get::<RenderItem>(child).unwrap().cache.clone();

    assert!(
        Arc::ptr_eq(&cache_before, &cache_after),
        "cache Arc must be unchanged when no draw-relevant prop changed"
    );
}

#[test]
fn changed_content_invalidates_cache() {
    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        s.leaf(Text::new("hello"));
    });
    let child = first_child(&world, root);
    let cache_before = world.get::<RenderItem>(child).unwrap().cache.clone();

    run_view(&mut world, root, |s| {
        s.leaf(Text::new("goodbye"));
    });
    let cache_after = world.get::<RenderItem>(child).unwrap().cache.clone();

    assert!(
        !Arc::ptr_eq(&cache_before, &cache_after),
        "cache Arc must change when content changed"
    );
}

#[test]
fn changed_font_size_invalidates_cache() {
    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        s.leaf(Text::new("hello").font_size(16.0));
    });
    let child = first_child(&world, root);
    let cache_before = world.get::<RenderItem>(child).unwrap().cache.clone();

    run_view(&mut world, root, |s| {
        s.leaf(Text::new("hello").font_size(24.0));
    });
    let cache_after = world.get::<RenderItem>(child).unwrap().cache.clone();

    assert!(
        !Arc::ptr_eq(&cache_before, &cache_after),
        "cache Arc must change when font_size changed"
    );
}

#[test]
fn changed_color_invalidates_cache() {
    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        s.leaf(Text::new("hello").color([1.0, 0.0, 0.0, 1.0]));
    });
    let child = first_child(&world, root);
    let cache_before = world.get::<RenderItem>(child).unwrap().cache.clone();

    run_view(&mut world, root, |s| {
        s.leaf(Text::new("hello").color([0.0, 1.0, 0.0, 1.0]));
    });
    let cache_after = world.get::<RenderItem>(child).unwrap().cache.clone();

    assert!(
        !Arc::ptr_eq(&cache_before, &cache_after),
        "cache Arc must change when color changed"
    );
}

#[test]
fn enter_fade_builder_starts_from_transparent() {
    use matcha_ecs::animation::{Animated, Opacity};

    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        s.leaf(Text::new("hello").enter_fade(std::time::Duration::from_millis(200), Easing::Linear));
    });
    let child = first_child(&world, root);
    let animated = world.get::<Animated<Opacity>>(child).expect("Animated<Opacity> present");
    assert_eq!(animated.0, Opacity(0.0));
}
