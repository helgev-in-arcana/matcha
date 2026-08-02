//! Headless verification of the opacity-fade animation plugin
//! (`matcha_ecs_widgets::animation`) and the core `ManualDespawn` contract it
//! is built on: spawn (enter baked into `bundle()`), prune (deferred despawn),
//! revival (reversal), tween interpolation/completion, and despawn-on-complete.
//!
//! Note the split of responsibilities the tests exercise: the *core* only flags
//! a pruned entity (`ManualDespawn::is_pruned`) and keeps its slot; it is the
//! plugin's systems, running in the schedule, that start the fade, advance it,
//! and eventually despawn the entity. So a prune alone does nothing visible
//! until the schedule runs. Same style as `tests/reconcile.rs`.

use std::time::Duration;

use bevy_ecs::{entity::Entity, schedule::Schedule, world::World};

use matcha_ecs::{
    components::{
        render::RenderOpacity,
        view::{ManualDespawn, ViewChildren},
    },
    resources::{FrameTime, RedrawRequest},
    view::{run_view, Scope},
};
use matcha_ecs_widgets::{
    animation::{self, ExitFade, OpacityTween},
    ColorRect, Easing,
};

fn setup() -> (World, Entity) {
    let mut world = World::new();
    world.insert_resource(FrameTime(web_time::Instant::now()));
    world.insert_resource(RedrawRequest::default());
    let root = world.spawn(ViewChildren::default()).id();
    (world, root)
}

/// The animation plugin's systems, as `UiEcs` would register them.
fn animation_schedule() -> Schedule {
    let mut schedule = Schedule::default();
    schedule.add_systems(animation::default_systems());
    schedule
}

/// Run the plugin's systems with the frame clock set to `now`, resetting the
/// redraw request first exactly as `UiEcs::advance_and_snapshot` does — so a
/// test can assert on whether *this* frame asked for a follow-up.
fn run_at(world: &mut World, schedule: &mut Schedule, now: web_time::Instant) {
    world.insert_resource(FrameTime(now));
    world.insert_resource(RedrawRequest::default());
    schedule.run(world);
}

fn children(world: &World, e: Entity) -> Vec<Entity> {
    world
        .get::<ViewChildren>(e)
        .map(|vc| vc.slots.iter().map(|(_, c)| *c).collect())
        .unwrap_or_default()
}

fn opacity(world: &World, e: Entity) -> f32 {
    world.get::<RenderOpacity>(e).unwrap().0
}

#[test]
fn enter_fade_bakes_in_a_transparent_start_and_a_tween_at_spawn() {
    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        s.leaf(ColorRect::new(100.0, 50.0).enter_fade(Duration::from_millis(300), Easing::Linear));
    });
    let rect = children(&world, root)[0];

    assert_eq!(world.get::<RenderOpacity>(rect), Some(&RenderOpacity(0.0)));
    let tween = world
        .get::<OpacityTween>(rect)
        .expect("enter_fade must bake in a tween immediately");
    assert_eq!((tween.from, tween.to), (0.0, 1.0));
}

#[test]
fn colorrect_without_fade_config_is_opaque_and_auto_despawned() {
    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        s.leaf(ColorRect::new(100.0, 50.0));
    });
    let rect = children(&world, root)[0];

    assert_eq!(world.get::<RenderOpacity>(rect), Some(&RenderOpacity(1.0)));
    assert!(world.get::<OpacityTween>(rect).is_none());
    assert!(world.get::<ExitFade>(rect).is_none());
    assert!(
        world.get::<ManualDespawn>(rect).is_none(),
        "a widget with no exit fade must not opt out of automatic despawn — \
         nothing would ever despawn it"
    );
}

#[test]
fn prune_without_exit_fade_despawns_immediately() {
    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        s.leaf(ColorRect::new(100.0, 50.0));
    });
    let rect = children(&world, root)[0];

    run_view(&mut world, root, |_s: &mut Scope| {});

    assert!(
        world.get_entity(rect).is_err(),
        "no ManualDespawn -> despawn synchronously in the prune pass"
    );
    assert!(children(&world, root).is_empty());
}

#[test]
fn prune_with_exit_fade_defers_the_despawn_and_keeps_the_slot() {
    let (mut world, root) = setup();
    let mut schedule = animation_schedule();
    run_view(&mut world, root, |s| {
        s.leaf(
            ColorRect::new(100.0, 50.0)
                .key(1u64)
                .exit_fade(Duration::from_millis(300), Easing::Linear),
        );
    });
    let rect = children(&world, root)[0];
    let start = web_time::Instant::now();

    run_view(&mut world, root, |_s: &mut Scope| {});

    // The core's half of the contract: flagged, kept alive, slot retained.
    assert!(world.get_entity(rect).is_ok());
    assert!(world.get::<ManualDespawn>(rect).unwrap().is_pruned());
    assert_eq!(
        children(&world, root),
        vec![rect],
        "slot must be kept alive in ViewChildren"
    );

    // The plugin's half: the fade only actually starts once the schedule runs.
    assert!(world.get::<OpacityTween>(rect).is_none());
    run_at(&mut world, &mut schedule, start);
    let tween = world
        .get::<OpacityTween>(rect)
        .expect("exit fade started by the plugin");
    assert_eq!(tween.to, 0.0);

    // A second prune pass (still not re-declared) must not restart the fade.
    let start_before = tween.start;
    run_view(&mut world, root, |_s: &mut Scope| {});
    run_at(&mut world, &mut schedule, start + Duration::from_millis(50));
    assert_eq!(children(&world, root), vec![rect]);
    assert_eq!(
        world.get::<OpacityTween>(rect).unwrap().start,
        start_before,
        "an already-pruned entity must not get a fresh tween every pass"
    );
}

