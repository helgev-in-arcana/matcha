use dashmap::DashMap;
use parking_lot::Mutex;
use std::sync::Weak;
use std::{any::Any, sync::Arc};

use super::window::AnyWindowWidgetInstance;
use matcha_window::adapter::EventLoop;
use matcha_window::window::WindowId;
use matcha_window::window::{Window, WindowConfig, WindowError};

pub use super::runtime::{Runtime, RuntimeHandle};

// ----------------------------------------------------------------------------
// EventSender / EventReceiver
// ----------------------------------------------------------------------------

/// Type-erased message carried over the backend channel.
///
/// Native targets additionally require `Send` (supplied by `MaybeSend`) so
/// messages can cross threads; on wasm `MaybeSend` collapses to a no-op.
pub trait AnyMessage: Any + utils::MaybeSend {
    /// Erases this trait object down to `dyn Any` so callers can `downcast`.
    fn into_any(self: Box<Self>) -> Box<dyn Any>;
}

impl<T: Any + utils::MaybeSend> AnyMessage for T {
    fn into_any(self: Box<Self>) -> Box<dyn Any> {
        self
    }
}

pub(super) type BoxedMessage = Box<dyn AnyMessage>;

/// Type-erased sender for messages from Component background tasks back to TreeApp.
///
/// Clone-able; each clone sends to the same channel. Use `emit()` to post a
/// message that TreeApp will downcast to `C::Message` in `buffer_updated()`.
#[derive(Clone)]
pub struct EventSender {
    sender: tokio::sync::mpsc::UnboundedSender<BoxedMessage>,
}

impl EventSender {
    pub(super) fn new(sender: tokio::sync::mpsc::UnboundedSender<BoxedMessage>) -> Self {
        Self { sender }
    }

    pub fn emit<T: Any + utils::MaybeSend + 'static>(&self, msg: T) {
        let _ = self.sender.send(Box::new(msg));
    }
}

/// Paired receiver for `EventSender`.
pub(super) struct EventReceiver {
    receiver: tokio::sync::mpsc::UnboundedReceiver<BoxedMessage>,
}

impl EventReceiver {
    pub(super) fn new(receiver: tokio::sync::mpsc::UnboundedReceiver<BoxedMessage>) -> Self {
        Self { receiver }
    }

    pub(super) fn try_recv(
        &mut self,
    ) -> Result<BoxedMessage, tokio::sync::mpsc::error::TryRecvError> {
        self.receiver.try_recv()
    }

    pub(super) async fn recv(&mut self) -> Option<BoxedMessage> {
        self.receiver.recv().await
    }
}

// ----------------------------------------------------------------------------
// AppContext
// ----------------------------------------------------------------------------

/// Context passed to Component lifecycle methods (init, resumed, suspended, exiting).
pub struct AppContext<'a> {
    pub(super) runtime_handle: RuntimeHandle,
    pub(super) event_sender: &'a EventSender,
    pub(super) event_loop: &'a dyn EventLoop,
}

impl<'a> AppContext<'a> {
    pub fn runtime_handle(&self) -> RuntimeHandle {
        self.runtime_handle.clone()
    }

    /// Returns a clone of the type-erased event sender.
    /// Use `sender.emit(your_message)` in spawned tasks to wake up TreeApp.
    pub fn event_sender(&self) -> EventSender {
        self.event_sender.clone()
    }

    pub fn event_loop(&self) -> &dyn EventLoop {
        self.event_loop
    }

    /// Convenience: emit a message directly (no need to call `event_sender()` first).
    pub fn emit<T: Any + utils::MaybeSend + 'static>(&self, msg: T) {
        self.event_sender.emit(msg);
    }
}

// ----------------------------------------------------------------------------
// SharedCtx
// ----------------------------------------------------------------------------

/// Stable, window-independent resources shared across all widget method calls.
///
/// Lives on the caller's stack and is borrowed by [`UiContext`].
/// Adding new resources here does not change `UiContext`'s stack size.
pub(super) struct SharedCtx<'a> {
    pub(super) runtime_handle: RuntimeHandle,
    pub(super) event_sender: &'a EventSender,
    pub(super) window_registry: &'a DashMap<WindowId, Weak<Mutex<dyn AnyWindowWidgetInstance>>>,
    pub(super) gpu_instance: &'a wgpu::Instance,
    pub(super) gpu_device: wgpu::Device,
    pub(super) gpu_queue: wgpu::Queue,
    pub(super) surface_creation_permitted: bool,
}

// ----------------------------------------------------------------------------
// WindowCtx
// ----------------------------------------------------------------------------

/// Per-window context set by [`WindowWidgetInstance::map_ui_context`].
///
/// Stored as `Option<WindowCtx>` inside [`UiContext`]; `None` outside a window pass.
#[derive(Clone)]
pub(super) struct WindowCtx {
    pub(super) dpi: f64,
    pub(super) format: wgpu::TextureFormat,
    pub(super) config: WindowConfig,
    pub(super) inner_size: [f32; 2],
}

