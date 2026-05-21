use crate::{
    adapter::{EventLoop, EventLoopProxy},
    event::{
        device_event::DeviceEvent,
        raw_device_event::{RawDeviceEvent, RawDeviceId},
        window_event::WindowEvent,
    },
    window::WindowId,
};

#[async_trait::async_trait]
pub trait Application: Send + Sync + 'static {
    type Command: Send + 'static;

    // lifecycle methods
    fn init(
        &mut self,
        runtime: &tokio::runtime::Handle,
        proxy: Box<dyn EventLoopProxy<Self> + Send>,
        event_loop: &impl EventLoop,
    );
    fn resumed(&self, runtime: &tokio::runtime::Handle, event_loop: &impl EventLoop);
    fn create_surface(&self, runtime: &tokio::runtime::Handle, event_loop: &impl EventLoop);
    fn destroy_surface(&self, runtime: &tokio::runtime::Handle, event_loop: &impl EventLoop);
    fn suspended(&self, runtime: &tokio::runtime::Handle, event_loop: &impl EventLoop);
    fn exiting(&self, runtime: &tokio::runtime::Handle, event_loop: &impl EventLoop);

    // rendering methods — no event_loop, spawnable in parallel
    async fn render(&self, runtime: &tokio::runtime::Handle, window_id: WindowId);
    fn request_redraw(&self, runtime: &tokio::runtime::Handle, window_id: WindowId) {
        let _ = runtime;
        let _ = window_id;
    }

    // event methods
    fn window_event(
        &self,
        runtime: &tokio::runtime::Handle,
        event_loop: &impl EventLoop,
        window_id: WindowId,
        event: WindowEvent,
    );
    fn window_destroyed(
        &self,
        runtime: &tokio::runtime::Handle,
        event_loop: &impl EventLoop,
        window_id: WindowId,
    );
    fn device_event(
        &self,
        runtime: &tokio::runtime::Handle,
        event_loop: &impl EventLoop,
        window_id: WindowId,
        event: DeviceEvent,
    );
    fn ui_command(
        &self,
        runtime: &tokio::runtime::Handle,
        event_loop: &impl EventLoop,
        command: Self::Command,
    );

    // Default Methods
    fn raw_device_event(
        &self,
        runtime: &tokio::runtime::Handle,
        event_loop: &impl EventLoop,
        raw_device_id: RawDeviceId,
        raw_event: RawDeviceEvent,
    ) {
        let _ = runtime;
        let _ = event_loop;
        let _ = raw_device_id;
        let _ = raw_event;
    }
    fn poll(&self, runtime: &tokio::runtime::Handle, event_loop: &impl EventLoop) {
        let _ = runtime;
        let _ = event_loop;
    }
    fn resume_time_reached(
        &self,
        runtime: &tokio::runtime::Handle,
        event_loop: &impl EventLoop,
        start: web_time::Instant,
        requested_resume: web_time::Instant,
    ) {
        let _ = runtime;
        let _ = event_loop;
        let _ = start;
        let _ = requested_resume;
    }
    fn wait_cancelled(
        &self,
        runtime: &tokio::runtime::Handle,
        event_loop: &impl EventLoop,
        start: web_time::Instant,
        requested_resume: Option<web_time::Instant>,
    ) {
        let _ = runtime;
        let _ = event_loop;
        let _ = start;
        let _ = requested_resume;
    }
    fn about_to_wait(&self, runtime: &tokio::runtime::Handle, event_loop: &impl EventLoop) {
        let _ = runtime;
        let _ = event_loop;
    }
    fn memory_warning(&self, runtime: &tokio::runtime::Handle, event_loop: &impl EventLoop) {
        let _ = runtime;
        let _ = event_loop;
    }
}
