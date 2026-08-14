//! Model handle: the write side of the Model update loop.
//!
//! `ModelHandle<M>` is a cheaply-cloneable value that background tasks (and,
//! from M5, input handlers) use to mutate the model from anywhere. `update`
//! queues a mutation and wakes the event loop; `UiEcs` drains the queue and
//! re-runs the view on the next `ui_command(UiCommand::ModelUpdated)`.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc,
};

use bevy_ecs::resource::Resource;

/// World-resource wrapper for the user's model type `M`.
///
/// `M` belongs to user code and cannot itself implement bevy's `Resource`, so
/// this gives it one. A storage-strategy trait (`ModelStorage`, per
/// `ECS_IMPLEMENTATION_PLAN.md` §4.2) is deferred until a second strategy is
/// actually needed — this direct wrapper is the whole of it for now.
#[derive(Resource)]
pub struct ModelResource<M: Send + Sync + 'static>(pub M);

/// The "wake the event loop" callback held by a [`ModelHandle`].
///
/// It closes over an `EventLoopProxy`, which is only `Send + Sync` on native —
/// winit's web proxy is not, and there is no second thread on the web to send
/// it to anyway. Spelled as two aliases rather than one `MaybeSendSync` bound
/// because that trait is not an auto trait and so cannot appear alongside `Fn`
/// in a trait object.
#[cfg(not(web))]
pub(crate) type WakeFn = Arc<dyn Fn() + Send + Sync>;
#[cfg(web)]
pub(crate) type WakeFn = Arc<dyn Fn()>;

/// Cheaply-cloneable handle for mutating the model from any thread (native; on
/// the web there is only the one).
///
/// Queued mutations are drained on the next model-update cycle. `wake` fires
/// at most once per cycle: it only runs when a call transitions `wake_pending`
/// from `false` to `true`, so a burst of `update()` calls between drains costs
/// one wake, not one per call.
///
/// Stored as a world `Resource` so systems can fetch and clone it (`view/task
/// から clone で取り出す`); `UiEcs::model_handle` clones the same value for use
/// before the event loop starts.
#[derive(Resource)]
pub struct ModelHandle<M: 'static> {
    sender: mpsc::Sender<Box<dyn FnOnce(&mut M) + Send>>,
    wake_pending: Arc<AtomicBool>,
    wake: WakeFn,
}

// `ModelHandle` is stored as a bevy resource, so it must be `Send + Sync`, but
// on the web its `WakeFn` captures winit's event-loop proxy, which is not.
//
// SAFETY: single-threaded target — see the guard in this crate's lib.rs, which
// turns that assumption into a build failure if a threaded wasm target is used.
#[cfg(web)]
unsafe impl<M: 'static> Send for ModelHandle<M> {}
#[cfg(web)]
unsafe impl<M: 'static> Sync for ModelHandle<M> {}

impl<M: 'static> Clone for ModelHandle<M> {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
            wake_pending: self.wake_pending.clone(),
            wake: self.wake.clone(),
        }
    }
}

impl<M: 'static> ModelHandle<M> {
    pub(crate) fn new(
        sender: mpsc::Sender<Box<dyn FnOnce(&mut M) + Send>>,
        wake_pending: Arc<AtomicBool>,
        wake: WakeFn,
    ) -> Self {
        Self {
            sender,
            wake_pending,
            wake,
        }
    }

    /// Queue a mutation of the model. Runs on the next drain, not immediately.
    pub fn update(&self, f: impl FnOnce(&mut M) + Send + 'static) {
        let _ = self.sender.send(Box::new(f));
        if !self.wake_pending.swap(true, Ordering::AcqRel) {
            (self.wake)();
        }
    }
}