// ----------------------------------------------------------------------------
// UiContext
// ----------------------------------------------------------------------------

/// Context passed to all Component and Widget methods.
///
/// Internally holds a reference to [`SharedCtx`] (stable GPU + registry resources)
/// plus a small optional [`WindowCtx`] for per-window values (DPI, format, config).
/// The struct itself stays small regardless of how many resources are added to `SharedCtx`.
#[derive(Copy)]
pub struct UiContext<'a> {
    pub(super) event_loop: Option<&'a dyn EventLoop>,
    pub(super) shared: &'a SharedCtx<'a>,
    pub(super) window: Option<&'a WindowCtx>,
}

impl<'a> Clone for UiContext<'a> {
    fn clone(&self) -> Self {
        Self {
            shared: self.shared,
            event_loop: self.event_loop,
            window: self.window,
        }
    }
}

/// Run UI construction, measurement and drawing against a caller-owned GPU
/// without creating a window or an event loop.
///
/// The context reports `dpi() == Some(1.0)`, the given output format and viewport
/// in physical pixels, so viewport-relative sizes resolve as they do in a window
/// pass. GPU handles are only cloned for this scope; no GPU is created or submitted
/// here. The caller owns the runtime and must keep it alive for any spawned tasks.
///
/// The temporary message channel and window registry are scoped to this call;
/// messages are not delivered to an application. Window widgets cannot be created
/// through this context. Its borrowed state cannot escape the callback, although
/// owned widget trees and resource definitions may be returned and reused.
pub fn with_offscreen_context<R>(
    runtime_handle: RuntimeHandle,
    instance: &wgpu::Instance,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    viewport: [f32; 2],
    format: wgpu::TextureFormat,
    run: impl for<'ctx> FnOnce(&UiContext<'ctx>) -> R,
) -> R {
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    // There is no application event pump; do not accumulate undelivered messages.
    drop(receiver);
    let sender = EventSender::new(sender);
    let registry = DashMap::new();
    let shared = SharedCtx {
        runtime_handle,
        event_sender: &sender,
        window_registry: &registry,
        gpu_instance: instance,
        gpu_device: device.clone(),
        gpu_queue: queue.clone(),
        surface_creation_permitted: false,
    };
    let physical_size = viewport.map(|value| value.max(0.0) as u32);
    let mut config = WindowConfig::default()
        .with_surface_format(format)
        .with_inner_size(physical_size);
    config.surface_config.width = physical_size[0];
    config.surface_config.height = physical_size[1];
    let window = WindowCtx {
        dpi: 1.0,
        format,
        config,
        inner_size: viewport,
    };
    run(&UiContext {
        event_loop: None,
        shared: &shared,
        window: Some(&window),
    })
}

impl UiContext<'_> {
    pub(crate) fn register_window_instance(
        &self,
        instance: Arc<Mutex<dyn AnyWindowWidgetInstance>>,
    ) {
        let id = instance.lock().window_id();
        self.shared
            .window_registry
            .insert(id, Arc::downgrade(&instance));
    }

    pub(crate) fn create_window(&self, config: &WindowConfig) -> Result<Window, WindowError> {
        let event_loop = self.event_loop.ok_or_else(|| {
            WindowError::BackendError("window creation requires an application event loop".into())
        })?;
        let mut window = Window::new(config, event_loop)?;
        if self.shared.surface_creation_permitted {
            window.create_surface(self.shared.gpu_instance, &self.shared.gpu_device)?;
        }
        Ok(window)
    }

    pub fn runtime_handle(&self) -> RuntimeHandle {
        self.shared.runtime_handle.clone()
    }

    pub fn event_sender(&self) -> EventSender {
        self.shared.event_sender.clone()
    }

    pub fn emit<T: Any + utils::MaybeSend + 'static>(&self, msg: T) {
        self.shared.event_sender.emit(msg);
    }

    pub fn window_config(&self) -> Option<&WindowConfig> {
        self.window.map(|w| &w.config)
    }

    pub fn dpi(&self) -> Option<f64> {
        self.window.map(|w| w.dpi)
    }

    pub fn surface_format(&self) -> Option<wgpu::TextureFormat> {
        self.window.map(|w| w.format)
    }

    /// Returns the inner size of the current window in physical pixels.
    /// `None` when called outside a window pass (e.g. during update without a window).
    pub fn viewport_size(&self) -> Option<[f32; 2]> {
        self.window.map(|w| w.inner_size)
    }

    pub fn gpu_instance(&self) -> &wgpu::Instance {
        self.shared.gpu_instance
    }

    pub fn gpu_device(&self) -> &wgpu::Device {
        &self.shared.gpu_device
    }

    pub fn gpu_queue(&self) -> &wgpu::Queue {
        &self.shared.gpu_queue
    }
}
