use std::future::Future;

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub(super) struct Runtime(Option<tokio::runtime::Runtime>);

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
impl Runtime {
    pub fn new() -> Self {
        Self(Some(
            tokio::runtime::Runtime::new().expect("failed to create the app's Tokio runtime"),
        ))
    }

    fn inner(&self) -> &tokio::runtime::Runtime {
        self.0.as_ref().expect("runtime is taken only on drop")
    }

    pub fn spawn(&self, future: impl Future<Output = ()> + utils::MaybeSend + 'static) {
        self.inner().spawn(future);
    }
}

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
impl Drop for Runtime {
    fn drop(&mut self) {
        // The app can be dropped inside its host's Tokio task, where blocking is disallowed.
        if let Some(runtime) = self.0.take() {
            runtime.shutdown_background();
        }
    }
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
pub(super) struct Runtime;

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
impl Runtime {
    pub fn new() -> Self {
        Self
    }

    pub fn spawn(&self, future: impl Future<Output = ()> + utils::MaybeSend + 'static) {
        wasm_bindgen_futures::spawn_local(future);
    }
}

impl Runtime {
    pub fn with_context<R>(&self, callback: impl FnOnce() -> R) -> R {
        #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
        let _guard = self.inner().enter();
        callback()
    }
}
