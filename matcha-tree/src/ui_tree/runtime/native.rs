use std::future::Future;

// ----------------------------------------------------------------------------
// RuntimeHandle
// ----------------------------------------------------------------------------

/// Cheaply cloneable handle used to spawn background tasks.
///
/// On native targets the future must be `Send` because it may run on any
/// worker thread.
#[derive(Clone)]
pub struct RuntimeHandle {
    inner: tokio::runtime::Handle,
}

impl RuntimeHandle {
    pub(super) fn from_tokio(handle: tokio::runtime::Handle) -> Self {
        Self { inner: handle }
    }

    pub fn spawn<F>(&self, future: F) -> JoinHandle
    where
        F: Future<Output = ()> + Send + 'static,
    {
        JoinHandle {
            inner: self.inner.spawn(future),
        }
    }
}

// ----------------------------------------------------------------------------
// Runtime
// ----------------------------------------------------------------------------

/// Owns the async runtime for the lifetime of the application.
pub struct Runtime {
    inner: tokio::runtime::Runtime,
    handle: RuntimeHandle,
}

impl Runtime {
    pub fn new() -> Self {
        let inner = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("failed to create Tokio runtime");
        Self::from_tokio(inner)
    }

    pub fn from_tokio(inner: tokio::runtime::Runtime) -> Self {
        let handle = RuntimeHandle::from_tokio(inner.handle().clone());
        Self { inner, handle }
    }

    pub fn handle(&self) -> &RuntimeHandle {
        &self.handle
    }

    /// Blocks the current thread until `future` completes.
    ///
    /// Must be called from outside a tokio async context (e.g. from the winit
    /// event handler or before the event loop starts).
    pub fn block_on<F: Future>(&self, future: F) -> F::Output {
        self.inner.block_on(future)
    }

    /// Aborts the given tasks and blocks until they have all stopped.
    pub fn abort_and_join(&self, handles: impl IntoIterator<Item = JoinHandle>) {
        let handles: Vec<_> = handles.into_iter().collect();
        self.inner.block_on(async {
            for h in &handles {
                h.inner.abort();
            }
            for h in handles {
                let _ = h.inner.await;
            }
        });
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
    inner: tokio::task::JoinHandle<()>,
}

impl JoinHandle {
    pub fn abort(&self) {
        self.inner.abort();
    }

    pub fn is_finished(&self) -> bool {
        self.inner.is_finished()
    }
}
