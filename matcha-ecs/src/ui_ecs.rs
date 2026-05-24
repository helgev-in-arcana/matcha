use std::collections::HashMap;

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

mod window_thread;

pub struct UiEcs {
    world: bevy_ecs::world::World,
    scheduler: bevy_ecs::schedule::Schedule,
}

impl Application for UiEcs {
    type Command = ();

    fn init(
        &mut self,
        proxy: Box<dyn EventLoopProxy<Self>>,
        event_loop: &impl EventLoop,
    ) {
        todo!()
    }

    fn resumed(&self, event_loop: &impl EventLoop) {
        todo!()
    }

    fn create_surface(&self, event_loop: &impl EventLoop) {
        todo!()
    }

    fn destroy_surface(&self, event_loop: &impl EventLoop) {
        todo!()
    }

    fn suspended(&self, event_loop: &impl EventLoop) {
        todo!()
    }

    fn exiting(&self, event_loop: &impl EventLoop) {
        todo!()
    }

    fn render(&self, window_id: WindowId) {
        todo!()
    }

    fn window_event(
        &self,
        event_loop: &impl EventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        todo!()
    }

    fn window_destroyed(
        &self,
        event_loop: &impl EventLoop,
        window_id: WindowId,
    ) {
        todo!()
    }

    fn device_event(
        &self,
        event_loop: &impl EventLoop,
        window_id: WindowId,
        event: DeviceEvent,
    ) {
        todo!()
    }

    fn ui_command(
        &self,
        event_loop: &impl EventLoop,
        command: Self::Command,
    ) {
        todo!()
    }
}
