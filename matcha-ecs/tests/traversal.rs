//! Painting and picking share one order.
//!
//! The invariant here is not "z-index works" but "the two agree". Under the
//! painter's algorithm the entity drawn last is the one on top, so picking is
//! the reverse of paint order — and a second, independent ordering would be a
//! way for what the user clicks to stop being what the user sees. Both now
//! come out of `traversal::walk`, and these tests check the agreement rather
//! than each side separately.
//!
//! Headless, GPU-free: the extract snapshot list and the picker, no builders.

use bevy_ecs::{entity::Entity, world::World};

use matcha_ecs::{
    components::{render::ZIndex, view::ViewChildren},
    layout::{layout_root, Constraints},
    pick::{PickQuery, Picker, RectPicker},
    render::extract_items,
    view::run_view,
};
use matcha_ecs_widgets::{AlignItems, Button, Column, Panel};

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

/// The entities that draw, in paint order.
fn painted(world: &World, root: Entity) -> Vec<Entity> {
    extract_items(world, root)
        .items
        .iter()
        .map(|i| i.entity)
        .collect()
}

fn pick_at(world: &World, root: Entity, pos: [f32; 2]) -> Option<Entity> {
    RectPicker::build(world, root)
        .pick(world, &PickQuery { viewport_pos: pos })
        .map(|h| h.entity)
}

/// `(world, root, a, b)` where `a` and `b` occupy exactly the same box, so
/// only stacking decides which is in front.
///
/// The overlap is imposed after layout rather than produced by it: no
/// container in this crate stacks two children on top of each other, and
/// which one does is beside the point here.
fn two_overlapping() -> (World, Entity, Entity, Entity) {
    use matcha_ecs::components::layout::{GlobalTransform, LayoutOutput};
    use nalgebra::{Matrix4, Vector3};

    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        s.node(Column::new().align_items(AlignItems::Start), |s| {
            s.leaf(Button::<()>::new("a").key(0u64).size(100.0, 100.0));
            s.leaf(Button::<()>::new("b").key(1u64).size(100.0, 100.0));
        });
    });
    layout_root(&mut world, root, Constraints::from_max_size(WINDOW));

    let column = children(&world, root)[0];
    let [a, b]: [Entity; 2] = children(&world, column).try_into().unwrap();
    for e in [a, b] {
        world.entity_mut(e).insert((
            LayoutOutput {
                size: [100.0, 100.0],
                origin: [0.0, 0.0],
            },
            GlobalTransform {
                affine: Matrix4::new_translation(&Vector3::new(0.0, 0.0, 0.0)),
            },
        ));
    }
    (world, root, a, b)
}

#[test]
fn declaration_order_is_paint_order_and_the_last_one_wins_a_pick() {
    let (world, root, a, b) = two_overlapping();

    assert_eq!(painted(&world, root), vec![a, b]);
    assert_eq!(pick_at(&world, root, [50.0, 50.0]), Some(b));
}

#[test]
fn z_index_moves_painting_and_picking_together() {
    let (mut world, root, a, b) = two_overlapping();

    // Raise the earlier-declared one over its sibling.
    world.entity_mut(a).insert(ZIndex(1));

    assert_eq!(
        painted(&world, root),
        vec![b, a],
        "the raised sibling must now be painted last"
    );
    assert_eq!(
        pick_at(&world, root, [50.0, 50.0]),
        Some(a),
        "and the pick must follow it, not stay with what used to be on top"
    );
}

#[test]
fn siblings_sharing_a_z_index_keep_declaration_order() {
    let (mut world, root, a, b) = two_overlapping();

    world.entity_mut(a).insert(ZIndex(2));
    world.entity_mut(b).insert(ZIndex(2));

    assert_eq!(painted(&world, root), vec![a, b]);
    assert_eq!(pick_at(&world, root, [50.0, 50.0]), Some(b));
}

#[test]
fn a_child_never_goes_behind_its_parent_however_negative_its_z_index() {
    // Stacking reorders siblings and nothing else, which is what keeps this to
    // one sort instead of CSS's layered paint order.
    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        s.node(Panel::new(200.0, 200.0).key(9u64), |s| {
            s.leaf(Button::<()>::new("inner").key(0u64));
        });
    });
    layout_root(&mut world, root, Constraints::from_max_size(WINDOW));

    let panel = children(&world, root)[0];
    let inner = children(&world, panel)[0];
    world.entity_mut(inner).insert(ZIndex(-100));

    assert_eq!(painted(&world, root), vec![panel, inner]);
}

#[test]
fn stacking_moves_a_whole_subtree_rather_than_one_box() {
    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        s.node(Column::new().align_items(AlignItems::Start), |s| {
            s.node(Panel::new(50.0, 50.0).key(0u64), |s| {
                s.leaf(Button::<()>::new("in-a").key(10u64).size(10.0, 10.0));
            });
            s.node(Panel::new(50.0, 50.0).key(1u64), |s| {
                s.leaf(Button::<()>::new("in-b").key(11u64).size(10.0, 10.0));
            });
        });
    });
    layout_root(&mut world, root, Constraints::from_max_size(WINDOW));

    let column = children(&world, root)[0];
    let [first, second]: [Entity; 2] = children(&world, column).try_into().unwrap();
    let first_child = children(&world, first)[0];
    let second_child = children(&world, second)[0];

    world.entity_mut(first).insert(ZIndex(1));

    // The raised panel takes its child with it; the two subtrees stay
    // contiguous rather than interleaving.
    assert_eq!(
        painted(&world, root),
        vec![second, second_child, first, first_child]
    );
}

#[test]
fn a_pick_always_returns_whatever_was_painted_last_at_that_point() {
    // The invariant stated directly, across several arrangements: nothing here
    // knows *why* the order came out as it did, only that both sides used it.
    let (mut world, root, a, b) = two_overlapping();

    for (za, zb) in [(0, 0), (1, 0), (0, 1), (-3, -7), (5, 5)] {
        world.entity_mut(a).insert(ZIndex(za));
        world.entity_mut(b).insert(ZIndex(zb));

        let painted_last = *painted(&world, root).last().expect("something was painted");
        assert_eq!(
            pick_at(&world, root, [50.0, 50.0]),
            Some(painted_last),
            "disagreement at z = ({za}, {zb})"
        );
    }
}
