//! Async tasks bound to a widget entity's lifetime (M8, "(a) 型" per
//! `ECS_ARCHITECTURE.md` §5.4). Spawned from `Widget::patch`/`after_spawn`
//! (`&mut EntityWorldMut` already provides entity context) — no dedicated
//! scheduled system or `view.rs`/reducer change is needed, since [`TaskHandle`]
//! is a plain component: dropping it (entity despawn, or a reconcile
//! type-change rebuild — both already despawn the entity today) requests
//! cancellation of the underlying `bevy_tasks::Task` via its own `Drop` impl,
//! for free.
//!
//! **Cancellation latency caveat (confirmed experimentally, not just from
//! docs)**: dropping a `bevy_tasks::Task` stops it from ever being polled (or
//! its result observed) again, but does **not** promptly reclaim the future's
//! captured resources unless the future gets a chance to be polled once more
//! after the drop — which only happens if something wakes it again. A future
//! that never re-wakes itself (e.g. `std::future::pending()`) may hold its
//! resources indefinitely (observed: still not dropped after 1.5s of
//! wall-clock time with the future being woken every 10ms by a helper
//! thread, in a standalone experiment). Tasks that need prompt resource
//! release on cancellation should check a cooperative "still wanted" signal
//! themselves (e.g. an `Arc<AtomicBool>` read at each await point) rather
//! than relying purely on drop timing — this matches `ECS_ROADMAP.md`'s
//! original task-cancellation note ("タスクが自分で `is_cancelled` 等を
//! チェックする") more closely than the drop-cancels framing in
//! `ECS_ARCHITECTURE.md` §5.4 suggests on its own.
//!
//! "(b) 型" (app-logic tasks, e.g. save/export) need none of this — they're
//! already just `bevy_tasks::AsyncComputeTaskPool::get().spawn(..)` +
//! `ModelHandle::update`, with no entity involved at all.

use std::future::Future;

use bevy_ecs::{component::Component, world::EntityWorldMut};

/// Dedup key for a [`TaskHandle`]: spawning under a key already running on
/// the same entity is a no-op; spawning under a different key replaces (and
/// so cancels) whatever was running under the old one.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct TaskKey(pub u64);

impl From<u64> for TaskKey {
    fn from(v: u64) -> Self {
        TaskKey(v)
    }
}

/// A UI-lifecycle-bound task handle. SparseSet: attached only while a task is
/// running, so it doesn't affect the widget's archetype.
#[derive(Component)]
#[component(storage = "SparseSet")]
pub struct TaskHandle {
    key: TaskKey,
    _task: bevy_tasks::Task<()>,
}

impl TaskHandle {
    /// The key this handle's task was spawned under.
    pub fn key(&self) -> TaskKey {
        self.key
    }
}

/// Spawn a task scoped to `entity`'s lifetime: dropping it (entity despawn,
/// or a reconcile type-change rebuild, which despawns and respawns) requests
/// cancellation of the task automatically (see the module docs for a caveat
/// on how promptly that actually reclaims resources).
///
/// No-op if `entity` already has a live [`TaskHandle`] with the same `key`.
/// Spawning under a different key replaces (and so cancels) whatever was
/// running under the old one — an entity tracks at most one task at a time.
///
/// `future` must report its own result by calling a captured
/// [`crate::model::ModelHandle::update`] before finishing — there is no
/// polling system here (`ECS_ARCHITECTURE.md` §5.4 forbids one: the event
/// loop sleeps between redraws, so nothing would ever observe completion
/// otherwise).
pub fn spawn_task(
    entity: &mut EntityWorldMut,
    key: impl Into<TaskKey>,
    future: impl Future<Output = ()> + Send + 'static,
) {
    let key = key.into();
    if entity.get::<TaskHandle>().is_some_and(|h| h.key == key) {
        return;
    }
    let task = bevy_tasks::AsyncComputeTaskPool::get().spawn(future);
    entity.insert(TaskHandle { key, _task: task });
}
