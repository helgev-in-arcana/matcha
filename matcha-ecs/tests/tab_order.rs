//! Sequential focus navigation.
//!
//! The interesting claims are that tab order *is* document order (no second
//! ordering to drift), that a `Claim` container is one stop rather than
//! several, and that a hidden subtree drops out — all three of which come from
//! reusing `traversal::walk` rather than from code here.

use bevy_ecs::{bundle::Bundle, entity::Entity, world::{EntityWorldMut, World}};

use matcha_ecs::{
    components::{
        focus::FocusPolicy,
        layout::Hidden,
        view::ViewChildren,
    },
    focus::{request_focus, Focus},
    tab_order::{focusable_in_order, next_focusable, TabDirection},
    view::{run_view, Widget},
};
use matcha_ecs_widgets::{ColorRect, Column, Container};

use TabDirection::{Backward, Forward};

/// A focusable leaf.
struct Stop {
    inner: ColorRect,
    policy: FocusPolicy,
}

impl Stop {
    fn new(key: u64) -> Self {
        Self {
            inner: ColorRect::new(20.0, 20.0).key(key),
            policy: FocusPolicy::Normal,
        }
    }
    fn claiming(key: u64) -> Self {
        Self {
            policy: FocusPolicy::Claim,
            ..Self::new(key)
        }
    }
}

impl Widget for Stop {
    fn bundle(&self) -> impl Bundle {
        (self.inner.bundle(), self.policy)
    }
    fn patch(&self, _entity: &mut EntityWorldMut) {}
}

fn setup() -> (World, Entity) {
    let mut world = World::new();
    world.insert_resource(Focus::default());
    let root = world.spawn(ViewChildren::default()).id();
    (world, root)
}

fn children(world: &World, e: Entity) -> Vec<Entity> {
    world
        .get::<ViewChildren>(e)
        .map(|vc| vc.slots.iter().map(|(_, c)| *c).collect())
        .unwrap_or_default()
}

/// Three focusable leaves, the middle one nested a level deeper — so "document
/// order" and "sibling order" are genuinely different things here.
fn three_stops() -> (World, Entity, Vec<Entity>) {
    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        s.node(Column::new(), |s| {
            s.leaf(Stop::new(1));
            s.node(Container::new().key(2u64), |s| {
                s.leaf(Stop::new(3));
            });
            s.leaf(Stop::new(4));
        });
    });
    let column = children(&world, root)[0];
    let top = children(&world, column);
    let stops = vec![top[0], children(&world, top[1])[0], top[2]];
    (world, root, stops)
}

#[test]
fn tab_order_is_document_order_including_across_nesting() {
    let (world, root, stops) = three_stops();
    assert_eq!(focusable_in_order(&world, root), stops);
}

#[test]
fn the_first_tab_press_focuses_the_first_stop_and_shift_tab_the_last() {
    let (world, root, stops) = three_stops();
    assert_eq!(next_focusable(&world, root, Forward), Some(stops[0]));
    assert_eq!(next_focusable(&world, root, Backward), Some(stops[2]));
}

#[test]
fn tab_steps_forward_and_wraps_at_the_end() {
    let (mut world, root, stops) = three_stops();

    request_focus(&mut world, stops[0]);
    assert_eq!(next_focusable(&world, root, Forward), Some(stops[1]));

    request_focus(&mut world, stops[2]);
    assert_eq!(
        next_focusable(&world, root, Forward),
        Some(stops[0]),
        "the last stop wraps to the first"
    );
}

#[test]
fn shift_tab_steps_backward_and_wraps_at_the_start() {
    let (mut world, root, stops) = three_stops();

    request_focus(&mut world, stops[1]);
    assert_eq!(next_focusable(&world, root, Backward), Some(stops[0]));

    request_focus(&mut world, stops[0]);
    assert_eq!(next_focusable(&world, root, Backward), Some(stops[2]));
}

#[test]
fn a_claiming_container_is_one_stop_and_its_children_are_not() {
    // What stops a text box's decorative children from becoming tab stops of
    // their own.
    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        s.node(Column::new(), |s| {
            s.leaf(Stop::new(1));
            s.node(Stop::claiming(2), |s| {
                s.leaf(Stop::new(3));
                s.leaf(Stop::new(4));
            });
        });
    });

    let column = children(&world, root)[0];
    let top = children(&world, column);
    assert_eq!(
        focusable_in_order(&world, root),
        vec![top[0], top[1]],
        "the claimer replaces its subtree in the order"
    );
}

#[test]
fn a_hidden_subtree_has_no_tab_stops() {
    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        s.node(Column::new(), |s| {
            s.leaf(Stop::new(1));
            s.node(Container::new().key(2u64), |s| {
                s.leaf(Stop::new(3));
            });
        });
    });

    let column = children(&world, root)[0];
    let top = children(&world, column);
    assert_eq!(focusable_in_order(&world, root).len(), 2);

    // `display: none` removes it from the keyboard too, not just from layout.
    world.entity_mut(top[1]).insert(Hidden);
    assert_eq!(focusable_in_order(&world, root), vec![top[0]]);
}

#[test]
fn nothing_focusable_means_nowhere_to_go() {
    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        s.node(Column::new(), |s| {
            s.leaf(ColorRect::new(20.0, 20.0));
        });
    });
    assert_eq!(next_focusable(&world, root, Forward), None);
    assert_eq!(next_focusable(&world, root, Backward), None);
}

#[test]
fn a_focused_entity_that_left_the_tree_restarts_the_sequence() {
    // Focus can outlive the entity it points at for a frame; tabbing must not
    // be stuck when that happens.
    let (mut world, root, stops) = three_stops();
    request_focus(&mut world, stops[1]);

    run_view(&mut world, root, |s| {
        s.node(Column::new(), |s| {
            s.leaf(Stop::new(1));
            s.leaf(Stop::new(4));
        });
    });

    let survivors = focusable_in_order(&world, root);
    assert_eq!(survivors.len(), 2);
    assert_eq!(
        next_focusable(&world, root, Forward),
        Some(survivors[0]),
        "an unknown current position starts from the beginning"
    );
}
