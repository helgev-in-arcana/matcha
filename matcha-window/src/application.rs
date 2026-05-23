use crate::{
    adapter::{EventLoop, EventLoopProxy},
    event::{
        device_event::DeviceEvent,
        raw_device_event::{RawDeviceEvent, RawDeviceId},
        window_event::WindowEvent,
    },
    window::WindowId,
};

pub trait Application: utils::MaybeSendSync + 'static {
    type Command: utils::MaybeSend + 'static;

    // lifecycle methods
    fn init(
        &mut self,
        proxy: Box<dyn EventLoopProxy<Self>>,
        event_loop: &impl EventLoop,
    );
    fn resumed(&self, event_loop: &impl EventLoop);
    fn create_surface(&self, event_loop: &impl EventLoop);
    fn destroy_surface(&self, event_loop: &impl EventLoop);
    fn suspended(&self, event_loop: &impl EventLoop);
    fn exiting(&self, event_loop: &impl EventLoop);

    // rendering
    fn render(&self, window_id: WindowId);

    // event methods
    fn window_event(
        &self,
        event_loop: &impl EventLoop,
        window_id: WindowId,
        event: WindowEvent,
    );
    fn window_destroyed(
        &self,
        event_loop: &impl EventLoop,
        window_id: WindowId,
    );
    fn device_event(
        &self,
        event_loop: &impl EventLoop,
        window_id: WindowId,
        event: DeviceEvent,
    );
    fn ui_command(
        &self,
        event_loop: &impl EventLoop,
        command: Self::Command,
    );

    // Default Methods
    fn raw_device_event(
        &self,
        event_loop: &impl EventLoop,
        raw_device_id: RawDeviceId,
        raw_event: RawDeviceEvent,
    ) {
        let _ = event_loop;
        let _ = raw_device_id;
        let _ = raw_event;
    }
    fn poll(&self, event_loop: &impl EventLoop) {
        let _ = event_loop;
    }
    fn resume_time_reached(
        &self,
        event_loop: &impl EventLoop,
        start: web_time::Instant,
        requested_resume: web_time::Instant,
    ) {
        let _ = event_loop;
        let _ = start;
        let _ = requested_resume;
    }
    fn wait_cancelled(
        &self,
        event_loop: &impl EventLoop,
        start: web_time::Instant,
        requested_resume: Option<web_time::Instant>,
    ) {
        let _ = event_loop;
        let _ = start;
        let _ = requested_resume;
    }
    fn about_to_wait(&self, event_loop: &impl EventLoop) {
        let _ = event_loop;
    }
    fn memory_warning(&self, event_loop: &impl EventLoop) {
        let _ = event_loop;
    }
}
