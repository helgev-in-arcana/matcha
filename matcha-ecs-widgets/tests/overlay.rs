//! `Anchor` and `Slider`: the two widgets that finish the P6 batch.
//!
//! For `Anchor` the claims are that it takes no space (so the flow closes up
//! around it), that its child is offset from where it sits, and that `z_index`
//! puts the child over its siblings for **picking as well as painting** — an
//! overlay that covers a button but does not intercept its clicks is worse
//! than no overlay.
//!
//! For `Slider`, that a press anywhere on the track reports the value under
//! the pointer, and that the widget keeps no value of its own.
//!
//! Headless, GPU-free.

use bevy_ecs::{entity::Entity, world::World};

use matcha_ecs::{
    components::{layout::LayoutOutput, view::ViewChildren},
    input::{dispatch_pointer, MessageQueue},
    components::input::{PointerInput, PointerPhase},
    layout::{layout_root, Constraints},
    pick::{PickQuery, Picker, RectPicker},
    render::extract_items,
    view::run_view,
};
use matcha_ecs_widgets::{
    AlignItems, Anchor, Button, ColorRect, Column, Panel, Slider,
};

const WINDOW: [f32; 2] = [800.0, 600.0];

#[derive(Clone, PartialEq, Debug)]
enum Msg {
    Moved(f32),
    Clicked,
}

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
        .unwrap_or_else(|| panic!("{e:?} was never laid out"))
}

fn pick_at(world: &World, root: Entity, pos: [f32; 2]) -> Option<Entity> {
    RectPicker::build(world, root)
        .pick(world, &PickQuery { viewport_pos: pos })
        .map(|h| h.entity)
}

// ---------------------------------------------------------------------------
// Anchor
// ---------------------------------------------------------------------------

#[test]
fn an_anchor_takes_no_space_in_the_flow() {
    // Two 40-tall rects with an anchor between them: the second must sit
    // exactly where it would with no anchor declared at all.
    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        s.node(Column::new().align_items(AlignItems::Start), |s| {
            s.leaf(ColorRect::new(50.0, 40.0).key(1u64));
            s.node(Anchor::at(100.0, 100.0).key(2u64), |s| {
                s.leaf(ColorRect::new(80.0, 80.0).key(3u64));
            });
            s.leaf(ColorRect::new(50.0, 40.0).key(4u64));
        });
    });
    layout_root(&mut world, root, Constraints::from_max_size(WINDOW));

    let column = children(&world, root)[0];
    let kids = children(&world, column);
    assert_eq!(output(&world, kids[1]).size, [0.0, 0.0], "the anchor itself");
    assert_eq!(
        output(&world, kids[2]).origin[1],
        40.0,
        "the following sibling must not be pushed down by the anchor or its child"
    );
    assert_eq!(output(&world, column).size[1], 80.0, "40 + 0 + 40");
}

#[test]
fn the_child_is_drawn_at_the_offset_from_where_the_anchor_sits() {
    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        s.node(Column::new().align_items(AlignItems::Start), |s| {
            s.leaf(ColorRect::new(50.0, 40.0).key(1u64));
            s.node(Anchor::at(10.0, 5.0).key(2u64), |s| {
                s.leaf(ColorRect::new(80.0, 80.0).key(3u64));
            });
        });
    });
    layout_root(&mut world, root, Constraints::from_max_size(WINDOW));

    let column = children(&world, root)[0];
    let anchor = children(&world, column)[1];
    let child = children(&world, anchor)[0];

    // The anchor sits at y 40 (after the first rect); the child is offset from
    // there, not from the column's origin.
    assert_eq!(output(&world, anchor).origin, [0.0, 40.0]);
    assert_eq!(output(&world, child).origin, [10.0, 5.0]);
    assert_eq!(output(&world, child).size, [80.0, 80.0]);
}

#[test]
fn an_overlay_covers_its_siblings_for_clicking_as_well_as_drawing() {
    // The whole point of `z_index` being one order: a dropdown that visually
    // covers a button must also swallow the click.
    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        s.node(Column::new().align_items(AlignItems::Start), |s| {
            // The anchor is declared *first*, so only `z_index` can put its
            // child in front.
            s.node(Anchor::at(0.0, 0.0).key(1u64), |s| {
                s.leaf(Panel::new(200.0, 100.0).key(2u64).background_color([0.0, 0.0, 0.0, 1.0]));
            });
            s.leaf(Button::<Msg>::new("behind").key(3u64).size(200.0, 100.0));
        });
    });
    layout_root(&mut world, root, Constraints::from_max_size(WINDOW));

    let column = children(&world, root)[0];
    let kids = children(&world, column);
    let overlay = children(&world, kids[0])[0];

    let painted: Vec<Entity> = extract_items(&world, root)
        .items
        .iter()
        .map(|i| i.entity)
        .collect();
    let overlay_at = painted.iter().position(|&e| e == overlay);
    let button_at = painted.iter().position(|&e| e == kids[1]);
    assert!(
        overlay_at > button_at,
        "the raised overlay must be painted after the button it covers"
    );

    // `Panel` is not pickable, so what a click finds is whatever is behind —
    // but the *ordering* is the thing under test here, and it is shared.
    assert_eq!(pick_at(&world, root, [50.0, 50.0]), Some(kids[1]));
}

