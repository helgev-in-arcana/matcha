//! Headless event loop and proxy for driving an [`Application`] in tests.
//!
//! There is deliberately no headless `Adapter::run`: the test itself acts as
//! the event loop, calling [`crate::adapter::Adapter`]'s public methods
//! (`init` / `resumed` / `create_surface` / `render` / `device_event` /
//! `ui_command`, ...) in the same order the winit interface would. Commands
//! sent through [`HeadlessEventLoopProxy`] are queued instead of waking an OS
//! loop; the test drains them and feeds them back into `Adapter::ui_command`.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::{
    adapter::{ControlFlow, EventLoop, EventLoopCommand, EventLoopProxy},
    application::Application,
    window::{WindowConfig, WindowError, WindowId, WindowSurface},
};

// ---------------------------------------------------------------------------
// HeadlessEventLoop
// ---------------------------------------------------------------------------

/// [`EventLoop`] implementation that creates in-memory windows and records
/// state instead of talking to an OS.
pub struct HeadlessEventLoop {
    created: parking_lot::Mutex<Vec<WindowId>>,
    control_flow: parking_lot::Mutex<ControlFlow>,
    exit_requested: AtomicBool,
}

impl HeadlessEventLoop {
    pub fn new() -> Self {
        Self {
            created: parking_lot::Mutex::new(Vec::new()),
            control_flow: parking_lot::Mutex::new(ControlFlow::Wait),
            exit_requested: AtomicBool::new(false),
        }
    }

    /// Ids of every window created through this loop, in creation order.
    pub fn created_window_ids(&self) -> Vec<WindowId> {
        self.created.lock().clone()
    }
}

impl Default for HeadlessEventLoop {
    fn default() -> Self {
        Self::new()
    }
}

impl EventLoop for HeadlessEventLoop {
    fn create_window(&self, config: &WindowConfig) -> Result<WindowSurface, WindowError> {
        let window_surface = WindowSurface::new_headless(config);
        self.created.lock().push(window_surface.id());
        Ok(window_surface)
    }

    fn set_control_flow(&self, control_flow: ControlFlow) {
        *self.control_flow.lock() = control_flow;
    }

    fn control_flow(&self) -> ControlFlow {
        *self.control_flow.lock()
    }

    fn exit(&self) {
        self.exit_requested.store(true, Ordering::Relaxed);
    }

    fn exiting(&self) -> bool {
        self.exit_requested.load(Ordering::Relaxed)
    }
}

// ---------------------------------------------------------------------------
// HeadlessEventLoopProxy
// ---------------------------------------------------------------------------

/// [`EventLoopProxy`] implementation backed by shared queues. `clone_box`
/// shares the queues, so commands sent through a proxy handed to the app are
/// observable on the original held by the test.
pub struct HeadlessEventLoopProxy<App: Application> {
    commands: Arc<parking_lot::Mutex<VecDeque<App::Command>>>,
    loop_commands: Arc<parking_lot::Mutex<VecDeque<EventLoopCommand>>>,
}

impl<App: Application> HeadlessEventLoopProxy<App> {
    pub fn new() -> Self {
        Self {
            commands: Arc::new(parking_lot::Mutex::new(VecDeque::new())),
            loop_commands: Arc::new(parking_lot::Mutex::new(VecDeque::new())),
        }
    }

    /// Drains queued app commands. The test feeds them back into
    /// [`crate::adapter::Adapter::ui_command`] to emulate the delivery a real
    /// event loop would perform.
    pub fn drain_commands(&self) -> Vec<App::Command> {
        self.commands.lock().drain(..).collect()
    }

    /// Drains queued event-loop commands (`Exit` / `SetControlFlow`).
    pub fn drain_loop_commands(&self) -> Vec<EventLoopCommand> {
        self.loop_commands.lock().drain(..).collect()
    }
}

impl<App: Application> Default for HeadlessEventLoopProxy<App> {
    fn default() -> Self {
        Self::new()
    }
}

impl<App: Application> Clone for HeadlessEventLoopProxy<App> {
    fn clone(&self) -> Self {
        Self {
            commands: Arc::clone(&self.commands),
            loop_commands: Arc::clone(&self.loop_commands),
        }
    }
}

impl<App: Application> EventLoopProxy<App> for HeadlessEventLoopProxy<App> {
    fn clone_box(&self) -> Box<dyn EventLoopProxy<App>> {
        Box::new(self.clone())
    }

    fn send_command(&self, command: App::Command) {
        self.commands.lock().push_back(command);
    }

    fn request_exit(&self) {
        self.loop_commands.lock().push_back(EventLoopCommand::Exit);
    }

    fn request_control_flow(&self, control_flow: ControlFlow) {
        self.loop_commands
            .lock()
            .push_back(EventLoopCommand::SetControlFlow(control_flow));
    }
}