#[test]
fn re_declaring_a_mid_exit_entity_reverses_the_fade_and_clears_the_pruned_flag() {
    let (mut world, root) = setup();
    let mut schedule = animation_schedule();
    let declare = |s: &mut Scope| {
        s.leaf(
            ColorRect::new(100.0, 50.0)
                .key(1u64)
                .exit_fade(Duration::from_millis(300), Easing::Linear),
        );
    };
    run_view(&mut world, root, declare);
    let rect = children(&world, root)[0];
    let start = web_time::Instant::now();

    run_view(&mut world, root, |_s: &mut Scope| {}); // prune
    run_at(&mut world, &mut schedule, start); // fade begins
    run_at(&mut world, &mut schedule, start + Duration::from_millis(150)); // halfway out
    let mid = opacity(&world, rect);
    assert!((mid - 0.5).abs() < 0.05, "expected ~0.5 mid-exit, got {mid}");

    run_view(&mut world, root, declare); // revive with the same key

    assert_eq!(
        children(&world, root),
        vec![rect],
        "revival reuses the same entity, not a rebuild"
    );
    assert!(
        !world.get::<ManualDespawn>(rect).unwrap().is_pruned(),
        "revival clears the pruned flag"
    );
    let tween = world
        .get::<OpacityTween>(rect)
        .expect("reversal tween started by patch");
    assert_eq!(tween.to, 1.0, "reversal heads back to fully visible");
    assert!(
        (tween.from - mid).abs() < 0.05,
        "reversal starts from the current, mid-fade value"
    );
}

#[test]
fn a_revived_entity_is_not_despawned_when_its_tween_completes() {
    let (mut world, root) = setup();
    let mut schedule = animation_schedule();
    let declare = |s: &mut Scope| {
        s.leaf(
            ColorRect::new(100.0, 50.0)
                .key(1u64)
                .exit_fade(Duration::from_millis(100), Easing::Linear),
        );
    };
    run_view(&mut world, root, declare);
    let rect = children(&world, root)[0];
    let start = web_time::Instant::now();

    run_view(&mut world, root, |_s: &mut Scope| {}); // prune
    run_at(&mut world, &mut schedule, start); // exit fade begins
    run_view(&mut world, root, declare); // revived before it finishes

    // Long past any tween's completion: the reversal completes, but the entity
    // must survive, because the pruned flag was cleared.
    run_at(&mut world, &mut schedule, start + Duration::from_secs(5));

    assert!(
        world.get_entity(rect).is_ok(),
        "a revived entity must not be despawned by its completing tween"
    );
    assert_eq!(opacity(&world, rect), 1.0);
    assert!(world.get::<OpacityTween>(rect).is_none());
}

#[test]
fn advance_opacity_tweens_interpolates_linearly_and_snaps_on_completion() {
    let (mut world, _root) = setup();
    let mut schedule = animation_schedule();
    let start = web_time::Instant::now();
    let entity = world
        .spawn((
            RenderOpacity(0.0),
            OpacityTween {
                from: 0.0,
                to: 1.0,
                start,
                duration: Duration::from_secs(1),
                easing: Easing::Linear,
            },
        ))
        .id();

    run_at(&mut world, &mut schedule, start + Duration::from_millis(500));
    let mid = opacity(&world, entity);
    assert!((mid - 0.5).abs() < 0.01, "expected ~0.5, got {mid}");
    assert!(world.get::<OpacityTween>(entity).is_some(), "still in flight");
    assert!(
        world.resource::<RedrawRequest>().is_requested(),
        "an in-flight tween must keep asking for frames"
    );

    run_at(&mut world, &mut schedule, start + Duration::from_secs(2));
    assert_eq!(world.get::<RenderOpacity>(entity), Some(&RenderOpacity(1.0)));
    assert!(
        world.get::<OpacityTween>(entity).is_none(),
        "tween removed on completion"
    );
    assert!(
        !world.resource::<RedrawRequest>().is_requested(),
        "a completed tween must not keep the loop spinning"
    );
}

#[test]
fn a_completed_exit_fade_despawns_the_entity_and_its_parent_slot() {
    let (mut world, root) = setup();
    let mut schedule = animation_schedule();
    run_view(&mut world, root, |s| {
        s.leaf(
            ColorRect::new(100.0, 50.0)
                .key(1u64)
                .exit_fade(Duration::from_millis(100), Easing::Linear),
        );
    });
    let rect = children(&world, root)[0];
    let start = web_time::Instant::now();

    run_view(&mut world, root, |_s: &mut Scope| {}); // prune
    run_at(&mut world, &mut schedule, start); // fade begins
    assert!(world.get_entity(rect).is_ok(), "still fading");

    run_at(&mut world, &mut schedule, start + Duration::from_secs(1)); // fade completes

    assert!(
        world.get_entity(rect).is_err(),
        "entity despawned once its exit fade completes"
    );
    assert!(children(&world, root).is_empty(), "slot removed from parent");
}
