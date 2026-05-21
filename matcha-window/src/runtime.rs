//! Cross-platform async runtime abstraction.
//!
//! Native targets drive background tasks on a multi-threaded Tokio runtime.
//! WASM targets drive them on the browser microtask queue via
//! `wasm-bindgen-futures`. The rest of the framework only ever sees
//! [`Runtime`], [`RuntimeHandle`] and [`JoinHandle`], never the backend.

#[cfg(not(target_arch = "wasm32"))]
pub use native::{EnterGuard, JoinHandle, Runtime, RuntimeHandle};
#[cfg(target_arch = "wasm32")]
pub use wasm::{EnterGuard, JoinHandle, Runtime, RuntimeHandle};

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::future::Future;

    /// Cheaply cloneable handle used to spawn background tasks.
    #[derive(Clone)]
    pub struct RuntimeHandle {
        inner: tokio::runtime::Handle,
    }

    impl RuntimeHandle {
        /// Spawns a task on the runtime. On native targets the future must be
        /// `Send` because it may run on any worker thread.
        pub fn spawn<F>(&self, future: F) -> JoinHandle
        where
            F: Future<Output = ()> + Send + 'static,
        {
            JoinHandle {
                inner: self.inner.spawn(future),
            }
        }

        /// Access the underlying Tokio handle (native only).
        pub fn tokio(&self) -> &tokio::runtime::Handle {
            &self.inner
        }
    }

    /// Owns the async runtime for the lifetime of the application.
    pub struct Runtime {
        inner: tokio::runtime::Runtime,
        handle: RuntimeHandle,
    }

    impl Runtime {
        /// Creates a new multi-threaded Tokio runtime.
        pub fn new() -> Self {
            let inner =
                tokio::runtime::Runtime::new().expect("failed to create Tokio runtime");
            Self::from_tokio(inner)
        }

        /// Wraps an externally created Tokio runtime.
        pub fn from_tokio(inner: tokio::runtime::Runtime) -> Self {
            let handle = RuntimeHandle {
                inner: inner.handle().clone(),
            };
            Self { inner, handle }
        }

        pub fn handle(&self) -> &RuntimeHandle {
            &self.handle
        }

        /// Enters the runtime context; the returned guard must be kept alive
        /// while runtime-dependent code runs.
        pub fn enter(&self) -> EnterGuard<'_> {
            EnterGuard {
                _inner: self.inner.enter(),
            }
        }

        /// Aborts the given tasks and blocks until they have stopped.
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

    /// Keeps the runtime context active for its lifetime.
    pub struct EnterGuard<'a> {
        _inner: tokio::runtime::EnterGuard<'a>,
    }

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
}

#[cfg(target_arch = "wasm32")]
mod wasm {
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
}
