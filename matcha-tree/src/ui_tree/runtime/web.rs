use std::cell::Cell;
use std::future::Future;
use std::rc::Rc;

// ----------------------------------------------------------------------------
// RuntimeHandle
// ----------------------------------------------------------------------------

/// Cheaply cloneable handle used to spawn background tasks.
///
/// On WASM the future does not need to be `Send` because everything runs on
/// the single JS thread.
#[derive(Clone, Default)]
pub struct RuntimeHandle;

impl RuntimeHandle {
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

// ----------------------------------------------------------------------------
// Runtime
// ----------------------------------------------------------------------------

/// Runtime stub for WASM — the browser microtask queue is the actual executor.
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

    /// Not supported on WASM — the browser owns the event loop and cannot be
    /// blocked. GPU initialisation must be performed in an async WASM entry
    /// point before constructing `UiTree`.
    pub fn block_on<F: Future>(&self, _future: F) -> F::Output {
        unreachable!("Runtime::block_on is not available on WASM")
    }

    /// Browser futures cannot be force-cancelled; this simply drops the handles
    /// and lets the tasks finish on their own.
    ///
    /// Safe because WASM is single-threaded: no render task can be executing
    /// concurrently when `destroy_surface` is called from the JS event loop.
    pub fn abort_and_join(&self, handles: impl IntoIterator<Item = JoinHandle>) {
        drop(handles.into_iter().collect::<Vec<_>>());
    }
}

impl Default for Runtime {
    fn default() -> Self {
        Self::new()
    }
}

// ----------------------------------------------------------------------------
// JoinHandle
// ----------------------------------------------------------------------------

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
