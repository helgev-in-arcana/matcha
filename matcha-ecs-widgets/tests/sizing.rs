//! CSS sizing (`width`/`height`/`min-*`/`max-*`/`aspect-ratio`) driven
//! end-to-end through `layout_root`, and the `justify_content` behaviour it
//! unlocks.
//!
//! `justify_content` distributes a container's *leftover* main-axis space, so
//! it could not be exercised at all while every container was fit-content:
//! there was never any leftover. Giving a `Column` a main-axis size of its own
//! is what makes it reachable, which is why those tests live here rather than
//! in `tests/layout.rs`.
//!
//! Headless, GPU-free: only `LayoutOutput` is inspected.

use bevy_ecs::{entity::Entity, world::World};

use matcha_ecs::{
    components::{layout::LayoutOutput, view::ViewChildren},
    layout::{layout_root, Constraints},
    view::run_view,
};
use matcha_ecs_widgets::{AlignItems, ColorRect, Column, Container, JustifyContent, Length, Row};

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

/// Two stacked rects (20 and 40 tall, no gap) inside a column built by `build`.
fn column_of_two(
    world: &mut World,
    root: Entity,
    build: impl Fn() -> Column + Send + Sync + 'static,
) -> (Entity, Entity, Entity) {
    run_view(world, root, move |s| {
        s.node(build(), |s| {
            s.leaf(ColorRect::new(50.0, 20.0).key(1u64));
            s.leaf(ColorRect::new(50.0, 40.0).key(2u64));
        });
    });
    layout_root(world, root, Constraints::from_max_size(WINDOW));

    let column = children(world, root)[0];
    let kids = children(world, column);
    (column, kids[0], kids[1])
}

#[test]
fn a_column_left_at_auto_is_still_exactly_as_tall_as_its_content() {
    let (mut world, root) = setup();
    let (column, ..) = column_of_two(&mut world, root, || {
        Column::new().align_items(AlignItems::Start)
    });
    assert_eq!(output(&world, column).size, [50.0, 60.0]);
}

#[test]
fn fill_makes_a_column_as_tall_as_the_space_it_was_offered() {
    let (mut world, root) = setup();
    let (column, ..) = column_of_two(&mut world, root, || {
        Column::new()
            .height(Length::Fill)
            .align_items(AlignItems::Start)
    });
    assert_eq!(output(&world, column).size, [50.0, WINDOW[1]]);
}

#[test]
fn justify_content_center_centres_the_children_in_the_leftover_space() {
    let (mut world, root) = setup();
    let (_, first, second) = column_of_two(&mut world, root, || {
        Column::new()
            .height(Length::Fill)
            .align_items(AlignItems::Start)
            .justify_content(JustifyContent::Center)
    });

    // 600 tall, 60 of content: 540 leftover, half of it above.
    assert_eq!(output(&world, first).origin[1], 270.0);
    assert_eq!(output(&world, second).origin[1], 290.0);
}

#[test]
fn justify_content_end_pushes_the_children_flush_with_the_far_edge() {
    let (mut world, root) = setup();
    let (_, first, second) = column_of_two(&mut world, root, || {
        Column::new()
            .height(Length::Fill)
            .align_items(AlignItems::Start)
            .justify_content(JustifyContent::End)
    });

    assert_eq!(output(&world, first).origin[1], 540.0);
    let last = output(&world, second);
    assert_eq!(last.origin[1] + last.size[1], WINDOW[1]);
}

#[test]
fn justify_content_space_between_pins_the_ends_and_inflates_the_gap() {
    let (mut world, root) = setup();
    let (_, first, second) = column_of_two(&mut world, root, || {
        Column::new()
            .height(Length::Fill)
            .align_items(AlignItems::Start)
            .justify_content(JustifyContent::SpaceBetween)
    });

    assert_eq!(output(&world, first).origin[1], 0.0);
    let last = output(&world, second);
    assert_eq!(last.origin[1] + last.size[1], WINDOW[1]);
}

#[test]
fn an_explicit_width_sizes_the_column_and_stretch_carries_it_to_the_children() {
    let (mut world, root) = setup();
    let (column, first, _) = column_of_two(&mut world, root, || {
        Column::new().width(Length::Px(300.0))
    });

    assert_eq!(output(&world, column).size[0], 300.0);
    // Default `AlignItems::Stretch`, so the children take the column's width
    // rather than their own 50.
    assert_eq!(output(&world, first).size[0], 300.0);
}

#[test]
fn a_percentage_is_a_fraction_of_the_space_the_parent_offered() {
    let (mut world, root) = setup();
    let (column, ..) = column_of_two(&mut world, root, || {
        Column::new()
            .width(Length::Percent(25.0))
            .align_items(AlignItems::Start)
    });
    assert_eq!(output(&world, column).size[0], 200.0);
}

#[test]
fn min_and_max_bound_a_content_sized_column() {
    let (mut world, root) = setup();
    let (column, ..) = column_of_two(&mut world, root, || {
        Column::new()
            .min_width(Length::Px(120.0))
            .max_height(Length::Px(25.0))
            .align_items(AlignItems::Start)
    });
    // Content is 50 × 60; the bounds win on both axes.
    assert_eq!(output(&world, column).size, [120.0, 25.0]);
}

#[test]
fn aspect_ratio_derives_the_axis_left_auto() {
    let (mut world, root) = setup();
    let (column, ..) = column_of_two(&mut world, root, || {
        Column::new()
            .width(Length::Px(300.0))
            .aspect_ratio(3.0 / 2.0)
            .align_items(AlignItems::Start)
    });
    assert_eq!(output(&world, column).size, [300.0, 200.0]);
}

#[test]
fn sizing_is_available_on_container_and_row_too() {
    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        s.node(Container::new().width(Length::Px(400.0)), |s| {
            s.node(Row::new().height(Length::Px(90.0)), |s| {
                s.leaf(ColorRect::new(50.0, 20.0));
            });
        });
    });
    layout_root(&mut world, root, Constraints::from_max_size(WINDOW));

    let container = children(&world, root)[0];
    let row = children(&world, container)[0];
    assert_eq!(output(&world, container).size[0], 400.0);
    assert_eq!(output(&world, row).size[1], 90.0);
}

#[test]
fn a_re_declared_sizing_change_takes_effect_on_the_next_pass() {
    let (mut world, root) = setup();

    let (column, ..) = column_of_two(&mut world, root, || {
        Column::new().align_items(AlignItems::Start)
    });
    assert_eq!(output(&world, column).size[1], 60.0);

    let (column, ..) = column_of_two(&mut world, root, || {
        Column::new()
            .height(Length::Fill)
            .align_items(AlignItems::Start)
    });
    assert_eq!(output(&world, column).size[1], WINDOW[1]);
}
