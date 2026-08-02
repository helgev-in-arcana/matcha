//! The box decoration model, exercised through the widgets that consume it.
//!
//! Rasterisation itself is unit-tested inside `matcha-ecs-widgets::shape`
//! (pure `Vec<u8>` in, `Vec<u8>` out). What is checked here is the part that
//! needs a world: that declaring a border, radius or shadow reaches the entity,
//! that a change to any of them invalidates the cached node while a no-op
//! re-declare does not, and that the border a `Panel` *paints* is the one it
//! insets its child by.
//!
//! GPU-free, per this suite's convention: `RenderItem::builder` is never
//! invoked, only `cache` identity and `LayoutOutput` are inspected.

use std::sync::Arc;

use bevy_ecs::{entity::Entity, world::World};

use matcha_ecs::{
    components::{layout::LayoutOutput, render::RenderItem, view::ViewChildren},
    layout::{layout_root, Constraints},
    view::{run_view, Scope},
};
use matcha_ecs_widgets::{
    box_style::{BoxShadow, BoxStyle, Corners, Sides},
    Button, Checkbox, ColorRect, Panel,
};

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

fn cache_of(world: &World, e: Entity) -> Arc<parking_lot::Mutex<Option<Arc<renderer::RenderNode>>>> {
    world
        .get::<RenderItem>(e)
        .expect("entity draws")
        .cache
        .clone()
}

/// Declare `build` twice and report whether the second pass dropped the cached
/// render node — i.e. whether the change was recognised as draw-relevant.
fn rebuilds_between(
    first: impl Fn(&mut Scope) + Send + Sync + 'static,
    second: impl Fn(&mut Scope) + Send + Sync + 'static,
) -> bool {
    let (mut world, root) = setup();
    run_view(&mut world, root, first);
    let target = children(&world, root)[0];
    let before = cache_of(&world, target);

    run_view(&mut world, root, second);
    !Arc::ptr_eq(&before, &cache_of(&world, target))
}

#[test]
fn a_panel_insets_its_child_by_the_border_it_paints() {
    // The one place the decoration and the layout have to agree: whatever
    // `box_node` paints as the border is exactly what the child sits inside.
    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        s.node(
            Panel::new(200.0, 200.0).borders(Sides {
                top: 10.0,
                right: 4.0,
                bottom: 30.0,
                left: 20.0,
            }),
            |s| {
                s.leaf(ColorRect::new(20.0, 20.0));
            },
        );
    });
    layout_root(&mut world, root, Constraints::from_max_size(WINDOW));

    let panel = children(&world, root)[0];
    let child = children(&world, panel)[0];
    let child_box = *world.get::<LayoutOutput>(child).expect("child laid out");

    // Inner area: x 20..196, y 10..170. A 20x20 child, centred.
    assert_eq!(child_box.origin[0], 20.0 + (176.0 - 20.0) / 2.0);
    assert_eq!(child_box.origin[1], 10.0 + (160.0 - 20.0) / 2.0);
}

#[test]
fn a_uniform_border_still_centres_its_child() {
    // Regression guard for the per-side rewrite: the common case must not have
    // shifted.
    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        s.node(Panel::new(100.0, 100.0).border_width(10.0), |s| {
            s.leaf(ColorRect::new(20.0, 20.0));
        });
    });
    layout_root(&mut world, root, Constraints::from_max_size(WINDOW));

    let panel = children(&world, root)[0];
    let child = children(&world, panel)[0];
    assert_eq!(
        world.get::<LayoutOutput>(child).unwrap().origin,
        [40.0, 40.0]
    );
}

#[test]
fn changing_a_panels_radius_rebuilds_it_and_re_declaring_the_same_one_does_not() {
    assert!(
        rebuilds_between(
            |s| {
                s.leaf(Panel::new(100.0, 100.0).radius(0.0));
            },
            |s| {
                s.leaf(Panel::new(100.0, 100.0).radius(8.0));
            },
        ),
        "a new corner radius changes what is drawn"
    );
    assert!(
        !rebuilds_between(
            |s| {
                s.leaf(Panel::new(100.0, 100.0).radius(8.0));
            },
            |s| {
                s.leaf(Panel::new(100.0, 100.0).radius(8.0));
            },
        ),
        "re-declaring an unchanged panel must reuse the cached node"
    );
}

#[test]
fn changing_a_panels_shadow_rebuilds_it() {
    assert!(rebuilds_between(
        |s| {
            s.leaf(Panel::new(100.0, 100.0));
        },
        |s| {
            s.leaf(Panel::new(100.0, 100.0).shadow(BoxShadow::drop(4.0, 12.0, [0.0, 0.0, 0.0, 0.5])));
        },
    ));
}

#[test]
fn changing_a_color_rects_radius_rebuilds_it() {
    assert!(rebuilds_between(
        |s| {
            s.leaf(ColorRect::new(50.0, 50.0).radius(0.0));
        },
        |s| {
            s.leaf(ColorRect::new(50.0, 50.0).radius(25.0));
        },
    ));
    assert!(!rebuilds_between(
        |s| {
            s.leaf(ColorRect::new(50.0, 50.0).radius(25.0));
        },
        |s| {
            s.leaf(ColorRect::new(50.0, 50.0).radius(25.0));
        },
    ));
}

#[test]
fn changing_a_checkboxs_radius_rebuilds_it() {
    assert!(rebuilds_between(
        |s| {
            s.leaf(Checkbox::<()>::new(false));
        },
        |s| {
            s.leaf(Checkbox::<()>::new(false).radius(10.0));
        },
    ));
}

#[test]
fn changing_a_buttons_radius_rebuilds_it() {
    assert!(rebuilds_between(
        |s| {
            s.leaf(Button::<()>::new("ok"));
        },
        |s| {
            s.leaf(Button::<()>::new("ok").radius(6.0));
        },
    ));
}

#[test]
fn the_inner_box_is_what_is_left_inside_the_border() {
    let style = BoxStyle::default().borders(
        Sides {
            top: 1.0,
            right: 2.0,
            bottom: 3.0,
            left: 4.0,
        },
        [0.0; 4],
    );
    let (origin, size) = style.inner_box([100.0, 100.0]);
    assert_eq!(origin, [4.0, 1.0]);
    assert_eq!(size, [94.0, 96.0]);
}

#[test]
fn a_border_wider_than_the_box_leaves_no_inner_area_rather_than_a_negative_one() {
    let style = BoxStyle::default().border(80.0, [0.0; 4]);
    let (_, size) = style.inner_box([100.0, 20.0]);
    assert_eq!(size, [0.0, 0.0]);
}

#[test]
fn the_zero_checks_recognise_an_undecorated_box() {
    assert!(Sides::default().is_zero());
    assert!(Corners::default().is_zero());
    assert!(!Sides::bottom(1.0).is_zero(), "one side still counts");
    assert!(!Corners::top(1.0).is_zero(), "one corner still counts");
    assert!(BoxStyle::default().radius.is_zero());
}
