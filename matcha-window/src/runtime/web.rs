use std::cell::Cell;
use std::future::Future;
use std::rc::Rc;

/// Cheaply cloneable handle used to spawn background tasks.
#[derive(Clone, Default)]
pub struct RuntimeHandle;

impl RuntimeHandle {
    /// Spawns a task on the browser microtask queue. The future does not
    /// need to be `Send` because everything runs on the single JS thread.
    pub fn spawn<F>(&self, future: F) -> JoinHandle
    where
        F: Future<Output = ()> + 'static,
    {
        let finished = Rc::new(Cell::new(false));
        let flag = Rc::clone(&finished);
        wasm_bindgen_futures::spawn_local(async move {
            future.await;
            flag.set(true);
        });
        JoinHandle { finished }
    }
}

/// Owns the async runtime for the lifetime of the application.
pub struct Runtime {
    handle: RuntimeHandle,
}

impl Runtime {
    pub fn new() -> Self {
        Self {
            handle: RuntimeHandle,
        }
    }

    pub fn handle(&self) -> &RuntimeHandle {
        &self.handle
    }

    /// No-op on WASM: there is no ambient runtime context to enter.
    pub fn enter(&self) -> EnterGuard {
        EnterGuard
    }

    /// Browser futures cannot be force-cancelled, so this just drops the
    /// handles and lets the tasks finish on their own.
    pub fn abort_and_join(&self, handles: impl IntoIterator<Item = JoinHandle>) {
        drop(handles.into_iter().collect::<Vec<_>>());
    }
}

impl Default for Runtime {
    fn default() -> Self {
        Self::new()
    }
}

/// Placeholder guard; the WASM runtime has no context to keep active.
pub struct EnterGuard;

/// Handle to a spawned task.
pub struct JoinHandle {
    finished: Rc<Cell<bool>>,
}

impl JoinHandle {
    /// No-op: a browser future cannot be force-cancelled.
    pub fn abort(&self) {}

    pub fn is_finished(&self) -> bool {
        self.finished.get()
    }
}
