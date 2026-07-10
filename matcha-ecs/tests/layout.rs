//! Headless verification of the layout core (M3): drive `layout_root`
//! directly against a bare `World`, no window/GPU needed, and assert the
//! resulting `LayoutOutput` values. Same style as `tests/reconcile.rs`.

use bevy_ecs::{entity::Entity, world::World};

use matcha_ecs::{
    components::{layout::LayoutOutput, view::ViewChildren},
    layout::{layout_root, Constraints},
    view::run_view,
};
use matcha_ecs_widgets::{AlignItems, Column, ColorRect, Row};

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
fn column_stacks_children_with_gap() {
    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        // Pin `AlignItems::Start` explicitly: this test is about gap-stacking,
        // not cross-axis alignment, and the default `AlignItems::Stretch`
        // (CSS's default) would otherwise stretch the narrower second child
        // to the column's own (widest-child) width — see
        // `align_items_stretch_expands_narrower_children_to_container_width`
        // below for a test of that behaviour.
        s.node(Column::new().gap(10.0).align_items(AlignItems::Start), |s| {
            s.leaf(ColorRect::new(50.0, 20.0));
            s.leaf(ColorRect::new(30.0, 40.0));
        });
    });
    layout_root(&mut world, root, Constraints::from_max_size([800.0, 600.0]));

    let column = children(&world, root)[0];
    let [first, second]: [Entity; 2] = children(&world, column).try_into().unwrap();

    let first_out = output(&world, first);
    assert_eq!(first_out.size, [50.0, 20.0]);
    assert_eq!(first_out.origin, [0.0, 0.0]);

    let second_out = output(&world, second);
    assert_eq!(second_out.size, [30.0, 40.0]);
    assert_eq!(second_out.origin, [0.0, 30.0]); // first height (20) + gap (10)

    let column_out = output(&world, column);
    assert_eq!(column_out.size, [50.0, 70.0]); // max width 50, total height 20+40+10
    assert_eq!(column_out.origin, [0.0, 0.0]); // top-level: window origin
}

#[test]
fn row_stacks_children_horizontally_with_gap() {
    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        s.node(Row::new().gap(5.0), |s| {
            s.leaf(ColorRect::new(20.0, 40.0));
            s.leaf(ColorRect::new(30.0, 10.0));
        });
    });
    layout_root(&mut world, root, Constraints::from_max_size([800.0, 600.0]));

    let row = children(&world, root)[0];
    let [first, second]: [Entity; 2] = children(&world, row).try_into().unwrap();

    let first_out = output(&world, first);
    assert_eq!(first_out.origin, [0.0, 0.0]);

    let second_out = output(&world, second);
    assert_eq!(second_out.origin, [25.0, 0.0]); // first width (20) + gap (5)

    let row_out = output(&world, row);
    assert_eq!(row_out.size, [55.0, 40.0]); // total width 20+30+5, max height 40
}

#[test]
fn nested_column_of_row_positions_grandchildren_in_window_space() {
    let (mut world, root) = setup();
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
    let [top_rect, row]: [Entity; 2] = children(&world, column).try_into().unwrap();
    let [row_first, row_second]: [Entity; 2] = children(&world, row).try_into().unwrap();

    assert_eq!(output(&world, top_rect).origin, [0.0, 0.0]);
    // Row's local origin within the column is [0, 100 + 20].
    assert_eq!(output(&world, row).origin, [0.0, 120.0]);
    // Grandchildren origins are local to the Row, not the window.
    assert_eq!(output(&world, row_first).origin, [0.0, 0.0]);
    assert_eq!(output(&world, row_second).origin, [120.0, 0.0]);

    // But GlobalTransform composes through the whole ancestor chain, so the
    // second row child's world-space position is column_origin + row_origin + local.
    let global = world
        .get::<matcha_ecs::components::layout::GlobalTransform>(row_second)
        .unwrap();
    let translation = global.affine.column(3);
    assert_eq!((translation.x, translation.y), (120.0, 120.0));
}

#[test]
fn align_items_stretch_expands_narrower_children_to_container_width() {
    // Default `AlignItems::Stretch` (CSS's default): a Column auto-sizes its
    // own width to its widest child (50), so a narrower second child (30)
    // gets stretched to fill that width.
    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        s.node(Column::new().gap(10.0), |s| {
            s.leaf(ColorRect::new(50.0, 20.0));
            s.leaf(ColorRect::new(30.0, 40.0));
        });
    });
    layout_root(&mut world, root, Constraints::from_max_size([800.0, 600.0]));

    let column = children(&world, root)[0];
    let [_first, second]: [Entity; 2] = children(&world, column).try_into().unwrap();
    assert_eq!(output(&world, second).size, [50.0, 40.0]);
}

#[test]
fn align_items_center_centres_narrower_children_on_the_cross_axis() {
    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        s.node(Column::new().gap(0.0).align_items(AlignItems::Center), |s| {
            s.leaf(ColorRect::new(50.0, 20.0));
            s.leaf(ColorRect::new(30.0, 40.0));
        });
    });
    layout_root(&mut world, root, Constraints::from_max_size([800.0, 600.0]));

    let column = children(&world, root)[0];
    let [_first, second]: [Entity; 2] = children(&world, column).try_into().unwrap();
    let second_out = output(&world, second);
    // Column width is 50 (widest child); a natural-width (30) child under
    // `Center` sits at x = (50 - 30) / 2 = 10, keeping its own size.
    assert_eq!(second_out.size, [30.0, 40.0]);
    assert_eq!(second_out.origin, [10.0, 20.0]);
}

#[test]
fn unchanged_layout_leaves_layout_output_arc_identity_stable() {
    // Re-running layout_root with an unchanged view must not mark
    // LayoutOutput as Changed (set_if_neq skips the write when equal).
    let (mut world, root) = setup();
    let build = |s: &mut matcha_ecs::view::Scope| {
        s.node(Column::new().gap(10.0), |s| {
            s.leaf(ColorRect::new(50.0, 20.0));
        });
    };
    run_view(&mut world, root, build);
    layout_root(&mut world, root, Constraints::from_max_size([800.0, 600.0]));

    let column = children(&world, root)[0];
    let before = output(&world, column);

    run_view(&mut world, root, build);
    layout_root(&mut world, root, Constraints::from_max_size([800.0, 600.0]));
    let after = output(&world, column);

    assert_eq!(before, after);
}
