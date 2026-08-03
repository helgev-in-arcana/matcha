use std::sync::Arc;

use arc_swap::{ArcSwap, Guard};

use super::context::BufferContext;

// ---------------------------------------------------------------------------
// SignalContext (internal)
// ---------------------------------------------------------------------------

/// Determines which [`BufferContext`] a [`SharedValue`] signals on `store()`.
enum SignalContext {
    /// Uses the global context (lazily initialized).
    Global,
    /// A fixed custom context supplied at construction time.
    Custom(Arc<BufferContext>),
}

impl SignalContext {
    fn signal(&self) {
        let ctx = match self {
            Self::Global => BufferContext::global(),
            Self::Custom(ctx) => ctx,
        };
        ctx.signal();
    }
}

// ---------------------------------------------------------------------------
// SharedValue<T>
// ---------------------------------------------------------------------------

/// A thread-safe value buffer that automatically wakes the event loop on write.
///
/// - `store()` is callable with `&self` from any thread.
/// - `store()` atomically replaces the value and sends a wakeup signal to the
///   event loop via the associated [`BufferContext`].
/// - The internal implementation (`arc-swap`) is fully hidden behind this API.
///
/// # Using the global context (common case)
///
/// ```rust
/// use crate::shared_buffer::SharedValue;
/// 
/// let v = SharedValue::new(0.0f32);
/// v.store(1.0);
/// ```
///
/// # Using a custom context
///
/// ```rust
/// use crate::shared_buffer::SharedValue;
/// use crate::shared_buffer::BufferContext;
/// 
/// let ctx = BufferContext::new();
/// let v = SharedValue::new_in(0.0f32, ctx);
/// ```
pub struct SharedValue<T: Send + Sync + 'static> {
    inner: ArcSwap<T>,
    ctx: SignalContext,
}

impl<T: Send + Sync + 'static> SharedValue<T> {
    /// Creates a `SharedValue` backed by the global context.
    pub fn new(value: T) -> Self {
        Self {
            inner: ArcSwap::from_pointee(value),
            ctx: SignalContext::Global,
        }
    }

    /// Creates a `SharedValue` backed by a custom context.
    pub fn new_in(value: T, ctx: Arc<BufferContext>) -> Self {
        Self {
            inner: ArcSwap::from_pointee(value),
            ctx: SignalContext::Custom(ctx),
        }
    }

    /// Replaces the stored value and sends a wakeup signal to the event loop.
    ///
    /// Callable with `&self` from any thread.
    pub fn store(&self, value: T) {
        self.inner.store(Arc::new(value));
        self.ctx.signal();
    }

    /// Returns the current value as a zero-copy guard.
    ///
    /// The returned `Guard` implements `Deref<Target = T>` and keeps the
    /// inner `Arc` alive for its lifetime.
    pub fn load(&self) -> Guard<Arc<T>> {
        self.inner.load()
    }

    /// Returns a clone of the current value.
    pub fn get(&self) -> T
    where
        T: Clone,
    {
        T::clone(&self.inner.load())
    }
}
