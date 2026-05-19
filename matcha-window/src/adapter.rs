use std::{collections::HashMap, sync::Arc};

use crate::{
    RuntimeHandle,
    application::Application,
    event::{
        EventStateConfig,
        device_event::{DeviceEvent, DeviceEventState},
        raw_device_event::{RawDeviceEvent, RawDeviceId},
        window_event::{WindowEvent, WindowEventState},
    },
    task_handle::TaskHandle,
    window::{WindowConfig, WindowError, WindowId, WindowSurface},
};

// ---------------------------------------------------------------------------
// Platform-specific runtime state
// ---------------------------------------------------------------------------

#[cfg(not(target_arch = "wasm32"))]
mod runtime_state {
    use super::{HashMap, TaskHandle, WindowId};

    pub(super) struct RuntimeState {
        tokio_runtime: tokio::runtime::Runtime,
    }

    impl RuntimeState {
        pub(super) fn new() -> Self {
            Self {
                tokio_runtime: tokio::runtime::Runtime::new().unwrap(),
            }
        }

        pub(super) fn with_runtime(tokio_runtime: tokio::runtime::Runtime) -> Self {
            Self { tokio_runtime }
        }

        pub(super) fn handle(&self) -> crate::RuntimeHandle {
            crate::RuntimeHandle::from_tokio(self.tokio_runtime.handle().clone())
        }

        pub(super) fn abort_all_tasks(
            &self,
            rendering_window: &mut HashMap<WindowId, TaskHandle<()>>,
        ) {
            self.tokio_runtime.block_on(async {
                for handle in rendering_window.values() {
                    handle.abort();
                }
                for (_, handle) in rendering_window.drain() {
                    let _ = handle.join().await;
                }
            });
        }
    }
}

#[cfg(target_arch = "wasm32")]
mod runtime_state {
    use super::{HashMap, TaskHandle, WindowId};

    pub(super) struct RuntimeState;

    impl RuntimeState {
        pub(super) fn new() -> Self {
            Self
        }

        pub(super) fn handle(&self) -> crate::RuntimeHandle {
            crate::RuntimeHandle
        }

        pub(super) fn abort_all_tasks(
            &self,
            rendering_window: &mut HashMap<WindowId, TaskHandle<()>>,
        ) {
            for handle in rendering_window.values() {
                handle.abort();
            }
            rendering_window.clear();
        }
    }
}

