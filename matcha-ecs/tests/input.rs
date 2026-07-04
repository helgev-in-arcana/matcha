//! Headless verification of the M5 hit-test cache and click resolution:
//! build a view, run layout, then call `build_hit_test_cache`/
//! `resolve_click_target` directly (no window/GPU) and assert rects, overlap
//! resolution, and that the resolved entity's `OnClick<Msg>` carries the
//! expected message. Same style as `tests/layout.rs`/`tests/extract.rs`.

use bevy_ecs::{entity::Entity, world::World};

use matcha_ecs::{
    components::{input::OnClick, view::ViewChildren},
    input::{build_hit_test_cache, resolve_click_target},
    layout::{layout_root, Constraints},
    view::run_view,
};
use matcha_ecs_widgets::{Button, Column, Row};

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

/// `Row > (Button("-").on(Dec), Button("+").on(Inc))`: two side-by-side
/// buttons, each 120x40 by default, laid out with a 10px gap.
#[test]
fn hit_test_cache_finds_the_button_under_the_cursor() {
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

    let cache = build_hit_test_cache(&world, root);

    // Inside "-" (origin [0,0], size 120x40).
    let hit = resolve_click_target::<Msg>(&world, &cache, [10.0, 10.0]);
    assert_eq!(hit, Some(minus));
    assert_eq!(world.get::<OnClick<Msg>>(minus), Some(&OnClick(Some(Msg::Dec))));

    // Inside "+" (origin [130,0] = 120 width + 10 gap).
    let hit = resolve_click_target::<Msg>(&world, &cache, [140.0, 10.0]);
    assert_eq!(hit, Some(plus));
    assert_eq!(world.get::<OnClick<Msg>>(plus), Some(&OnClick(Some(Msg::Inc))));

    // Between the two buttons (in the gap) and outside both entirely: no hit.
    assert_eq!(resolve_click_target::<Msg>(&world, &cache, [125.0, 10.0]), None);
    assert_eq!(resolve_click_target::<Msg>(&world, &cache, [10.0, 1000.0]), None);
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
    let cache = build_hit_test_cache(&world, root);

    let hit = resolve_click_target::<Msg>(&world, &cache, [10.0, 10.0]);
    assert_eq!(hit, Some(front), "later-painted sibling wins on an exact overlap");
    assert_ne!(hit, Some(back));
}

/// `Column > Button` nests the button inside a container that itself has no
/// `OnClick`; the cache must still resolve the click straight to the button
/// (containers without `HitTestEnabled` are simply absent from the cache).
#[test]
fn nested_button_inside_a_plain_container_is_still_hit_testable() {
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

    let cache = build_hit_test_cache(&world, root);
    let hit = resolve_click_target::<Msg>(&world, &cache, [10.0, 10.0]);
    assert_eq!(hit, Some(button));
}

/// Applying the resolved click's message to the model via a plain reducer
/// function — the same shape `UiEcs::dispatch_click` uses internally —
/// changes the model as expected. Closes the loop: hit-test -> message ->
/// reducer, without needing a GPU-backed `UiEcs` in a headless test.
#[test]
fn resolved_message_applied_through_a_reducer_updates_the_model() {
    let mut world = World::new();
    let root = world.spawn(ViewChildren::default()).id();
    run_view(&mut world, root, |s| {
        s.leaf(Button::new("+").on(Msg::Inc));
    });
    layout_root(&mut world, root, Constraints::from_max_size([800.0, 600.0]));

    let cache = build_hit_test_cache(&world, root);
    let entity = resolve_click_target::<Msg>(&world, &cache, [10.0, 10.0])
        .expect("the button is under the cursor");
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
