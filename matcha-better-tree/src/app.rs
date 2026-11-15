//! A matcha-window application driving one model and its declared native windows.
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};

use matcha_window::adapter::{EventLoop, EventLoopProxy};
use matcha_window::application::Application;
use matcha_window::event::{device_event::DeviceEvent, window_event::WindowEvent};
use matcha_window::window::{Window as OsWindow, WindowId};

use crate::ui_tree::context::{Context, Services};
use crate::ui_tree::{
    UiTree,
    widget::View,
    window::{WindowConfig, WindowRegistry},
};
pub use gpu_utils::gpu::{Gpu, GpuDescriptor};

mod presentation;
mod runtime;
use presentation::Presentation;
use runtime::Runtime;

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
use std::rc::{Rc as Shared, Weak};
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
use std::sync::{Arc as Shared, Weak};

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
type Wake = dyn Fn() + Send + Sync;
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
type Wake = dyn Fn();

/// Produces a view on the UI thread. A parent can drive a child model by calling
/// its `view` with the same context wherever the child's output is needed.
pub trait Model {
    /// Apply any pending model input and return the current UI declaration.
    fn view(&mut self, ctx: &ModelContext<'_>) -> impl View;
}

/// App services available while evaluating the root model and its children.
///
/// On native targets the app enters its Tokio runtime for the duration of
/// [`Model::view`], including synchronous calls to child models and components.
/// The caller's runtime context is restored before widget reconciliation.
pub struct ModelContext<'a> {
    runtime: &'a Runtime,
    view_update_handle: ViewUpdateHandle,
}

impl ModelContext<'_> {
    /// Spawn a detached task on the app's Tokio runtime (native) or the browser's
    /// microtask queue (Web). Native futures must be `Send`. Dropping the app
    /// shuts down its native runtime. Browser tasks may outlive the app.
    pub fn spawn(&self, future: impl Future<Output = ()> + utils::MaybeSend + 'static) {
        self.runtime.spawn(future);
    }

    /// Borrow the handle for requesting a root view evaluation.
    /// Clone it to keep it in a widget callback or task.
    pub fn view_update_handle(&self) -> &ViewUpdateHandle {
        &self.view_update_handle
    }
}

struct ViewRequest {
    pending: AtomicBool,
    wake: Box<Wake>,
}

impl ViewRequest {
    fn request(&self) {
        if !self.pending.swap(true, Ordering::AcqRel) {
            (self.wake)();
        }
    }
}

/// A handle for scheduling a root view evaluation with [`request`](Self::request).
#[derive(Clone)]
pub struct ViewUpdateHandle {
    request: Weak<ViewRequest>,
}

impl ViewUpdateHandle {
    /// Coalesces requests until the host consumes them. The weak handle allows
    /// a task holding it to outlive the app.
    pub fn request(&self) {
        if let Some(request) = self.request.upgrade() {
            request.request();
        }
    }
}

/// Owns the model, its retained widgets and window resources for a window event loop.
pub struct App<M> {
    model: M,
    tree: UiTree,
    windows: WindowRegistry,
    services: Services,
    request: Option<Shared<ViewRequest>>,
    presentation: Presentation,
    surfaces_allowed: bool,
    runtime: Runtime,
}

impl<M: Model + utils::MaybeSendSync + 'static> App<M> {
    /// Pass the app to [`matcha_window::adapter::Adapter::new`]. The first view
    /// is evaluated when the event loop permits window and surface creation.
    ///
    /// Starting the native winit backend:
    /// ```no_run
    /// # use matcha_better_tree::app::{App, Gpu, GpuDescriptor, Model};
    /// # use matcha_window::adapter::Adapter;
    /// # #[cfg(all(feature = "winit", not(all(target_arch = "wasm32", target_os = "unknown"))))]
    /// # async fn start<M: Model + utils::MaybeSendSync + 'static>(model: M) -> Result<(), Box<dyn std::error::Error>> {
    /// let gpu = Gpu::new(GpuDescriptor::standard()).await?;
    /// Adapter::new(App::new(model, gpu)).run()?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Panics
    /// On native targets, panics if the Tokio runtime cannot be created.
    pub fn new(model: M, gpu: Gpu) -> Self {
        Self {
            model,
            tree: UiTree::new(),
            windows: WindowRegistry::default(),
            services: Services::default(),
            request: None,
            presentation: Presentation::new(gpu),
            surfaces_allowed: false,
            runtime: Runtime::new(),
        }
    }

    fn update_view(&mut self, event_loop: &impl EventLoop, resuming: bool) -> bool {
        let Some(request) = &self.request else {
            return false;
        };
        // Consume before evaluating: a request from the view schedules another iteration.
        if !request.pending.swap(false, Ordering::AcqRel) && !resuming {
            return false;
        }
        let ctx = ModelContext {
            runtime: &self.runtime,
            view_update_handle: ViewUpdateHandle {
                request: Shared::downgrade(request),
            },
        };
        let view = {
            #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
            let _guard = self.runtime.enter();
            self.model.view(&ctx)
        };
        let mut open = |declaration: &WindowConfig| {
            let config = declaration
                .clone()
                .into_inner_config(self.presentation.surface_config().clone());
            OsWindow::new(&config, event_loop)
        };
        let mut root = Context::with_services(&mut self.windows, &mut self.services);
        if let Err(error) = self
            .tree
            .try_update(&view, &mut root.with_window_factory(&mut open))
        {
            log::error!("Mounting window: {error}");
        }
        true
    }

    fn attach_surfaces(&mut self) {
        for (window, platform) in self.windows.live_windows_mut() {
            if let Err(error) = self.presentation.attach(window) {
                log::error!("Attaching surface for {:?}: {error}", window.id());
            }
            platform.sync_to_window(window);
        }
    }
}