use runtime_state::RuntimeState;

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
    runtime: RuntimeState,
    rendering_window: HashMap<WindowId, TaskHandle<()>>,
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
        Self::with_runtime_and_event_config(RuntimeState::new(), app, EventStateConfig::default())
    }

    pub fn with_event_config(app: App, event_config: EventStateConfig) -> Self {
        Self::with_runtime_and_event_config(RuntimeState::new(), app, event_config)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn with_tokio_runtime(app: App, runtime: tokio::runtime::Runtime) -> Self {
        Self::with_runtime_and_event_config(
            RuntimeState::with_runtime(runtime),
            app,
            EventStateConfig::default(),
        )
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn with_tokio_runtime_and_event_config(
        app: App,
        runtime: tokio::runtime::Runtime,
        event_config: EventStateConfig,
    ) -> Self {
        Self::with_runtime_and_event_config(RuntimeState::with_runtime(runtime), app, event_config)
    }

    fn with_runtime_and_event_config(
        runtime: RuntimeState,
        app: App,
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
}

/// Running and setup
impl<App: Application> Adapter<App> {
    #[cfg(feature = "winit")]
    pub fn run(self) -> Result<(), winit::error::EventLoopError> {
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
        proxy: Box<dyn EventLoopProxy<App> + Send>,
        event_loop: &impl EventLoop,
    ) {
        let runtime = self.runtime_handle();
        let _guard = runtime.enter();
        let app = Arc::get_mut(&mut self.app)
            .expect("Adapter::init must be called before any Arc clones are created");
        app.init(&runtime, proxy, event_loop);
    }

    pub fn resumed(&mut self, event_loop: &impl EventLoop) {
        let runtime = self.runtime_handle();
        let _guard = runtime.enter();
        self.app.resumed(&runtime, event_loop);
    }

    pub fn create_surface(&mut self, event_loop: &impl EventLoop) {
        let runtime = self.runtime_handle();
        let _guard = runtime.enter();
        self.app.create_surface(&runtime, event_loop);
    }

    pub fn destroy_surface(&mut self, event_loop: &impl EventLoop) {
        // ensure all rendering tasks are finished
        self.abort_all_rendering_tasks();

        let runtime = self.runtime_handle();
        let _guard = runtime.enter();
        self.app.destroy_surface(&runtime, event_loop);
    }

    pub fn suspended(&mut self, event_loop: &impl EventLoop) {
        let runtime = self.runtime_handle();
        let _guard = runtime.enter();
        self.app.suspended(&runtime, event_loop);
    }

    pub fn exiting(&mut self, event_loop: &impl EventLoop) {
        let runtime = self.runtime_handle();
        let _guard = runtime.enter();
        self.app.exiting(&runtime, event_loop);
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
                self.app.request_redraw(&self.runtime_handle(), window_id);
                return;
            }
        }

        let app = self.app.clone();
        let runtime_handle = self.runtime_handle();
        let runtime_in_task = runtime_handle.clone();

        let handle = runtime_handle.spawn(async move {
            app.render(&runtime_in_task, window_id).await;
        });

        self.rendering_window.insert(window_id, handle);
    }

    pub fn window_event(
        &mut self,
        event_loop: &impl EventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let runtime = self.runtime_handle();
        let _guard = runtime.enter();
        let event = self.window_state_mut(window_id).window.process(event);
        self.app.window_event(&runtime, event_loop, window_id, event);
    }

    pub fn window_destroyed(&mut self, event_loop: &impl EventLoop, window_id: WindowId) {
        // Clean up the per-window state machine so it doesn't outlive the window.
        self.remove_window_state(window_id);
        // Clean up the rendering task for the window.
        self.remove_rendering_task(window_id);
        // Notify the Application that the window is gone.
        let runtime = self.runtime_handle();
        let _guard = runtime.enter();
        self.app.window_destroyed(&runtime, event_loop, window_id);
    }

    pub fn device_event(
        &mut self,
        event_loop: &impl EventLoop,
        window_id: WindowId,
        event: DeviceEvent,
    ) {
        if let Some(processed) = self.window_state_mut(window_id).device.process(event) {
            let runtime = self.runtime_handle();
            let _guard = runtime.enter();
            self.app
                .device_event(&runtime, event_loop, window_id, processed);
        }
    }

    pub fn raw_device_event(
        &mut self,
        event_loop: &impl EventLoop,
        raw_device_id: RawDeviceId,
        raw_event: RawDeviceEvent,
    ) {
        let runtime = self.runtime_handle();
        let _guard = runtime.enter();
        self.app
            .raw_device_event(&runtime, event_loop, raw_device_id, raw_event);
    }
}

/// Ui commands
impl<App: Application> Adapter<App> {
    pub fn ui_command(&mut self, event_loop: &impl EventLoop, command: App::Command) {
        let runtime = self.runtime_handle();
        let _guard = runtime.enter();
        self.app.ui_command(&runtime, event_loop, command);
    }
}

/// Polling
impl<App: Application> Adapter<App> {
    pub fn poll(&mut self, event_loop: &impl EventLoop) {
        let runtime = self.runtime_handle();
        let _guard = runtime.enter();
        self.app.poll(&runtime, event_loop);
    }

    pub fn resume_time_reached(
        &mut self,
        event_loop: &impl EventLoop,
        start: web_time::Instant,
        requested_resume: web_time::Instant,
    ) {
        let runtime = self.runtime_handle();
        let _guard = runtime.enter();
        self.app
            .resume_time_reached(&runtime, event_loop, start, requested_resume);
    }

    pub fn wait_cancelled(
        &mut self,
        event_loop: &impl EventLoop,
        start: web_time::Instant,
        requested_resume: Option<web_time::Instant>,
    ) {
        let runtime = self.runtime_handle();
        let _guard = runtime.enter();
        self.app
            .wait_cancelled(&runtime, event_loop, start, requested_resume);
    }

    pub fn about_to_wait(&mut self, event_loop: &impl EventLoop) {
        let runtime = self.runtime_handle();
        let _guard = runtime.enter();
        self.app.about_to_wait(&runtime, event_loop);
    }
}

impl<App: Application> Adapter<App> {
    pub fn memory_warning(&mut self, event_loop: &impl EventLoop) {
        let runtime = self.runtime_handle();
        let _guard = runtime.enter();
        self.app.memory_warning(&runtime, event_loop);
    }
}

// -------------------
// Helpers
// -------------------

impl<App: Application> Adapter<App> {
    fn runtime_handle(&self) -> RuntimeHandle {
        self.runtime.handle()
    }

    fn abort_all_rendering_tasks(&mut self) {
        self.runtime.abort_all_tasks(&mut self.rendering_window);
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

pub trait EventLoopProxy<App: Application>: Send {
    fn clone_box(&self) -> Box<dyn EventLoopProxy<App> + Send>;
    fn send_command(&self, command: App::Command);
    fn request_exit(&self);
    fn request_control_flow(&self, control_flow: ControlFlow);
}
