use std::collections::HashMap;

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
    app: App,

    /// Configuration applied to every new per-window state machine.
    event_config: EventStateConfig,

    /// Per-window event state machines, keyed by WindowId.
    /// Created lazily on the first event for a window;
    /// removed when `WindowEvent::Destroyed` is received.
    window_states: HashMap<WindowId, PerWindowState>,
}

/// Construction
impl<App: Application> Adapter<App> {
    pub fn new(app: App) -> Self {
        Self::with_event_config(app, EventStateConfig::default())
    }

    pub fn with_event_config(app: App, event_config: EventStateConfig) -> Self {
        Self {
            window_states: HashMap::new(),
            event_config,
            app,
        }
    }

    /// The wrapped application (headless tests inspect it between the
    /// `Adapter` calls they drive manually).
    pub fn app(&self) -> &App {
        &self.app
    }
}

// Platform-specific impls (`run`).
#[cfg(not(web))]
#[path = "adapter/native.rs"]
mod platform;
#[cfg(web)]
#[path = "adapter/web.rs"]
mod platform;

/// Lifecycle events
impl<App: Application> Adapter<App> {
    pub fn init(&mut self, proxy: Box<dyn EventLoopProxy<App>>, event_loop: &impl EventLoop) {
        self.app.init(proxy, event_loop);
    }

    pub fn resumed(&mut self, event_loop: &impl EventLoop) {
        self.app.resumed(event_loop);
    }

    pub fn create_surface(&mut self, event_loop: &impl EventLoop) {
        self.app.create_surface(event_loop);
    }

    pub fn destroy_surface(&mut self, event_loop: &impl EventLoop) {
        self.app.destroy_surface(event_loop);
    }

    pub fn suspended(&mut self, event_loop: &impl EventLoop) {
        self.app.suspended(event_loop);
    }

    pub fn exiting(&mut self, event_loop: &impl EventLoop) {
        self.app.exiting(event_loop);
    }
}

/// Events
impl<App: Application> Adapter<App> {
    pub fn render(&mut self, window_id: WindowId) {
        self.app.render(window_id);
    }

    pub fn window_event(
        &mut self,
        event_loop: &impl EventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let event = self.window_state_mut(window_id).window.process(event);
        self.app.window_event(event_loop, window_id, event);
    }

    pub fn window_destroyed(&mut self, event_loop: &impl EventLoop, window_id: WindowId) {
        self.remove_window_state(window_id);
        self.app.window_destroyed(event_loop, window_id);
    }

    pub fn device_event(
        &mut self,
        event_loop: &impl EventLoop,
        window_id: WindowId,
        event: DeviceEvent,
    ) {
        if let Some(processed) = self.window_state_mut(window_id).device.process(event) {
            self.app.device_event(event_loop, window_id, processed);
        }
    }

    pub fn raw_device_event(
        &mut self,
        event_loop: &impl EventLoop,
        raw_device_id: RawDeviceId,
        raw_event: RawDeviceEvent,
    ) {
        self.app.raw_device_event(event_loop, raw_device_id, raw_event);
    }
}

/// Ui commands
impl<App: Application> Adapter<App> {
    pub fn ui_command(&mut self, event_loop: &impl EventLoop, command: App::Command) {
        self.app.ui_command(event_loop, command);
    }
}

/// Polling
impl<App: Application> Adapter<App> {
    pub fn poll(&mut self, event_loop: &impl EventLoop) {
        self.app.poll(event_loop);
    }

    pub fn resume_time_reached(
        &mut self,
        event_loop: &impl EventLoop,
        start: web_time::Instant,
        requested_resume: web_time::Instant,
    ) {
        self.app.resume_time_reached(event_loop, start, requested_resume);
    }

    pub fn wait_cancelled(
        &mut self,
        event_loop: &impl EventLoop,
        start: web_time::Instant,
        requested_resume: Option<web_time::Instant>,
    ) {
        self.app.wait_cancelled(event_loop, start, requested_resume);
    }

    pub fn about_to_wait(&mut self, event_loop: &impl EventLoop) {
        self.app.about_to_wait(event_loop);
    }
}

impl<App: Application> Adapter<App> {
    pub fn memory_warning(&mut self, event_loop: &impl EventLoop) {
        self.app.memory_warning(event_loop);
    }
}

// -------------------
// Helpers
// -------------------

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

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ControlFlow {
    Wait,
    Poll,
    WaitUntil(web_time::Instant),
}

pub trait EventLoopProxy<App: Application>: utils::MaybeSendSync {
    fn clone_box(&self) -> Box<dyn EventLoopProxy<App>>;
    fn send_command(&self, command: App::Command);
    fn request_exit(&self);
    fn request_control_flow(&self, control_flow: ControlFlow);
}
