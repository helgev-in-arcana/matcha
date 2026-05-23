use std::{collections::HashMap, sync::Arc};

use crate::{
    application::Application,
    event::{
        EventStateConfig,
        device_event::{DeviceEvent, DeviceEventState},
        raw_device_event::{RawDeviceEvent, RawDeviceId},
        window_event::{WindowEvent, WindowEventState},
    },
    window::{WindowConfig, WindowError, WindowId, WindowSurface},
};

// ---------------------------------------------------------------------------
// Per-window state machines
// ---------------------------------------------------------------------------

pub(crate) struct PerWindowState {
    pub device: DeviceEventState,
    pub window: WindowEventState,
}

impl PerWindowState {
    fn new(config: &EventStateConfig) -> Self {
        Self {
            device: DeviceEventState::new(config.mouse)
                .expect("EventStateConfig passed to PerWindowState::new must be valid"),
            window: WindowEventState::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// Adapter
// ---------------------------------------------------------------------------

pub struct Adapter<App: Application> {
    runtime: crate::runtime::Runtime,

    rendering_window: HashMap<WindowId, crate::runtime::JoinHandle>,

    /// Per-window event state machines, keyed by WindowId.
    /// Created lazily on the first event for a window;
    /// removed when `WindowEvent::Destroyed` is received.
    window_states: HashMap<WindowId, PerWindowState>,

    /// Configuration applied to every new per-window state machine.
    event_config: EventStateConfig,

    app: Arc<App>,
}

/// Construction
impl<App: Application> Adapter<App> {
    pub fn new(app: App) -> Self {
        Self::with_event_config(app, EventStateConfig::default())
    }

    pub fn with_event_config(app: App, event_config: EventStateConfig) -> Self {
        Self::with_runtime_and_event_config(app, crate::runtime::Runtime::new(), event_config)
    }

    fn with_runtime_and_event_config(
        app: App,
        runtime: crate::runtime::Runtime,
        event_config: EventStateConfig,
    ) -> Self {
        Self {
            runtime,
            rendering_window: HashMap::new(),
            window_states: HashMap::new(),
            event_config,
            app: Arc::new(app),
        }
    }

    /// Builds an adapter on an externally created Tokio runtime (native only).
    #[cfg(not(web))]
    pub fn with_tokio_runtime(app: App, runtime: tokio::runtime::Runtime) -> Self {
        Self::with_tokio_runtime_and_event_config(app, runtime, EventStateConfig::default())
    }

    /// Builds an adapter on an externally created Tokio runtime (native only).
    #[cfg(not(web))]
    pub fn with_tokio_runtime_and_event_config(
        app: App,
        runtime: tokio::runtime::Runtime,
        event_config: EventStateConfig,
    ) -> Self {
        Self::with_runtime_and_event_config(
            app,
            crate::runtime::Runtime::from_tokio(runtime),
            event_config,
        )
    }
}

/// Running and setup
impl<App: Application> Adapter<App> {
    #[cfg(all(feature = "winit", not(target_arch = "wasm32")))]
    pub fn run(self) -> Result<(), winit::error::EventLoopError> {
        crate::winit_interface::run(self)
    }

    /// On wasm the browser owns the event loop, so `run` returns immediately
    /// after handing the adapter off to winit.
    #[cfg(all(feature = "winit", target_arch = "wasm32"))]
    pub fn run(self) {
        crate::winit_interface::run(self)
    }

    #[cfg(feature = "baseview")]
    pub fn run(self) -> () {
        unimplemented!("baseview support is not implemented yet")
    }
}

/// Lifecycle events
impl<App: Application> Adapter<App> {
    /// Called exactly once at `StartCause::Init`.
    ///
    /// `Arc::get_mut` is guaranteed to succeed here because no rendering tasks
    /// have been spawned yet and no other `Arc` clones exist.
    pub fn init(
        &mut self,
        proxy: Box<dyn EventLoopProxy<App>>,
        event_loop: &impl EventLoop,
    ) {
        let app = Arc::get_mut(&mut self.app)
            .expect("Adapter::init must be called before any Arc clones are created");
        let _guard = self.runtime.enter();
        app.init(self.runtime.handle(), proxy, event_loop);
    }

    pub fn resumed(&mut self, event_loop: &impl EventLoop) {
        let _guard = self.runtime.enter();
        self.app.resumed(self.runtime.handle(), event_loop);
    }

    pub fn create_surface(&mut self, event_loop: &impl EventLoop) {
        let _guard = self.runtime.enter();
        self.app
            .create_surface(self.runtime.handle(), event_loop);
    }

    pub fn destroy_surface(&mut self, event_loop: &impl EventLoop) {
        // ensure all rendering tasks are finished
        self.abort_all_rendering_tasks();

        let _guard = self.runtime.enter();
        self.app
            .destroy_surface(self.runtime.handle(), event_loop);
    }

    pub fn suspended(&mut self, event_loop: &impl EventLoop) {
        let _guard = self.runtime.enter();
        self.app.suspended(self.runtime.handle(), event_loop);
    }

    pub fn exiting(&mut self, event_loop: &impl EventLoop) {
        let _guard = self.runtime.enter();
        self.app.exiting(self.runtime.handle(), event_loop);
    }
}

/// Events
impl<App: Application> Adapter<App> {
    pub fn render(&mut self, window_id: WindowId) {
        if let Some(handle) = self.rendering_window.get(&window_id) {
            if handle.is_finished() {
                self.rendering_window.remove(&window_id);
            } else {
                // request redraw again to catch up latest redraw request
                self.app
                    .request_redraw(self.runtime.handle(), window_id);
                return;
            }
        }

        let app = self.app.clone();
        let runtime_handle = self.runtime.handle().clone();

        let handle = self.runtime.handle().spawn(async move {
            app.render(&runtime_handle, window_id).await;
        });

        self.rendering_window.insert(window_id, handle);
    }

    pub fn window_event(
        &mut self,
        event_loop: &impl EventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let _guard = self.runtime.enter();
        let event = self.window_state_mut(window_id).window.process(event);
        self.app
            .window_event(self.runtime.handle(), event_loop, window_id, event);
    }

    pub fn window_destroyed(&mut self, event_loop: &impl EventLoop, window_id: WindowId) {
        // Clean up the per-window state machine so it doesn't outlive the window.
        self.remove_window_state(window_id);
        // Clean up the rendering task for the window.
        self.remove_rendering_task(window_id);
        // Notify the Application that the window is gone.
        let _guard = self.runtime.enter();
        self.app
            .window_destroyed(self.runtime.handle(), event_loop, window_id);
    }

    pub fn device_event(
        &mut self,
        event_loop: &impl EventLoop,
        window_id: WindowId,
        event: DeviceEvent,
    ) {
        if let Some(processed) = self.window_state_mut(window_id).device.process(event) {
            let _guard = self.runtime.enter();
            self.app.device_event(
                self.runtime.handle(),
                event_loop,
                window_id,
                processed,
            );
        }
    }

    pub fn raw_device_event(
        &mut self,
        event_loop: &impl EventLoop,
        raw_device_id: RawDeviceId,
        raw_event: RawDeviceEvent,
    ) {
        let _guard = self.runtime.enter();
        self.app.raw_device_event(
            self.runtime.handle(),
            event_loop,
            raw_device_id,
            raw_event,
        );
    }
}

/// Ui commands
impl<App: Application> Adapter<App> {
    pub fn ui_command(&mut self, event_loop: &impl EventLoop, command: App::Command) {
        let _guard = self.runtime.enter();
        self.app
            .ui_command(self.runtime.handle(), event_loop, command);
    }
}

/// Polling
impl<App: Application> Adapter<App> {
    pub fn poll(&mut self, event_loop: &impl EventLoop) {
        let _guard = self.runtime.enter();
        self.app.poll(self.runtime.handle(), event_loop);
    }

    pub fn resume_time_reached(
        &mut self,
        event_loop: &impl EventLoop,
        start: web_time::Instant,
        requested_resume: web_time::Instant,
    ) {
        let _guard = self.runtime.enter();
        self.app.resume_time_reached(
            self.runtime.handle(),
            event_loop,
            start,
            requested_resume,
        );
    }

    pub fn wait_cancelled(
        &mut self,
        event_loop: &impl EventLoop,
        start: web_time::Instant,
        requested_resume: Option<web_time::Instant>,
    ) {
        let _guard = self.runtime.enter();
        self.app.wait_cancelled(
            self.runtime.handle(),
            event_loop,
            start,
            requested_resume,
        );
    }

    pub fn about_to_wait(&mut self, event_loop: &impl EventLoop) {
        let _guard = self.runtime.enter();
        self.app
            .about_to_wait(self.runtime.handle(), event_loop);
    }
}

impl<App: Application> Adapter<App> {
    pub fn memory_warning(&mut self, event_loop: &impl EventLoop) {
        let _guard = self.runtime.enter();
        self.app
            .memory_warning(self.runtime.handle(), event_loop);
    }
}

// -------------------
// Helpers
// -------------------

impl<App: Application> Adapter<App> {
    fn abort_all_rendering_tasks(&mut self) {
        let handles: Vec<_> = self.rendering_window.drain().map(|(_, h)| h).collect();
        self.runtime.abort_and_join(handles);
    }

    fn remove_rendering_task(&mut self, window_id: WindowId) {
        if let Some(handle) = self.rendering_window.get(&window_id) {
            handle.abort();
            self.rendering_window.remove(&window_id);
        }
    }
}

/// Per-window state machine access
impl<App: Application> Adapter<App> {
    /// Returns a mutable reference to the state machine for `id`,
    /// creating it with the stored `event_config` if it doesn't exist yet.
    fn window_state_mut(&mut self, id: WindowId) -> &mut PerWindowState {
        let config = self.event_config; // EventStateConfig is Copy
        self.window_states
            .entry(id)
            .or_insert_with(|| PerWindowState::new(&config))
    }

    /// Removes the state machine for `id`.
    /// Called when winit fires `WindowEvent::Destroyed`.
    fn remove_window_state(&mut self, id: WindowId) {
        self.window_states.remove(&id);
    }
}

// -------------------
// API type definition
// -------------------

pub trait EventLoop {
    /// Creates the native window only. The wgpu surface is not yet attached.
    /// Call [`Window::create_surface`](crate::window::Window::create_surface) separately.
    fn create_window(&self, config: &WindowConfig) -> Result<WindowSurface, WindowError>;

    // Todo: fn create_custom_cursor(&self) -> CustomCursor;
    // Todo: fn available_monitors(&self) -> impl Iterator<Item = MonitorHandle>;
    // Todo: fn primary_monitor(&self) -> Option<MonitorHandle>;
    // Todo: fn listen_device_events(&self, allowed: DeviceEvents);
    // Todo: fn system_theme(&self) -> Option<Theme>

    fn set_control_flow(&self, control_flow: ControlFlow);
    fn control_flow(&self) -> ControlFlow;
    fn exit(&self);
    fn exiting(&self) -> bool;
}

pub enum EventLoopCommand {
    Exit,
    SetControlFlow(ControlFlow),
}

pub enum ControlFlow {
    Wait,
    Poll,
    WaitUntil(web_time::Instant),
}

pub trait EventLoopProxy<App: Application>: utils::MaybeSend {
    fn clone_box(&self) -> Box<dyn EventLoopProxy<App>>;
    fn send_command(&self, command: App::Command);
    fn request_exit(&self);
    fn request_control_flow(&self, control_flow: ControlFlow);
}
