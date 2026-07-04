//! Headless verification of M8's `TaskHandle`/`spawn_task`: dedup by key,
//! key-replacement updates the tracked key, entity despawn removes the
//! component, and a spawned future actually runs to completion on the real
//! `AsyncComputeTaskPool`. No GPU/window needed.
//!
//! Deliberately **not** tested here: how promptly a cancelled task's captured
//! resources get reclaimed. A standalone experiment (see `task.rs`'s module
//! docs) found this is not prompt in general — it depends on the future
//! being polled again after cancellation, which a future with no self-driven
//! wake source may never get. That's a property of `bevy_tasks`, not
//! something `spawn_task` can control, so it isn't asserted as a timing
//! guarantee here.

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use bevy_ecs::world::World;
use bevy_tasks::{AsyncComputeTaskPool, TaskPoolBuilder};

use matcha_ecs::task::{spawn_task, TaskHandle};

/// `AsyncComputeTaskPool` is a first-wins global static; safe to call from
/// every test in this file (only the first call actually initialises it).
fn ensure_pool() {
    AsyncComputeTaskPool::get_or_init(|| TaskPoolBuilder::new().num_threads(2).build());
}

#[test]
fn spawning_under_the_same_key_twice_is_a_no_op() {
    ensure_pool();
    let mut world = World::new();
    let entity = world.spawn_empty().id();
    let spawn_count = Arc::new(Mutex::new(0u32));

    let mut entity_mut = world.entity_mut(entity);
    let count_a = spawn_count.clone();
    spawn_task(&mut entity_mut, 1u64, async move {
        *count_a.lock().expect("poison") += 1;
        std::future::pending::<()>().await;
    });
    assert_eq!(entity_mut.get::<TaskHandle>().unwrap().key().0, 1);

    // Spawning again under the same key must not replace the running task
    // (a second future would eventually bump the counter to 2).
    let count_b = spawn_count.clone();
    spawn_task(&mut entity_mut, 1u64, async move {
        *count_b.lock().expect("poison") += 1;
        std::future::pending::<()>().await;
    });
    assert_eq!(entity_mut.get::<TaskHandle>().unwrap().key().0, 1);

    // Give the (would-be, if wrongly double-spawned) second future a chance
    // to run, then confirm only the first one ever executed.
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        *spawn_count.lock().expect("poison"),
        1,
        "same-key respawn must be a no-op, not a second spawn"
    );
}

#[test]
fn spawning_under_a_different_key_replaces_the_tracked_task() {
    ensure_pool();
    let mut world = World::new();
    let entity = world.spawn_empty().id();

    let mut entity_mut = world.entity_mut(entity);
    spawn_task(&mut entity_mut, 1u64, async {
        std::future::pending::<()>().await;
    });
    assert_eq!(entity_mut.get::<TaskHandle>().unwrap().key().0, 1);

    spawn_task(&mut entity_mut, 2u64, async {
        std::future::pending::<()>().await;
    });
    assert_eq!(
        entity_mut.get::<TaskHandle>().unwrap().key().0,
        2,
        "a different key must replace the tracked TaskHandle"
    );
}

#[test]
fn despawning_the_entity_removes_its_task_handle() {
    ensure_pool();
    let mut world = World::new();
    let entity = world.spawn_empty().id();

    let mut entity_mut = world.entity_mut(entity);
    spawn_task(&mut entity_mut, 1u64, async {
        std::future::pending::<()>().await;
    });
    assert!(world.get::<TaskHandle>(entity).is_some());

    world.despawn(entity);

    assert!(world.get_entity(entity).is_err(), "entity is gone");
}

#[test]
fn spawned_task_actually_runs_on_the_pool_and_can_report_a_result() {
    ensure_pool();
    let mut world = World::new();
    let entity = world.spawn_empty().id();
    let result: Arc<Mutex<Option<i32>>> = Arc::new(Mutex::new(None));

    let mut entity_mut = world.entity_mut(entity);
    let result_handle = result.clone();
    spawn_task(&mut entity_mut, 1u64, async move {
        // Stand-in for a real (a)-type task's `ModelHandle::update(..)` call
        // on completion — the mechanism is identical either way: run to
        // completion on the pool, then report a result through a captured
        // handle.
        *result_handle.lock().expect("poison") = Some(42);
    });

    let start = Instant::now();
    loop {
        if *result.lock().expect("poison") == Some(42) {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "task did not complete in time"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