impl<M: Model + utils::MaybeSendSync + 'static> Application for App<M> {
    type Command = ();

    fn init(&mut self, proxy: Box<dyn EventLoopProxy<Self>>, _: &impl EventLoop) {
        self.request = Some(Shared::new(ViewRequest {
            pending: AtomicBool::new(false),
            wake: Box::new(move || proxy.send_command(())),
        }));
    }

    fn resumed(&mut self, _: &impl EventLoop) {
        // nothing to do
    }

    fn create_surface(&mut self, event_loop: &impl EventLoop) {
        let resuming = !self.surfaces_allowed;
        self.surfaces_allowed = true;
        // On resume the OS can accept mounts again, including ones that failed on an earlier pass.
        self.update_view(event_loop, resuming);
        self.attach_surfaces();
    }

    fn destroy_surface(&mut self, _: &impl EventLoop) {
        self.surfaces_allowed = false;
        for (window, _) in self.windows.live_windows_mut() {
            window.destroy_surface();
        }
    }

    fn suspended(&mut self, _: &impl EventLoop) {
        // nothing to do
    }

    fn exiting(&mut self, _: &impl EventLoop) {
        self.request = None;
        self.surfaces_allowed = false;
        self.tree.clear(&mut Context::with_services(
            &mut self.windows,
            &mut self.services,
        ));
    }

    fn render(&mut self, id: WindowId) {
        if !self.surfaces_allowed {
            return;
        }
        let mut ctx = Context::with_services(&mut self.windows, &mut self.services);
        if let Err(error) = self.presentation.render(id, &mut self.tree, &mut ctx) {
            log::error!("Rendering window {id:?}: {error}");
        }
        self.windows.sync_to_window(id);
    }

    fn window_event(&mut self, _: &impl EventLoop, id: WindowId, event: WindowEvent) {
        if matches!(
            event,
            WindowEvent::Resized { .. } | WindowEvent::ScaleFactorChanged { .. }
        ) && let Some(window) = self.windows.window(id)
        {
            self.presentation.reconfigure_surface(window);
        }
        self.tree.window_event(
            id,
            &event,
            &mut Context::with_services(&mut self.windows, &mut self.services),
        );
    }

    fn window_destroyed(&mut self, event_loop: &impl EventLoop, id: WindowId) {
        let was_declared = self.tree.window_destroyed(
            id,
            &mut Context::with_services(&mut self.windows, &mut self.services),
        );
        // Re-read the model before reopening an externally destroyed window:
        // it may already have received a message asking to remove the declaration.
        if was_declared
            && !event_loop.exiting()
            && let Some(request) = &self.request
        {
            request.request();
        }
    }

    fn device_event(&mut self, _: &impl EventLoop, id: WindowId, event: DeviceEvent) {
        if !self.surfaces_allowed {
            return;
        }
        let mut ctx = Context::with_services(&mut self.windows, &mut self.services);
        // The hit path depends on the latest reconciliation and native viewport.
        self.tree.layout_window(id, &mut ctx);
        let result = self.tree.input_window(id, &event, &mut ctx);
        if !result.update.is_empty()
            && let Some(window) = self.windows.window(id)
        {
            window.request_redraw();
        }
        self.windows.sync_to_window(id);
    }

    fn ui_command(&mut self, event_loop: &impl EventLoop, (): ()) {
        // During suspension the pending request is consumed by the next create_surface.
        if self.surfaces_allowed && self.update_view(event_loop, false) {
            self.attach_surfaces();
        }
    }

    fn raw_device_event(
        &mut self,
        event_loop: &impl EventLoop,
        raw_device_id: matcha_window::event::raw_device_event::RawDeviceId,
        raw_event: matcha_window::event::raw_device_event::RawDeviceEvent,
    ) {
        let _ = event_loop;
        let _ = raw_device_id;
        let _ = raw_event;
    }

    fn poll(&mut self, event_loop: &impl EventLoop) {
        let _ = event_loop;
    }

    fn resume_time_reached(
        &mut self,
        event_loop: &impl EventLoop,
        start: web_time::Instant,
        requested_resume: web_time::Instant,
    ) {
        let _ = event_loop;
        let _ = start;
        let _ = requested_resume;
    }

    fn wait_cancelled(
        &mut self,
        event_loop: &impl EventLoop,
        start: web_time::Instant,
        requested_resume: Option<web_time::Instant>,
    ) {
        let _ = event_loop;
        let _ = start;
        let _ = requested_resume;
    }

    fn about_to_wait(&mut self, event_loop: &impl EventLoop) {
        let _ = event_loop;
    }

    fn memory_warning(&mut self, event_loop: &impl EventLoop) {
        let _ = event_loop;
    }
}

#[cfg(test)]
mod tests;
