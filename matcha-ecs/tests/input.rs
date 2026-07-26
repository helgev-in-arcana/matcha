//! Headless verification of picking and click resolution: build a view, run
//! layout, then call `RectZPicker::build`/`resolve_click_at` directly (no
//! window/GPU) and assert rects, overlap resolution, bubbling, and that the
//! resolved entity's `OnClick<Msg>` carries the expected message. Same style
//! as `tests/layout.rs`/`tests/extract.rs`.

use bevy_ecs::{
    bundle::Bundle,
    entity::Entity,
    world::{EntityWorldMut, World},
};

use matcha_ecs::{
    components::{
        input::{OnClick, Pickable},
        view::ViewChildren,
    },
    input::resolve_click_at,
    layout::{layout_root, Constraints},
    pick::{PickQuery, Picker, RectZPicker},
    view::{run_view, Widget},
};
use matcha_ecs_widgets::{Button, ColorRect, Column, Container, Row};

#[derive(Clone, Copy, PartialEq, Debug)]
enum Msg {
    Inc,
    Dec,
}

fn children(world: &World, e: Entity) -> Vec<Entity> {
    world
        .get::<ViewChildren>(e)
        .map(|vc| vc.slots.iter().map(|(_, c)| *c).collect())
        .unwrap_or_default()
}

fn click_at(world: &World, picker: &RectZPicker, pos: [f32; 2]) -> Option<Entity> {
    resolve_click_at::<Msg>(world, picker, &PickQuery { viewport_pos: pos })
}

/// `Row > (Button("-").on(Dec), Button("+").on(Inc))`: two side-by-side
/// buttons, each 120x40 by default, laid out with a 10px gap.
#[test]
fn picking_finds_the_button_under_the_cursor() {
    let mut world = World::new();
    let root = world.spawn(ViewChildren::default()).id();
    run_view(&mut world, root, |s| {
        s.node(Row::new().gap(10.0), |s| {
            s.leaf(Button::new("-").on(Msg::Dec));
            s.leaf(Button::new("+").on(Msg::Inc));
        });
    });
    layout_root(&mut world, root, Constraints::from_max_size([800.0, 600.0]));

    let row = children(&world, root)[0];
    let [minus, plus]: [Entity; 2] = children(&world, row).try_into().unwrap();

    let picker = RectZPicker::build(&world, root);

    // Inside "-" (origin [0,0], size 120x40).
    let hit = click_at(&world, &picker, [10.0, 10.0]);
    assert_eq!(hit, Some(minus));
    assert_eq!(
        world.get::<OnClick<Msg>>(minus),
        Some(&OnClick(Some(Msg::Dec)))
    );

    // Inside "+" (origin [130,0] = 120 width + 10 gap).
    let hit = click_at(&world, &picker, [140.0, 10.0]);
    assert_eq!(hit, Some(plus));
    assert_eq!(
        world.get::<OnClick<Msg>>(plus),
        Some(&OnClick(Some(Msg::Inc)))
    );

    // Between the two buttons (in the gap) and outside both entirely: no hit.
    assert_eq!(click_at(&world, &picker, [125.0, 10.0]), None);
    assert_eq!(click_at(&world, &picker, [10.0, 1000.0]), None);
}

/// Two top-level `Button`s (not wrapped in a container) are each arranged at
/// window origin `[0,0]` independently (`layout_root`'s documented top-level
/// behaviour), so they exactly overlap. With no explicit `ZOrder` (both
/// default to `0`), resolution must fall back to paint order: the
/// later-declared (later-painted) button wins.
#[test]
fn overlapping_top_level_buttons_resolve_to_the_later_painted_one() {
    let mut world = World::new();
    let root = world.spawn(ViewChildren::default()).id();
    run_view(&mut world, root, |s| {
        s.leaf(Button::new("back").on(Msg::Dec));
        s.leaf(Button::new("front").on(Msg::Inc));
    });
    layout_root(&mut world, root, Constraints::from_max_size([800.0, 600.0]));

    let [back, front]: [Entity; 2] = children(&world, root).try_into().unwrap();
    let picker = RectZPicker::build(&world, root);

    let hit = click_at(&world, &picker, [10.0, 10.0]);
    assert_eq!(
        hit,
        Some(front),
        "later-painted sibling wins on an exact overlap"
    );
    assert_ne!(hit, Some(back));
}

/// `Column > Button` nests the button inside a container that itself has no
/// `OnClick`; picking must still resolve the click straight to the button
/// (containers without `Pickable` are simply transparent to picking).
#[test]
fn nested_button_inside_a_plain_container_is_still_pickable() {
    let mut world = World::new();
    let root = world.spawn(ViewChildren::default()).id();
    run_view(&mut world, root, |s| {
        s.node(Column::new(), |s| {
            s.leaf(Button::new("ok").on(Msg::Inc));
        });
    });
    layout_root(&mut world, root, Constraints::from_max_size([800.0, 600.0]));

    let column = children(&world, root)[0];
    let button = children(&world, column)[0];

    let picker = RectZPicker::build(&world, root);
    assert_eq!(click_at(&world, &picker, [10.0, 10.0]), Some(button));
}

// ---------------------------------------------------------------------------
// Bubbling semantics (replaces the old fall-through model)
// ---------------------------------------------------------------------------

