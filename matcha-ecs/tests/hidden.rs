//! `display: none` — [`Hidden`] removes an entity and its subtree from layout,
//! drawing and picking alike.
//!
//! The point of these tests is that hiding is *absence*, not a zero size: a
//! hidden child must not leave its `Column`'s gap behind, and must not keep
//! being painted or clicked at wherever it was last arranged.
//!
//! Headless, GPU-free: `RenderItem` builders are never invoked, only the
//! extract-level snapshot list and `LayoutOutput` are inspected.

use bevy_ecs::{entity::Entity, world::World};

use matcha_ecs::{
    components::{
        layout::LayoutOutput,
        view::ViewChildren,
    },
    layout::{layout_root, Constraints},
    pick::{PickQuery, Picker, RectZPicker},
    render::extract_items,
    view::run_view,
};
use matcha_ecs_widgets::{AlignItems, Button, ColorRect, Column, Container};

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

/// Three stacked rects with a gap, the middle one optionally hidden.
fn stack(world: &mut World, root: Entity, middle_visible: bool) {
    run_view(world, root, move |s| {
        s.node(Column::new().gap(10.0).align_items(AlignItems::Start), |s| {
            s.leaf(ColorRect::new(50.0, 20.0).key(1u64));
            s.node(Container::new().visible(middle_visible).key(2u64), |s| {
                s.leaf(ColorRect::new(50.0, 30.0));
            });
            s.leaf(ColorRect::new(50.0, 40.0).key(3u64));
        });
    });
    layout_root(world, root, Constraints::from_max_size([800.0, 600.0]));
}

#[test]
fn a_hidden_child_claims_no_space_and_no_gap() {
    let (mut world, root) = setup();
    stack(&mut world, root, true);

    let column = children(&world, root)[0];
    let visible_total = world.get::<LayoutOutput>(column).unwrap().size[1];
    // 20 + 30 + 40 with two 10px gaps.
    assert_eq!(visible_total, 110.0);

    stack(&mut world, root, false);
    let hidden_total = world.get::<LayoutOutput>(column).unwrap().size[1];
    // 20 + 40 with a single gap: the hidden child took its gap with it. A
    // zero-sized child would have left 70.0 here.
    assert_eq!(hidden_total, 70.0);

    // And the surviving sibling closed up rather than staying put.
    let last = children(&world, column)[2];
    assert_eq!(world.get::<LayoutOutput>(last).unwrap().origin, [0.0, 30.0]);
}

#[test]
fn a_hidden_subtree_is_not_extracted_for_drawing() {
    let (mut world, root) = setup();

    stack(&mut world, root, true);
    let visible_items = extract_items(&world, root).items.len();
    assert_eq!(visible_items, 3);

    stack(&mut world, root, false);
    let hidden_items = extract_items(&world, root).items.len();
    assert_eq!(
        hidden_items, 2,
        "the hidden container's descendant must not be painted at its stale transform"
    );
}

#[test]
fn a_hidden_subtree_is_not_pickable() {
    let (mut world, root) = setup();

    // A button inside a container, sitting under a plain rect so there is
    // something to hit either way.
    let build = |world: &mut World, visible: bool| {
        run_view(world, root, move |s| {
            s.node(Column::new().align_items(AlignItems::Start), |s| {
                s.node(Container::new().visible(visible).key(1u64), |s| {
                    s.leaf(Button::<()>::new("hit me").size(100.0, 40.0));
                });
            });
        });
        layout_root(world, root, Constraints::from_max_size([800.0, 600.0]));
    };

    build(&mut world, true);
    let picker = RectZPicker::build(&world, root);
    let hit = picker.pick(
        &world,
        &PickQuery {
            viewport_pos: [50.0, 20.0],
        },
    );
    assert!(hit.is_some(), "the button should be pickable while shown");

    build(&mut world, false);
    let picker = RectZPicker::build(&world, root);
    let hit = picker.pick(
        &world,
        &PickQuery {
            viewport_pos: [50.0, 20.0],
        },
    );
    assert!(
        hit.is_none(),
        "a hidden button must not keep an invisible hit target behind"
    );
}

#[test]
fn re_showing_restores_layout_drawing_and_picking() {
    let (mut world, root) = setup();

    stack(&mut world, root, true);
    let column = children(&world, root)[0];
    let before = world.get::<LayoutOutput>(column).unwrap().size;
    let before_items = extract_items(&world, root).items.len();

    stack(&mut world, root, false);
    stack(&mut world, root, true);

    assert_eq!(world.get::<LayoutOutput>(column).unwrap().size, before);
    assert_eq!(extract_items(&world, root).items.len(), before_items);
}

#[test]
fn hiding_keeps_the_subtree_alive_rather_than_despawning_it() {
    // Hiding is not pruning: the entities stay, so whatever state they hold
    // (a text box's contents, a scroll offset) survives being hidden.
    let (mut world, root) = setup();
    stack(&mut world, root, true);

    let column = children(&world, root)[0];
    let container = children(&world, column)[1];
    let inner = children(&world, container)[0];

    stack(&mut world, root, false);

    assert!(world.entities().contains(inner));
    assert_eq!(children(&world, column)[1], container);
    assert_eq!(children(&world, container)[0], inner);
}
