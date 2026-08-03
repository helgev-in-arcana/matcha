use std::sync::{Arc, LazyLock};

use tokio::sync::watch;

// ---------------------------------------------------------------------------
// BufferContext
// ---------------------------------------------------------------------------

/// Shared notification context for all [`SharedValue`](super::SharedValue) instances.
///
/// Uses a `watch` channel internally so that multiple rapid `signal()` calls
/// between two `subscribe()` polls coalesce into a single notification.
/// This prevents queued `BufferUpdated` commands from piling up when the event
/// loop is busy rendering.
pub struct BufferContext {
    tx: watch::Sender<()>,
}

static GLOBAL: LazyLock<Arc<BufferContext>> = LazyLock::new(BufferContext::new);

impl BufferContext {
    /// Creates a new isolated context.
    ///
    /// For the common case, prefer [`global()`](Self::global).
    pub fn new() -> Arc<Self> {
        let (tx, _) = watch::channel(());
        Arc::new(Self { tx })
    }

    /// Returns a reference to the global context.
    ///
    /// Initialized automatically on first access.
    pub fn global() -> &'static Arc<BufferContext> {
        &GLOBAL
    }
}

impl BufferContext {
    /// Sends a wakeup signal. Non-blocking, callable from any thread.
    ///
    /// Multiple calls between two `subscribe` polls coalesce — only one
    /// wakeup is delivered per polling interval.
    pub(crate) fn signal(&self) {
        // send_replace always notifies receivers even when the value is the
        // same type (), so every signal is guaranteed to be delivered.
        self.tx.send_replace(());
    }

    /// Returns a new [`watch::Receiver`] for this context.
    ///
    /// The receiver starts with the current value marked as "seen"; only
    /// future `signal()` calls will trigger [`watch::Receiver::changed`].
    pub fn subscribe(&self) -> watch::Receiver<()> {
        self.tx.subscribe()
    }
}
