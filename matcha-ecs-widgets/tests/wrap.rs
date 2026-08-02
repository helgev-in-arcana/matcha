//! `flex-wrap` and `align-content`: a container whose children do not fit on
//! one line breaks them into several, then shares out whatever cross-axis
//! space is left between those lines.
//!
//! Headless, GPU-free: only `LayoutOutput` is inspected.

use bevy_ecs::{entity::Entity, world::World};

use matcha_ecs::{
    components::{layout::LayoutOutput, view::ViewChildren},
    layout::{layout_root, Constraints},
    view::{run_view, Scope},
};
use matcha_ecs_widgets::{AlignItems, ColorRect, JustifyContent, Length, Row, Wrap};

const WINDOW: [f32; 2] = [800.0, 600.0];

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

/// Returns `(world, container, children)`.
fn run(build: impl Fn(&mut Scope) + Send + Sync + 'static) -> (World, Entity, Vec<Entity>) {
    let (mut world, root) = setup();
    run_view(&mut world, root, build);
    layout_root(&mut world, root, Constraints::from_max_size(WINDOW));
    let top = children(&world, root)[0];
    let kids = children(&world, top);
    (world, top, kids)
}

/// Three 100×20 rects in a 250-wide row: two fit on a line, the third wraps.
fn three_in_a_narrow_row(
    row: impl Fn() -> Row + Send + Sync + 'static,
) -> (World, Entity, Vec<Entity>) {
    run(move |s| {
        s.node(row(), |s| {
            s.leaf(ColorRect::new(100.0, 20.0).key(1u64));
            s.leaf(ColorRect::new(100.0, 20.0).key(2u64));
            s.leaf(ColorRect::new(100.0, 30.0).key(3u64));
        });
    })
}

#[test]
fn without_wrapping_everything_stays_on_one_line_and_overflows() {
    let (world, _, kids) = three_in_a_narrow_row(|| {
        Row::new()
            .width(Length::Px(250.0))
            .align_items(AlignItems::Start)
            .shrink(0.0)
    });

    // Same row, no line breaks: each starts where the previous ended.
    assert_eq!(output(&world, kids[0]).origin, [0.0, 0.0]);
    assert_eq!(output(&world, kids[2]).origin[1], 0.0);
}

#[test]
fn a_child_that_does_not_fit_starts_a_new_line() {
    let (world, _, kids) = three_in_a_narrow_row(|| {
        Row::new()
            .width(Length::Px(250.0))
            .wrap(Wrap::Wrap)
            .align_items(AlignItems::Start)
    });

    assert_eq!(output(&world, kids[0]).origin, [0.0, 0.0]);
    assert_eq!(output(&world, kids[1]).origin, [100.0, 0.0]);
    // Third wraps: back to the start of the main axis, one line down.
    assert_eq!(output(&world, kids[2]).origin, [0.0, 20.0]);
}

#[test]
fn a_wrapping_row_is_as_deep_as_its_lines_stacked_up() {
    let (world, row, _) = three_in_a_narrow_row(|| {
        Row::new()
            .width(Length::Px(250.0))
            .wrap(Wrap::Wrap)
            .align_items(AlignItems::Start)
    });

    // Two lines, 20 and 30 tall.
    assert_eq!(output(&world, row).size[1], 50.0);
}

#[test]
fn the_gap_applies_between_lines_as_well_as_within_them() {
    let (world, row, kids) = three_in_a_narrow_row(|| {
        Row::new()
            .width(Length::Px(250.0))
            .gap(10.0)
            .wrap(Wrap::Wrap)
            .align_items(AlignItems::Start)
    });

    // 100 + 10 + 100 = 210 fits in 250; adding the third would not.
    assert_eq!(output(&world, kids[1]).origin[0], 110.0);
    assert_eq!(output(&world, kids[2]).origin, [0.0, 30.0]);
    assert_eq!(output(&world, row).size[1], 60.0);
}

#[test]
fn wrap_reverse_stacks_the_same_lines_from_the_far_edge() {
    let (world, _, kids) = three_in_a_narrow_row(|| {
        Row::new()
            .width(Length::Px(250.0))
            .wrap(Wrap::WrapReverse)
            .align_items(AlignItems::Start)
    });

    // The wrapped child's line comes first; the order *within* a line is
    // unchanged, which is what distinguishes this from `.reverse(true)`.
    assert_eq!(output(&world, kids[2]).origin, [0.0, 0.0]);
    assert_eq!(output(&world, kids[0]).origin, [0.0, 30.0]);
    assert_eq!(output(&world, kids[1]).origin, [100.0, 30.0]);
}

#[test]
fn align_content_shares_leftover_cross_space_between_the_lines() {
    let (world, _, kids) = run(|s| {
        s.node(
            Row::new()
                .width(Length::Px(250.0))
                .height(Length::Px(200.0))
                .wrap(Wrap::Wrap)
                .align_items(AlignItems::Start)
                .align_content(JustifyContent::SpaceBetween),
            |s| {
                s.leaf(ColorRect::new(100.0, 20.0).key(1u64));
                s.leaf(ColorRect::new(100.0, 20.0).key(2u64));
                s.leaf(ColorRect::new(100.0, 30.0).key(3u64));
            },
        );
    });

    // Two lines in 200: first pinned to the top, second flush with the bottom.
    assert_eq!(output(&world, kids[0]).origin[1], 0.0);
    let last = output(&world, kids[2]);
    assert_eq!(last.origin[1] + last.size[1], 200.0);
}

#[test]
fn each_line_justifies_and_distributes_on_its_own() {
    // The second line has one child and the whole row's width to itself, so
    // centring it puts it in the middle while the full first line does not
    // move at all.
    let (world, _, kids) = three_in_a_narrow_row(|| {
        Row::new()
            .width(Length::Px(250.0))
            .wrap(Wrap::Wrap)
            .align_items(AlignItems::Start)
            .justify_content(JustifyContent::Center)
    });

    // First line: 200 of 250 used, 25 either side.
    assert_eq!(output(&world, kids[0]).origin[0], 25.0);
    // Second line: 100 of 250 used, 75 either side.
    assert_eq!(output(&world, kids[2]).origin[0], 75.0);
}

#[test]
fn a_child_wider_than_the_container_takes_a_line_of_its_own() {
    let (world, _, kids) = run(|s| {
        s.node(
            Row::new()
                .width(Length::Px(100.0))
                .wrap(Wrap::Wrap)
                .align_items(AlignItems::Start),
            |s| {
                s.leaf(ColorRect::new(300.0, 20.0).key(1u64).shrink(0.0));
                s.leaf(ColorRect::new(50.0, 20.0).key(2u64));
            },
        );
    });

    assert_eq!(output(&world, kids[0]).origin, [0.0, 0.0]);
    assert_eq!(output(&world, kids[1]).origin, [0.0, 20.0]);
}
