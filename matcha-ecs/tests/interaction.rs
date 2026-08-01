//! The widget-side half of `:hover`/`:active`: the colour cell a builder reads
//! and the system that drives it.
//!
//! The cell is what makes a *transition* expressible at all — a builder is
//! captured at `bundle()` time and cannot read the world, so an interpolated
//! value has to arrive through shared storage. These tests drive the real
//! system against a real `PointerState`, and inspect the cell rather than any
//! pixels (GPU-free, per this suite's convention).

use std::time::Duration;

use bevy_ecs::{
    bundle::Bundle,
    entity::Entity,
    schedule::Schedule,
    world::{EntityWorldMut, World},
};

use matcha_ecs::{
    components::{input::Pickable, render::RenderItem, view::ViewChildren},
    layout::{layout_root, Constraints},
    pick::{PickerResource, RectPicker},
    pointer::set_position,
    resources::{FrameTime, RedrawRequest},
    view::{run_view, Widget},
};
use matcha_ecs_widgets::{
    interaction::{self, InteractionColorState, InteractionColors},
    Button, ColorRect, Column,
};

const WINDOW: [f32; 2] = [800.0, 600.0];
const BASE: [f32; 4] = [0.1, 0.1, 0.1, 1.0];
const HOVER: [f32; 4] = [0.9, 0.9, 0.9, 1.0];

/// A pickable leaf carrying interaction colours, so the system has something to
/// drive without dragging `Button`'s font machinery into the test.
struct Swatch {
    inner: ColorRect,
    colors: InteractionColors,
}

impl Widget for Swatch {
    fn bundle(&self) -> impl Bundle {
        (self.inner.bundle(), Pickable)
    }
    fn after_spawn(&self, entity: &mut EntityWorldMut) {
        // A wrapper that delegates `bundle()` must delegate `after_spawn()`
        // too — `ColorRect` builds its `RenderItem` there, since that is the
        // only place it can reach the `ShapeCtx` resource.
        Widget::after_spawn(&self.inner, entity);
        interaction::interaction_cell(entity, self.colors);
    }
    fn patch(&self, entity: &mut EntityWorldMut) {
        interaction::interaction_cell(entity, self.colors);
    }
}

struct Fixture {
    world: World,
    schedule: Schedule,
    swatch: Entity,
}

impl Fixture {
    fn new(colors: InteractionColors) -> Self {
        let mut world = World::new();
        let root = world.spawn(ViewChildren::default()).id();
        world.insert_resource(FrameTime(web_time::Instant::now()));
        world.insert_resource(RedrawRequest::default());

        run_view(&mut world, root, move |s| {
            s.node(Column::new(), |s| {
                s.leaf(Swatch {
                    inner: ColorRect::new(100.0, 50.0).key(1u64),
                    colors,
                });
            });
        });
        layout_root(&mut world, root, Constraints::from_max_size(WINDOW));
        world.insert_resource(PickerResource(Box::new(RectPicker::build(&world, root))));

        let column = children(&world, root)[0];
        let swatch = children(&world, column)[0];

        let mut schedule = Schedule::default();
        schedule.add_systems(interaction::default_systems());

        Self {
            world,
            schedule,
            swatch,
        }
    }

    /// Advance to `now` (relative to the fixture's start) and run one frame.
    fn tick(&mut self, elapsed: Duration) {
        let start = self.world.resource::<FrameTime>().0;
        self.world.insert_resource(FrameTime(start + elapsed));
        self.schedule.run(&mut self.world);
    }

    fn color(&self) -> [f32; 4] {
        self.world
            .get::<InteractionColorState>(self.swatch)
            .expect("the swatch carries interaction state")
            .color()
    }

    fn hover(&mut self, on: bool) {
        let pos = if on { Some([10.0, 10.0]) } else { None };
        set_position(&mut self.world, pos);
    }
}

fn children(world: &World, e: Entity) -> Vec<Entity> {
    world
        .get::<ViewChildren>(e)
        .map(|vc| vc.slots.iter().map(|(_, c)| *c).collect())
        .unwrap_or_default()
}

fn snapping() -> InteractionColors {
    InteractionColors {
        base: BASE,
        hover: Some(HOVER),
        active: None,
        transition: None,
    }
}

fn easing_over(ms: u64) -> InteractionColors {
    InteractionColors {
        transition: Some((
            Duration::from_millis(ms),
            matcha_ecs_widgets::Easing::Linear,
        )),
        ..snapping()
    }
}

#[test]
fn a_widget_starts_at_its_base_colour() {
    let f = Fixture::new(snapping());
    assert_eq!(f.color(), BASE);
}