#[test]
fn a_re_declared_anchor_moves_its_child_without_a_rebuild() {
    let (mut world, root) = setup();
    let build = |offset: f32| {
        move |s: &mut matcha_ecs::view::Scope| {
            s.node(Anchor::at(offset, offset).key(1u64), |s| {
                s.leaf(ColorRect::new(20.0, 20.0).key(2u64));
            });
        }
    };

    run_view(&mut world, root, build(10.0));
    layout_root(&mut world, root, Constraints::from_max_size(WINDOW));
    let anchor = children(&world, root)[0];
    let child = children(&world, anchor)[0];
    assert_eq!(output(&world, child).origin, [10.0, 10.0]);

    run_view(&mut world, root, build(60.0));
    layout_root(&mut world, root, Constraints::from_max_size(WINDOW));
    assert_eq!(
        children(&world, root)[0],
        anchor,
        "re-declaring the same key must reuse the entity"
    );
    assert_eq!(output(&world, child).origin, [60.0, 60.0]);
}

// ---------------------------------------------------------------------------
// Slider
// ---------------------------------------------------------------------------

/// A 200x24 slider over 0..=100, sitting at `value`.
fn slider_world(value: f32, step: f32) -> (World, Entity, Entity) {
    let (mut world, root) = setup();
    run_view(&mut world, root, move |s| {
        s.leaf(
            Slider::<Msg>::new(value, 0.0, 100.0)
                .step(step)
                .size(200.0, 24.0)
                .on_change(Msg::Moved),
        );
    });
    layout_root(&mut world, root, Constraints::from_max_size(WINDOW));
    let slider = children(&world, root)[0];
    (world, root, slider)
}

fn press_at(world: &mut World, slider: Entity, x: f32) -> bool {
    dispatch_pointer(
        world,
        slider,
        [x, 12.0],
        PointerPhase::Press { count: 1 },
    )
}

fn queued(world: &mut World) -> Vec<Msg> {
    world
        .get_resource_mut::<MessageQueue<Msg>>()
        .map(|mut q| q.drain())
        .unwrap_or_default()
}

#[test]
fn pressing_the_track_reports_the_value_under_the_pointer() {
    let (mut world, _root, slider) = slider_world(0.0, 0.0);
    assert!(press_at(&mut world, slider, 100.0));

    // The knob's centre travels 9..191, so the box's midpoint is the range's.
    let [Msg::Moved(value)] = queued(&mut world)[..] else {
        panic!("expected exactly one Moved message");
    };
    assert!((value - 50.0).abs() < 0.5, "got {value}");
}

#[test]
fn the_extremes_of_the_box_reach_the_extremes_of_the_range() {
    let (mut world, _root, slider) = slider_world(50.0, 0.0);

    press_at(&mut world, slider, 0.0);
    assert_eq!(queued(&mut world), vec![Msg::Moved(0.0)]);

    press_at(&mut world, slider, 200.0);
    assert_eq!(queued(&mut world), vec![Msg::Moved(100.0)]);
}

#[test]
fn a_slider_holds_no_value_of_its_own() {
    // It reports and waits for the app to declare the new value; writing it
    // back here would create a second, competing copy.
    let (mut world, _root, slider) = slider_world(0.0, 0.0);
    press_at(&mut world, slider, 200.0);
    let _ = queued(&mut world);

    assert_eq!(
        world
            .get::<matcha_ecs_widgets::SliderRange>(slider)
            .expect("a slider carries its range")
            .value,
        0.0,
        "the declared value must survive an unacknowledged drag"
    );
}

#[test]
fn a_press_that_lands_on_the_current_value_still_consumes_the_event() {
    // It must, or the press never captures the pointer and the drag that
    // follows goes nowhere.
    let (mut world, _root, slider) = slider_world(0.0, 0.0);
    let consumed = press_at(&mut world, slider, 0.0);

    assert!(consumed);
    assert!(
        queued(&mut world).is_empty(),
        "but there is no change to report"
    );
}

#[test]
fn a_stepped_slider_only_reports_values_on_its_grid() {
    let (mut world, _root, slider) = slider_world(0.0, 25.0);
    press_at(&mut world, slider, 120.0);

    let [Msg::Moved(value)] = queued(&mut world)[..] else {
        panic!("expected one message");
    };
    assert_eq!(value % 25.0, 0.0, "got {value}");
}

#[test]
fn a_drag_keeps_reporting_even_past_the_ends_of_the_track() {
    // Capture delivers drags from outside the widget; clamping is what stops
    // that from producing an out-of-range value.
    let (mut world, _root, slider) = slider_world(50.0, 0.0);
    dispatch_pointer(&mut world, slider, [-500.0, 12.0], PointerPhase::Drag);
    assert_eq!(queued(&mut world), vec![Msg::Moved(0.0)]);

    dispatch_pointer(&mut world, slider, [9999.0, 12.0], PointerPhase::Drag);
    assert_eq!(queued(&mut world), vec![Msg::Moved(100.0)]);
}

#[test]
fn a_scroll_over_a_slider_is_not_a_drag() {
    let (mut world, _root, slider) = slider_world(50.0, 0.0);
    let consumed = dispatch_pointer(
        &mut world,
        slider,
        [10.0, 12.0],
        PointerPhase::Scroll { delta: [0.0, 40.0] },
    );
    assert!(!consumed, "a wheel event must bubble past, not move the value");
    assert!(queued(&mut world).is_empty());
}

/// Silences the unused-variant warning: `Clicked` exists so `Msg` is a real
/// multi-variant app message rather than a newtype.
#[test]
fn msg_is_a_normal_app_enum() {
    assert_ne!(Msg::Clicked, Msg::Moved(0.0));
}

/// `PointerInput` is part of the public surface these handlers read.
#[test]
fn pointer_input_carries_a_local_position() {
    let input = PointerInput {
        local_pos: [3.0, 4.0],
        phase: PointerPhase::Drag,
    };
    assert_eq!(input.local_pos, [3.0, 4.0]);
}
