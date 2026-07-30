//! Headless verification that the size layout allocates to an entity
//! (`LayoutOutput::size`) is what actually reaches the render stage
//! (`RenderItemSnapshot::size`, i.e. `RenderCtx::size` at build time).
//!
//! This is the layout→render contract the original Tier-1 demo bugs slipped
//! through: `matcha-ecs/tests/layout.rs` asserts on `LayoutOutput` only, so a
//! widget whose `RenderItem` baked in its *declared* size passed every layout
//! test while visibly drawing at the wrong size — `AlignItems::Stretch`
//! appearing to do nothing, and a `Panel` stretched by a default-`Stretch`
//! `Row` centring its child within an allocated box larger than the box it
//! painted (the child "overflowing" the panel). These tests reproduce both
//! demo compositions at the extract level, GPU-free (`RenderItem::builder` is
//! never invoked, per this suite's convention).

use bevy_ecs::{entity::Entity, world::World};
use matcha_ecs::{
    components::{layout::LayoutOutput, view::ViewChildren},
    layout::{layout_root, Constraints},
    render::{extract_items, RenderItemSnapshot},
    view::run_view,
};
use matcha_ecs_widgets::{ColorRect, Panel, Row};

fn children(world: &World, e: Entity) -> Vec<Entity> {
    world
        .get::<ViewChildren>(e)
        .map(|vc| vc.slots.iter().map(|(_, c)| *c).collect())
        .unwrap_or_default()
}

fn translation(item: &RenderItemSnapshot) -> [f32; 2] {
    let t = item.transform.column(3);
    [t.x, t.y]
}

/// `[min, max]` drawn rect of an extracted item in window space.
fn drawn_rect(item: &RenderItemSnapshot) -> ([f32; 2], [f32; 2]) {
    let pos = translation(item);
    (pos, [pos[0] + item.size[0], pos[1] + item.size[1]])
}

/// The Stretch regression: a `Row` with the default `AlignItems::Stretch`
/// must hand render a *stretched* size for a shorter child, not the child's
/// declared size — `LayoutOutput` was already correct, but the snapshot
/// (what the builder will draw at) has to agree with it.
#[test]
fn stretch_row_snapshot_sizes_match_the_allocated_not_declared_sizes() {
    let mut world = World::new();
    let root = world.spawn(ViewChildren::default()).id();
    run_view(&mut world, root, |s| {
        s.node(Row::new().gap(6.0), |s| {
            s.leaf(ColorRect::new(36.0, 70.0));
            s.leaf(ColorRect::new(36.0, 26.0)); // shorter: must stretch to 70
        });
    });
    layout_root(&mut world, root, Constraints::from_max_size([800.0, 600.0]));

    let items = extract_items(&world, root).items;
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].size, [36.0, 70.0]);
    assert_eq!(
        items[1].size,
        [36.0, 70.0],
        "the shorter child must be drawn at its stretched height, not its declared 26.0"
    );
}

/// Every extracted item's snapshot size must equal its `LayoutOutput::size` —
/// the general form of the contract, checked over a nested composition.
#[test]
fn every_snapshot_size_equals_the_entitys_layout_output_size() {
    let mut world = World::new();
    let root = world.spawn(ViewChildren::default()).id();
    run_view(&mut world, root, |s| {
        s.node(Row::new().gap(16.0), |s| {
            s.node(Panel::new(220.0, 100.0).border_width(2.0), |s| {
                s.leaf(ColorRect::new(200.0, 80.0));
            });
            s.node(Panel::new(110.0, 180.0).border_width(2.0), |s| {
                s.leaf(ColorRect::new(90.0, 160.0));
            });
        });
    });
    layout_root(&mut world, root, Constraints::from_max_size([800.0, 600.0]));

    // Collect every laid-out drawable entity's LayoutOutput in DFS order,
    // mirroring extract's own walk.
    fn collect_sizes(world: &World, e: Entity, out: &mut Vec<[f32; 2]>) {
        for child in children(world, e) {
            if world
                .get::<matcha_ecs::components::render::RenderItem>(child)
                .is_some()
            {
                out.push(world.get::<LayoutOutput>(child).unwrap().size);
            }
            collect_sizes(world, child, out);
        }
    }
    let mut layout_sizes = Vec::new();
    collect_sizes(&world, root, &mut layout_sizes);

    let items = extract_items(&world, root).items;
    assert_eq!(items.len(), layout_sizes.len());
    for (item, expected) in items.iter().zip(&layout_sizes) {
        assert_eq!(&item.size, expected, "snapshot size must equal LayoutOutput::size");
    }
}

/// The demo composition that overflowed: two fixed-size `Panel`s (one wide,
/// one tall) inside a default-`Stretch` `Row`. The wide panel gets stretched
/// to the row's 180px cross size; its child must be centred within — and
/// contained by — the *drawn* panel rect, which therefore must also be the
/// allocated (stretched) rect.
#[test]
fn stretched_panel_draws_at_its_allocated_size_and_contains_its_centred_child() {
    let mut world = World::new();
    let root = world.spawn(ViewChildren::default()).id();
    run_view(&mut world, root, |s| {
        s.node(Row::new().gap(16.0), |s| {
            s.node(Panel::new(220.0, 100.0).border_width(2.0), |s| {
                s.leaf(ColorRect::new(200.0, 80.0));
            });
            s.node(Panel::new(110.0, 180.0).border_width(2.0), |s| {
                s.leaf(ColorRect::new(90.0, 160.0));
            });
        });
    });
    layout_root(&mut world, root, Constraints::from_max_size([800.0, 600.0]));

    // DFS order: wide panel, its child, tall panel, its child.
    let items = extract_items(&world, root).items;
    assert_eq!(items.len(), 4);
    let (wide_panel, wide_child, tall_panel, tall_child) =
        (&items[0], &items[1], &items[2], &items[3]);

    // The wide panel is stretched to the row's cross size (the tall panel's
    // 180.0) and must *draw* at that size, not its declared 100.0.
    assert_eq!(wide_panel.size, [220.0, 180.0]);
    assert_eq!(tall_panel.size, [110.0, 180.0]);

    // Children keep their own sizes and are centred in the border-inset
    // (2px) inner area of the *allocated* panel box.
    assert_eq!(wide_child.size, [200.0, 80.0]);
    assert_eq!(translation(wide_child), [10.0, 50.0]); // 2+(216-200)/2, 2+(176-80)/2
    assert_eq!(tall_child.size, [90.0, 160.0]);
    assert_eq!(translation(tall_child), [246.0, 10.0]); // panel at x=236; 2+(106-90)/2, 2+(176-160)/2

    // Containment: each child's drawn rect lies inside its panel's drawn rect
    // (the exact invariant the original bug report violated).
    for (panel, child) in [(wide_panel, wide_child), (tall_panel, tall_child)] {
        let (p_min, p_max) = drawn_rect(panel);
        let (c_min, c_max) = drawn_rect(child);
        assert!(
            c_min[0] >= p_min[0] && c_min[1] >= p_min[1] && c_max[0] <= p_max[0] && c_max[1] <= p_max[1],
            "child rect {c_min:?}..{c_max:?} must be contained in panel rect {p_min:?}..{p_max:?}"
        );
    }
}
