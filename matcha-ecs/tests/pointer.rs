//! Headless verification of `:hover` and `:active`.
//!
//! The two things worth pinning are that hover follows CSS — the marker lands
//! on the whole ancestor chain, not just the innermost box — and that `:active`
//! is the *intersection* of the press chain with the current hover chain, so
//! dragging off a pressed button releases its pressed look.
//!
//! No window, no GPU: the picker is built directly, `RenderItem` builders are
//! never invoked.

use bevy_ecs::{bundle::Bundle, entity::Entity, world::{EntityWorldMut, World}};

use matcha_ecs::{
    components::{
        input::{Active, Hovered, Pickable},
        view::ViewChildren,
    },
    layout::{layout_root, Constraints},
    pick::{PickerResource, RectPicker},
    pointer::{set_position, set_pressed, sync_pointer_components, PointerState},
    view::{run_view, Widget},
};
use matcha_ecs_widgets::{AlignItems, ColorRect, Column, Container};

const WINDOW: [f32; 2] = [800.0, 600.0];

/// A pickable leaf — containers are deliberately not pickable, which is what
/// makes "hover reaches the ancestors" a real claim rather than a tautology.
struct Leaf(ColorRect);

impl Widget for Leaf {
    fn bundle(&self) -> impl Bundle {
        (self.0.bundle(), Pickable)
    }
    fn patch(&self, _entity: &mut EntityWorldMut) {}
}

fn children(world: &World, e: Entity) -> Vec<Entity> {
    world
        .get::<ViewChildren>(e)
        .map(|vc| vc.slots.iter().map(|(_, c)| *c).collect())
        .unwrap_or_default()
}

/// `(world, root, column, [first, second])` — two 100x50 leaves stacked in a
/// column, the first at y 0..50 and the second at y 50..100.
fn two_leaves() -> (World, Entity, Entity, [Entity; 2]) {
    let mut world = World::new();
    let root = world.spawn(ViewChildren::default()).id();

    run_view(&mut world, root, |s| {
        s.node(Container::new(), |s| {
            s.node(Column::new().align_items(AlignItems::Start), |s| {
                s.leaf(Leaf(ColorRect::new(100.0, 50.0).key(1u64)));
                s.leaf(Leaf(ColorRect::new(100.0, 50.0).key(2u64)));
            });
        });
    });
    layout_root(&mut world, root, Constraints::from_max_size(WINDOW));

    let container = children(&world, root)[0];
    let column = children(&world, container)[0];
    let kids: [Entity; 2] = children(&world, column).try_into().expect("two leaves");
    world.insert_resource(PickerResource(Box::new(RectPicker::build(&world, root))));
    (world, root, column, kids)
}

/// Move the pointer and apply the derived markers, as one frame would.
fn hover_at(world: &mut World, pos: Option<[f32; 2]>) {
    set_position(world, pos);
    sync_pointer_components(world);
}

#[test]
fn hovering_a_leaf_hovers_every_ancestor_too() {
    let (mut world, root, column, [first, second]) = two_leaves();
    hover_at(&mut world, Some([10.0, 10.0]));

    // CSS's rule: `:hover` matches the element *and* its ancestors.
    for e in [first, column, root] {
        assert!(
            world.get::<Hovered>(e).is_some(),
            "{e:?} should be hovered as part of the chain"
        );
    }
    assert!(world.get::<Hovered>(second).is_none());
}

#[test]
fn moving_between_siblings_moves_the_marker_and_leaves_the_shared_parent_alone() {
    let (mut world, _, column, [first, second]) = two_leaves();

    hover_at(&mut world, Some([10.0, 10.0]));
    hover_at(&mut world, Some([10.0, 70.0]));

    assert!(world.get::<Hovered>(first).is_none());
    assert!(world.get::<Hovered>(second).is_some());
    assert!(
        world.get::<Hovered>(column).is_some(),
        "the shared ancestor never stopped containing the pointer"
    );
}

