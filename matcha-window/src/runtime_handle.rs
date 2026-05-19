use crate::task_handle::TaskHandle;

// ---------------------------------------------------------------------------
// Native
// ---------------------------------------------------------------------------

#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone)]
pub struct RuntimeHandle(tokio::runtime::Handle);

#[cfg(not(target_arch = "wasm32"))]
impl RuntimeHandle {
    pub fn from_tokio(h: tokio::runtime::Handle) -> Self {
        Self(h)
    }

    pub fn spawn<F, T>(&self, f: F) -> TaskHandle<T>
    where
        F: std::future::Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        TaskHandle(self.0.spawn(f))
    }

    pub fn enter(&self) -> tokio::runtime::EnterGuard<'_> {
        self.0.enter()
    }

    pub fn tokio_handle(&self) -> &tokio::runtime::Handle {
        &self.0
    }
}

// ---------------------------------------------------------------------------
// WASM
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
#[derive(Clone)]
pub struct RuntimeHandle;

#[cfg(target_arch = "wasm32")]
impl RuntimeHandle {
    pub fn spawn<F, T>(&self, f: F) -> TaskHandle<T>
    where
        F: std::future::Future<Output = T> + 'static,
        T: 'static,
    {
        crate::task_handle::spawn_local_task(f)
    }

    pub fn enter(&self) -> WasmEnterGuard {
        WasmEnterGuard
    }
}

#[cfg(target_arch = "wasm32")]
pub struct WasmEnterGuard;
