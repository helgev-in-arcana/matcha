use crate::{
    RuntimeHandle,
    adapter::{EventLoop, EventLoopProxy},
    event::{
        device_event::DeviceEvent,
        raw_device_event::{RawDeviceEvent, RawDeviceId},
        window_event::WindowEvent,
    },
    window::WindowId,
};

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub trait Application: Send + Sync + 'static {
    type Command: Send + 'static;

    // lifecycle methods
    fn init(
        &mut self,
        runtime: &RuntimeHandle,
        proxy: Box<dyn EventLoopProxy<Self> + Send>,
        event_loop: &impl EventLoop,
    );
    fn resumed(&self, runtime: &RuntimeHandle, event_loop: &impl EventLoop);
    fn create_surface(&self, runtime: &RuntimeHandle, event_loop: &impl EventLoop);
    fn destroy_surface(&self, runtime: &RuntimeHandle, event_loop: &impl EventLoop);
    fn suspended(&self, runtime: &RuntimeHandle, event_loop: &impl EventLoop);
    fn exiting(&self, runtime: &RuntimeHandle, event_loop: &impl EventLoop);

    // rendering methods — no event_loop, spawnable in parallel
    async fn render(&self, runtime: &RuntimeHandle, window_id: WindowId);
    fn request_redraw(&self, runtime: &RuntimeHandle, window_id: WindowId) {
        let _ = runtime;
        let _ = window_id;
    }

    // event methods
    fn window_event(
        &self,
        runtime: &RuntimeHandle,
        event_loop: &impl EventLoop,
        window_id: WindowId,
        event: WindowEvent,
    );
    fn window_destroyed(
        &self,
        runtime: &RuntimeHandle,
        event_loop: &impl EventLoop,
        window_id: WindowId,
    );
    fn device_event(
        &self,
        runtime: &RuntimeHandle,
        event_loop: &impl EventLoop,
        window_id: WindowId,
        event: DeviceEvent,
    );
    fn ui_command(
        &self,
        runtime: &RuntimeHandle,
        event_loop: &impl EventLoop,
        command: Self::Command,
    );

    // Default Methods
    fn raw_device_event(
        &self,
        runtime: &RuntimeHandle,
        event_loop: &impl EventLoop,
        raw_device_id: RawDeviceId,
        raw_event: RawDeviceEvent,
    ) {
        let _ = runtime;
        let _ = event_loop;
        let _ = raw_device_id;
        let _ = raw_event;
    }
    fn poll(&self, runtime: &RuntimeHandle, event_loop: &impl EventLoop) {
        let _ = runtime;
        let _ = event_loop;
    }
    fn resume_time_reached(
        &self,
        runtime: &RuntimeHandle,
        event_loop: &impl EventLoop,
        start: std::time::Instant,
        requested_resume: std::time::Instant,
    ) {
        let _ = runtime;
        let _ = event_loop;
        let _ = start;
        let _ = requested_resume;
    }
    fn wait_cancelled(
        &self,
        runtime: &RuntimeHandle,
        event_loop: &impl EventLoop,
        start: std::time::Instant,
        requested_resume: Option<std::time::Instant>,
    ) {
        let _ = runtime;
        let _ = event_loop;
        let _ = start;
        let _ = requested_resume;
    }
    fn about_to_wait(&self, runtime: &RuntimeHandle, event_loop: &impl EventLoop) {
        let _ = runtime;
        let _ = event_loop;
    }
    fn memory_warning(&self, runtime: &RuntimeHandle, event_loop: &impl EventLoop) {
        let _ = runtime;
        let _ = event_loop;
    }
}