#[test]
fn without_a_transition_hovering_snaps_in_one_frame() {
    let mut f = Fixture::new(snapping());
    f.hover(true);
    f.tick(Duration::ZERO);
    assert_eq!(f.color(), HOVER);
}

#[test]
fn a_transition_eases_through_the_midpoint_rather_than_jumping() {
    let mut f = Fixture::new(easing_over(100));
    f.hover(true);
    f.tick(Duration::ZERO); // starts the transition at t = 0

    f.tick(Duration::from_millis(50));
    let mid = f.color()[0];
    assert!(
        mid > BASE[0] + 0.05 && mid < HOVER[0] - 0.05,
        "halfway through a linear transition the colour should be between the two, got {mid}"
    );

    f.tick(Duration::from_millis(100));
    assert_eq!(f.color(), HOVER, "and it lands exactly on the target");
}

#[test]
fn an_in_flight_transition_keeps_asking_for_frames_and_stops_when_it_lands() {
    let mut f = Fixture::new(easing_over(100));
    f.hover(true);
    f.tick(Duration::ZERO);

    f.world.insert_resource(RedrawRequest::default());
    f.tick(Duration::from_millis(50));
    assert!(
        f.world.resource::<RedrawRequest>().is_requested(),
        "a fade that has not finished must keep the event loop coming back"
    );

    f.world.insert_resource(RedrawRequest::default());
    f.tick(Duration::from_millis(200));
    assert!(
        !f.world.resource::<RedrawRequest>().is_requested(),
        "once it has landed there is nothing left to animate"
    );
}

#[test]
fn leaving_mid_transition_reverses_from_where_it_had_reached() {
    let mut f = Fixture::new(easing_over(100));
    f.hover(true);
    f.tick(Duration::ZERO);
    f.tick(Duration::from_millis(50));
    let interrupted = f.color()[0];

    f.hover(false);
    f.tick(Duration::from_millis(51));
    let after = f.color()[0];
    assert!(
        (after - interrupted).abs() < 0.05,
        "the reversal must start from the interrupted colour ({interrupted}), not from the target; got {after}"
    );

    f.tick(Duration::from_millis(300));
    assert_eq!(f.color(), BASE);
}

#[test]
fn a_colour_step_invalidates_the_cached_render_node() {
    // Colour is baked into the atlas by the builder, so unlike opacity a change
    // has to force a rebuild.
    let mut f = Fixture::new(snapping());
    let before = f
        .world
        .get::<RenderItem>(f.swatch)
        .expect("swatch draws")
        .cache
        .clone();

    f.hover(true);
    f.tick(Duration::ZERO);

    let after = f.world.get::<RenderItem>(f.swatch).unwrap().cache.clone();
    assert!(
        !std::sync::Arc::ptr_eq(&before, &after),
        "the cached node must be dropped when the colour moves"
    );
}

#[test]
fn a_re_declared_widget_keeps_its_cell_so_a_transition_is_not_restarted() {
    // `patch` runs on every view pass; if it replaced the cell, any transition
    // would snap back to the base colour whenever the model changed.
    let mut f = Fixture::new(easing_over(100));
    f.hover(true);
    f.tick(Duration::ZERO);
    f.tick(Duration::from_millis(50));
    let mid = f.color();

    let entity = f.swatch;
    let mut e = f.world.entity_mut(entity);
    Swatch {
        inner: ColorRect::new(100.0, 50.0).key(1u64),
        colors: easing_over(100),
    }
    .patch(&mut e);

    assert_eq!(f.color(), mid, "re-declaring must not reset the live colour");
}

#[test]
fn a_button_declares_its_state_colours_onto_its_entity() {
    // End-to-end through the real widget: the builders have to reach the
    // component the system drives, not just a field on the view struct.
    let mut world = World::new();
    let root = world.spawn(ViewChildren::default()).id();
    run_view(&mut world, root, |s| {
        s.leaf(
            Button::<()>::new("ok")
                .color(BASE)
                .hover_color(HOVER)
                .active_color([0.5, 0.5, 0.5, 1.0])
                .transition(Duration::from_millis(10), matcha_ecs_widgets::Easing::Linear),
        );
    });

    let button = children(&world, root)[0];
    let colors = world
        .get::<InteractionColors>(button)
        .copied()
        .expect("a Button declares interaction colours");

    assert_eq!(colors.base, BASE);
    assert_eq!(colors.target(true, false), HOVER);
    assert_eq!(colors.target(true, true), [0.5, 0.5, 0.5, 1.0]);
    assert!(colors.transition.is_some());
}
