use matcha_window::{
    adapter::{EventLoop, EventLoopProxy},
    application::Application,
    event::{
        device_event::DeviceEvent,
        raw_device_event::{RawDeviceEvent, RawDeviceId},
        window_event::WindowEvent,
    },
    window::WindowId,
};

pub mod widgets;
pub mod components;
pub mod systems;

pub struct UiEcs {
    world: bevy_ecs::world::World,
    scheduler: bevy_ecs::schedule::Schedule,
}

#[async_trait::async_trait]
impl Application for UiEcs {
    type Command = ();

    fn init(
        &mut self,
        runtime: &tokio::runtime::Handle,
        proxy: Box<dyn EventLoopProxy<Self> + Send>,
        event_loop: &impl EventLoop,
    ) {
        todo!()
    }

    fn resumed(&self, runtime: &tokio::runtime::Handle, event_loop: &impl EventLoop) {
        todo!()
    }

    fn create_surface(&self, runtime: &tokio::runtime::Handle, event_loop: &impl EventLoop) {
        todo!()
    }

    fn destroy_surface(&self, runtime: &tokio::runtime::Handle, event_loop: &impl EventLoop) {
        todo!()
    }

    fn suspended(&self, runtime: &tokio::runtime::Handle, event_loop: &impl EventLoop) {
        todo!()
    }

    fn exiting(&self, runtime: &tokio::runtime::Handle, event_loop: &impl EventLoop) {
        todo!()
    }

    async fn render(&self, runtime: &tokio::runtime::Handle, window_id: WindowId) {
        todo!()
    }

    fn window_event(
        &self,
        runtime: &tokio::runtime::Handle,
        event_loop: &impl EventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        todo!()
    }

    fn window_destroyed(
        &self,
        runtime: &tokio::runtime::Handle,
        event_loop: &impl EventLoop,
        window_id: WindowId,
    ) {
        todo!()
    }

    fn device_event(
        &self,
        runtime: &tokio::runtime::Handle,
        event_loop: &impl EventLoop,
        window_id: WindowId,
        event: DeviceEvent,
    ) {
        todo!()
    }

    fn ui_command(
        &self,
        runtime: &tokio::runtime::Handle,
        event_loop: &impl EventLoop,
        command: Self::Command,
    ) {
        todo!()
    }

    fn request_redraw(&self, runtime: &tokio::runtime::Handle, window_id: WindowId) {
        let _ = runtime;
        let _ = window_id;
    }

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
        runtime: &tokio::runtime::Handle,
        event_loop: &impl EventLoop,
        start: std::time::Instant,
        requested_resume: Option<std::time::Instant>,
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