#[test]
fn leaving_the_window_clears_hover_entirely() {
    let (mut world, _, _, [first, _]) = two_leaves();

    hover_at(&mut world, Some([10.0, 10.0]));
    hover_at(&mut world, None);

    assert!(world.get::<Hovered>(first).is_none());
    assert!(world.resource::<PointerState>().hover_path().is_empty());
}

#[test]
fn hovering_empty_space_hovers_nothing() {
    let (mut world, _, _, [first, _]) = two_leaves();
    hover_at(&mut world, Some([400.0, 400.0]));

    assert!(world.get::<Hovered>(first).is_none());
    assert_eq!(world.resource::<PointerState>().hit(), None);
}

#[test]
fn a_press_makes_the_chain_active() {
    let (mut world, root, column, [first, second]) = two_leaves();
    hover_at(&mut world, Some([10.0, 10.0]));

    set_pressed(&mut world, Some(first));
    sync_pointer_components(&mut world);

    for e in [first, column, root] {
        assert!(world.get::<Active>(e).is_some(), "{e:?} should be active");
    }
    assert!(world.get::<Active>(second).is_none());
}

#[test]
fn dragging_off_a_pressed_leaf_releases_it_and_dragging_back_restores_it() {
    let (mut world, _, _, [first, _]) = two_leaves();
    hover_at(&mut world, Some([10.0, 10.0]));
    set_pressed(&mut world, Some(first));
    sync_pointer_components(&mut world);
    assert!(world.get::<Active>(first).is_some());

    // Still held, but the pointer has wandered onto the sibling.
    hover_at(&mut world, Some([10.0, 70.0]));
    assert!(
        world.get::<Active>(first).is_none(),
        "a button should not keep looking pressed under a cursor that left it"
    );

    hover_at(&mut world, Some([10.0, 10.0]));
    assert!(world.get::<Active>(first).is_some());
}

#[test]
fn releasing_clears_active_but_not_hover() {
    let (mut world, _, _, [first, _]) = two_leaves();
    hover_at(&mut world, Some([10.0, 10.0]));
    set_pressed(&mut world, Some(first));
    sync_pointer_components(&mut world);

    set_pressed(&mut world, None);
    sync_pointer_components(&mut world);

    assert!(world.get::<Active>(first).is_none());
    assert!(world.get::<Hovered>(first).is_some());
}

#[test]
fn a_widget_appearing_under_a_stationary_cursor_comes_up_hovered() {
    // This is why hover is re-resolved every frame rather than only on a move.
    let (mut world, root, _, [first, _]) = two_leaves();
    hover_at(&mut world, Some([10.0, 10.0]));
    assert!(world.get::<Hovered>(first).is_some());

    // Rebuild the view with the first leaf gone; the second slides up into the
    // pointer's position without the pointer moving at all.
    run_view(&mut world, root, |s| {
        s.node(Container::new(), |s| {
            s.node(Column::new().align_items(AlignItems::Start), |s| {
                s.leaf(Leaf(ColorRect::new(100.0, 50.0).key(2u64)));
            });
        });
    });
    layout_root(&mut world, root, Constraints::from_max_size(WINDOW));
    world.insert_resource(PickerResource(Box::new(RectPicker::build(&world, root))));

    sync_pointer_components(&mut world);

    let survivor = children(&world, children(&world, children(&world, root)[0])[0])[0];
    assert!(
        world.get::<Hovered>(survivor).is_some(),
        "the widget that moved under the cursor should be hovered without a move event"
    );
}

#[test]
fn re_resolving_an_unchanged_state_reports_no_change() {
    let (mut world, _, _, _) = two_leaves();
    assert!(set_position(&mut world, Some([10.0, 10.0])));
    assert!(
        !set_position(&mut world, Some([10.0, 10.0])),
        "an unmoved pointer must not ask for a redraw"
    );
}
