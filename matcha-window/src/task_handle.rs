// native
#[cfg(not(target_arch = "wasm32"))]
pub struct TaskHandle<T>(pub(crate) tokio::task::JoinHandle<T>);

#[cfg(not(target_arch = "wasm32"))]
impl<T> TaskHandle<T> {
    pub fn is_finished(&self) -> bool {
        self.0.is_finished()
    }

    pub fn abort(&self) {
        self.0.abort();
    }

    pub async fn join(self) -> Option<T> {
        self.0.await.ok()
    }
}

// WASM
//
// Receiver<T> is !Sync (contains UnsafeCell), so we wrap it in parking_lot::Mutex
// to make TaskHandle<T>: Send + Sync when T: Send — required because Application
// stores TaskHandle<()> inside OnceLock which needs the payload to be Sync.
#[cfg(target_arch = "wasm32")]
pub struct TaskHandle<T> {
    abort_handle: futures::future::AbortHandle,
    finished: std::sync::Arc<std::sync::atomic::AtomicBool>,
    receiver: parking_lot::Mutex<Option<futures::channel::oneshot::Receiver<T>>>,
}

#[cfg(target_arch = "wasm32")]
impl<T> TaskHandle<T> {
    pub fn is_finished(&self) -> bool {
        self.finished
            .load(std::sync::atomic::Ordering::Acquire)
    }

    pub fn abort(&self) {
        self.abort_handle.abort();
    }

    pub async fn join(self) -> Option<T> {
        let rx = self.receiver.into_inner();
        match rx {
            Some(rx) => rx.await.ok(),
            None => None,
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub fn spawn_local_task<F, T>(f: F) -> TaskHandle<T>
where
    F: std::future::Future<Output = T> + 'static,
    T: 'static,
{
    use futures::future::{AbortHandle, Abortable};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    let (abort_handle, abort_reg) = AbortHandle::new_pair();
    let finished = Arc::new(AtomicBool::new(false));
    let finished_clone = finished.clone();
    let (tx, rx) = futures::channel::oneshot::channel::<T>();
    wasm_bindgen_futures::spawn_local(async move {
        if let Ok(val) = Abortable::new(f, abort_reg).await {
            let _ = tx.send(val);
        }
        finished_clone.store(true, Ordering::Release);
    });
    TaskHandle {
        abort_handle,
        finished,
        receiver: parking_lot::Mutex::new(Some(rx)),
    }
}