/// A `Pickable` leaf with no `OnClick` of its own, used to prove that a pick
/// landing on it resolves by walking *up* to an ancestor handler.
///
/// Composes its inner widget by value (rather than building it inline) for the
/// same reason `Link` does: `bundle()`'s RPITIT return borrows `&self`, so the
/// inner widget has to outlive the call.
struct PickableChild {
    inner: ColorRect,
}

impl PickableChild {
    fn new() -> Self {
        Self {
            inner: ColorRect::new(60.0, 20.0),
        }
    }
}

impl Widget for PickableChild {
    fn bundle(&self) -> impl Bundle {
        (self.inner.bundle(), Pickable)
    }
    fn patch(&self, _entity: &mut EntityWorldMut) {}
}

/// A `Pickable` container that carries the `OnClick`, wrapping a `Pickable`
/// child that does not. Clicking the child must bubble to the container.
struct ClickableParent {
    inner: Container,
    msg: Msg,
}

impl ClickableParent {
    fn new(msg: Msg) -> Self {
        Self {
            inner: Container::new(),
            msg,
        }
    }
}

impl Widget for ClickableParent {
    fn bundle(&self) -> impl Bundle {
        (self.inner.bundle(), OnClick(Some(self.msg)), Pickable)
    }
    fn patch(&self, _entity: &mut EntityWorldMut) {}
}

/// The pick lands on the inner `Pickable` leaf, which has no `OnClick`.
/// Resolution walks up `ChildOf` and finds the parent's handler. This is the
/// case an ID-buffer backend must be able to serve, since it can only ever
/// report the frontmost surface.
#[test]
fn a_pick_on_a_handlerless_child_bubbles_to_the_nearest_ancestor_handler() {
    let mut world = World::new();
    let root = world.spawn(ViewChildren::default()).id();
    run_view(&mut world, root, |s| {
        s.node(ClickableParent::new(Msg::Inc), |s| {
            s.leaf(PickableChild::new());
        });
    });
    layout_root(&mut world, root, Constraints::from_max_size([800.0, 600.0]));

    let parent = children(&world, root)[0];
    let child = children(&world, parent)[0];

    let picker = RectZPicker::build(&world, root);

    // Picking itself lands on the child (later-painted, on top of the parent).
    let picked = picker
        .pick(
            &world,
            &PickQuery {
                viewport_pos: [10.0, 10.0],
            },
        )
        .map(|h| h.entity);
    assert_eq!(picked, Some(child), "the raw pick is the frontmost surface");

    assert_eq!(
        click_at(&world, &picker, [10.0, 10.0]),
        Some(parent),
        "the click resolves to the ancestor that handles it, not the picked leaf"
    );
    assert!(world.get::<OnClick<Msg>>(child).is_none());
}

/// A `Pickable` element with no handler, painted *over* a separate `Button`,
/// must **not** let the click fall through to the button underneath: picking
/// returns exactly one entity, and resolution only ever goes up from there.
/// (Under the previous fall-through model this returned the button.)
#[test]
fn a_pickable_overlay_without_a_handler_does_not_fall_through_to_what_is_behind() {
    let mut world = World::new();
    let root = world.spawn(ViewChildren::default()).id();
    run_view(&mut world, root, |s| {
        // Both are top-level, so both are arranged at window origin [0,0].
        s.leaf(Button::new("under").on(Msg::Inc));
        s.leaf(PickableChild::new());
    });
    layout_root(&mut world, root, Constraints::from_max_size([800.0, 600.0]));

    let [button, overlay]: [Entity; 2] = children(&world, root).try_into().unwrap();
    let picker = RectZPicker::build(&world, root);

    // Inside the overlay (60x20) and therefore also inside the button (120x40).
    assert_eq!(
        click_at(&world, &picker, [10.0, 10.0]),
        None,
        "the overlay occludes the button; nothing above it handles clicks"
    );
    assert!(world.get::<OnClick<Msg>>(overlay).is_none());

    // Outside the overlay but still inside the button: the button is picked.
    assert_eq!(click_at(&world, &picker, [80.0, 30.0]), Some(button));
}

/// Applying the resolved click's message to the model via a plain reducer
/// function — the same shape `UiEcs::dispatch_click` uses internally —
/// changes the model as expected. Closes the loop: pick -> bubble -> message
/// -> reducer, without needing a GPU-backed `UiEcs` in a headless test.
#[test]
fn resolved_message_applied_through_a_reducer_updates_the_model() {
    let mut world = World::new();
    let root = world.spawn(ViewChildren::default()).id();
    run_view(&mut world, root, |s| {
        s.leaf(Button::new("+").on(Msg::Inc));
    });
    layout_root(&mut world, root, Constraints::from_max_size([800.0, 600.0]));

    let picker = RectZPicker::build(&world, root);
    let entity = click_at(&world, &picker, [10.0, 10.0]).expect("the button is under the cursor");
    let msg = world
        .get::<OnClick<Msg>>(entity)
        .and_then(|on_click| on_click.0)
        .expect("button has an assigned message");

    struct Model {
        count: i32,
    }
    fn reduce(model: &mut Model, msg: Msg) {
        match msg {
            Msg::Inc => model.count += 1,
            Msg::Dec => model.count -= 1,
        }
    }

    let mut model = Model { count: 0 };
    reduce(&mut model, msg);
    assert_eq!(model.count, 1);
}
