//! Headless verification of the M7 animation state machine: spawn (enter
//! baked into `bundle()`), prune (deferred despawn via `ExitTransition`),
//! revival (reversal), `advance_tweens` interpolation/completion, and
//! `despawn_completed_exits` cleanup. Same style as `tests/reconcile.rs`.

use std::time::Duration;

use bevy_ecs::{entity::Entity, schedule::Schedule, world::World};

use matcha_ecs::{
    animation::{
        advance_tweens, Animated, Easing, ExitTransition, FrameTime, Opacity, Target, ToBeDespawn,
        Tween,
    },
    components::view::ViewChildren,
    view::{despawn_completed_exits, run_view, Scope},
};
use matcha_ecs_widgets::ColorRect;

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

#[test]
fn enter_fade_bakes_in_a_mismatched_animated_value_and_tween_at_spawn() {
    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        s.leaf(ColorRect::new(100.0, 50.0).enter_fade(Duration::from_millis(300), Easing::Linear));
    });
    let rect = children(&world, root)[0];

    assert_eq!(
        world.get::<Animated<Opacity>>(rect),
        Some(&Animated(Opacity(0.0)))
    );
    assert_eq!(world.get::<Target<Opacity>>(rect), Some(&Target(Opacity(1.0))));
    assert!(
        world.get::<Tween<Opacity>>(rect).is_some(),
        "enter_fade must bake in a tween immediately"
    );
}

#[test]
fn colorrect_without_fade_config_has_no_tween_or_exit_transition() {
    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        s.leaf(ColorRect::new(100.0, 50.0));
    });
    let rect = children(&world, root)[0];

    assert_eq!(
        world.get::<Animated<Opacity>>(rect),
        Some(&Animated(Opacity(1.0)))
    );
    assert!(world.get::<Tween<Opacity>>(rect).is_none());
    assert!(world.get::<ExitTransition<Opacity>>(rect).is_none());
}

#[test]
fn prune_without_exit_fade_despawns_immediately_unchanged() {
    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        s.leaf(ColorRect::new(100.0, 50.0));
    });
    let rect = children(&world, root)[0];

    run_view(&mut world, root, |_s: &mut Scope| {});

    assert!(
        world.get_entity(rect).is_err(),
        "no ExitTransition -> despawn synchronously, same as before M7"
    );
    assert!(children(&world, root).is_empty());
}

#[test]
fn prune_with_exit_fade_defers_despawn_and_keeps_the_slot() {
    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        s.leaf(
            ColorRect::new(100.0, 50.0)
                .key(1u64)
                .exit_fade(Duration::from_millis(300), Easing::Linear),
        );
    });
    let rect = children(&world, root)[0];

    run_view(&mut world, root, |_s: &mut Scope| {});

    assert!(
        world.get_entity(rect).is_ok(),
        "entity must survive while its exit fade is in flight"
    );
    assert!(world.get::<ToBeDespawn>(rect).is_some());
    assert!(world.get::<Tween<Opacity>>(rect).is_some());
    assert_eq!(
        children(&world, root),
        vec![rect],
        "slot must be kept alive in ViewChildren"
    );

    // A second prune pass (still not re-declared) must not re-trigger the
    // exit or disturb the slot.
    let tween_before = *world.get::<Tween<Opacity>>(rect).unwrap();
    run_view(&mut world, root, |_s: &mut Scope| {});
    assert_eq!(children(&world, root), vec![rect]);
    let tween_after = *world.get::<Tween<Opacity>>(rect).unwrap();
    assert_eq!(
        tween_before.start, tween_after.start,
        "already-exiting entity must not get a fresh tween every pass"
    );
}

#[test]
fn re_declaring_a_mid_exit_entity_reverses_the_fade() {
    let (mut world, root) = setup();
    let declare = |s: &mut Scope| {
        s.leaf(
            ColorRect::new(100.0, 50.0)
                .key(1u64)
                .exit_fade(Duration::from_millis(300), Easing::Linear),
        );
    };
    run_view(&mut world, root, declare);
    let rect = children(&world, root)[0];

    run_view(&mut world, root, |_s: &mut Scope| {}); // prune -> starts exiting
    assert!(world.get::<ToBeDespawn>(rect).is_some());

    run_view(&mut world, root, declare); // revive with the same key

    assert_eq!(
        children(&world, root),
        vec![rect],
        "revival reuses the same entity, not a rebuild"
    );
    assert!(
        world.get::<ToBeDespawn>(rect).is_none(),
        "revival clears ToBeDespawn"
    );
    assert_eq!(world.get::<Target<Opacity>>(rect), Some(&Target(Opacity(1.0))));
    assert!(
        world.get::<Tween<Opacity>>(rect).is_some(),
        "reversal tween started"
    );
}

#[test]
fn advance_tweens_interpolates_linearly_and_snaps_on_completion() {
    let mut world = World::new();
    let entity = world.spawn((Target(Opacity(1.0)), Animated(Opacity(0.0)))).id();

    let start = web_time::Instant::now();
    world.entity_mut(entity).insert(Tween::<Opacity> {
        from: Opacity(0.0),
        start,
        duration: Duration::from_secs(1),
        easing: Easing::Linear,
    });

    let mut schedule = Schedule::default();
    schedule.add_systems(advance_tweens::<Opacity>);

    // Halfway.
    world.insert_resource(FrameTime(start + Duration::from_millis(500)));
    schedule.run(&mut world);
    let mid = world.get::<Animated<Opacity>>(entity).unwrap().0.0;
    assert!((mid - 0.5).abs() < 0.01, "expected ~0.5, got {mid}");
    assert!(
        world.get::<Tween<Opacity>>(entity).is_some(),
        "still in flight at t=0.5"
    );

    // Completion.
    world.insert_resource(FrameTime(start + Duration::from_secs(2)));
    schedule.run(&mut world);
    assert_eq!(
        world.get::<Animated<Opacity>>(entity),
        Some(&Animated(Opacity(1.0)))
    );
    assert!(
        world.get::<Tween<Opacity>>(entity).is_none(),
        "tween removed on completion"
    );
}

#[test]
fn despawn_completed_exits_removes_the_entity_and_its_parent_slot() {
    let (mut world, root) = setup();
    run_view(&mut world, root, |s| {
        s.leaf(
            ColorRect::new(100.0, 50.0)
                .key(1u64)
                .exit_fade(Duration::from_millis(1), Easing::Linear),
        );
    });
    let rect = children(&world, root)[0];
    run_view(&mut world, root, |_s: &mut Scope| {}); // prune -> starts exiting

    let start = world.get::<Tween<Opacity>>(rect).unwrap().start;
    world.insert_resource(FrameTime(start + Duration::from_secs(1)));
    let mut schedule = Schedule::default();
    schedule.add_systems(advance_tweens::<Opacity>);
    schedule.run(&mut world);
    assert!(world.get::<Tween<Opacity>>(rect).is_none(), "tween finished");

    despawn_completed_exits(&mut world);

    assert!(
        world.get_entity(rect).is_err(),
        "entity despawned once its exit tween completes"
    );
    assert!(children(&world, root).is_empty(), "slot removed from parent");
}
