use bevy_ecs::{
    resource::Resource,
    system::{Query, Res, ResMut},
};
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

use crate::{
    components::{layout::GlobalTransform, render::RenderItem, window::{Window, WindowBelonging}},
    resources::GpuResource,
};

#[derive(Resource)]
struct ProxyResource {
    proxy: Box<dyn EventLoopProxy<UiEcs>>,
}

#[derive(Resource)]
pub struct CanCreateSurface {
    pub flag: bool,
}

#[derive(Resource)]
struct RenderWindowId {
    id: WindowId,
}

pub struct UiEcs {
    world: bevy_ecs::world::World,

    // Update model execute view function update
    update_schedule: bevy_ecs::schedule::Schedule,

    // all animation systems include foreign systems
    pre_layout_schedule: bevy_ecs::schedule::Schedule,
    // all layout systems include foreign systems
    layout_schedule: bevy_ecs::schedule::Schedule,
    // Core system for rendering
    render_schedule: bevy_ecs::schedule::Schedule,
}

impl UiEcs {
    pub fn new() -> Self {
        Self {
            world: bevy_ecs::world::World::new(),
            update_schedule: bevy_ecs::schedule::Schedule::default(),
            pre_layout_schedule: bevy_ecs::schedule::Schedule::default(),
            layout_schedule: bevy_ecs::schedule::Schedule::default(),
            render_schedule: bevy_ecs::schedule::Schedule::default(),
        }
    }
}

impl Application for UiEcs {
    type Command = ();

    fn init(&mut self, proxy: Box<dyn EventLoopProxy<Self>>, event_loop: &impl EventLoop) {
        // Insert proxy
        self.world.insert_resource(ProxyResource { proxy });
    }

    fn resumed(&mut self, _event_loop: &impl EventLoop) {}

    fn create_surface(&mut self, event_loop: &impl EventLoop) {
        let _ = event_loop;

        if !self.world.contains_resource::<GpuResource>() {
            return;
        }

        let _ = self.world.run_system_cached(
            |mut q: Query<&mut Window>,
             gpu: Res<GpuResource>,
             mut can_create_surface: ResMut<CanCreateSurface>| {
                let gpu = &gpu.gpu;
                let (device, _) = gpu.context().unwrap();

                q.par_iter_mut().for_each(|mut window| {
                    window
                        .window
                        .create_surface(&gpu.instance(), &device)
                        .unwrap();
                });

                can_create_surface.flag = true;
            },
        );
    }

    fn destroy_surface(&mut self, event_loop: &impl EventLoop) {
        let _ = event_loop;

        let _ = self.world.run_system_cached(
            |mut q: Query<&mut Window>, mut can_create_surface: ResMut<CanCreateSurface>| {
                q.par_iter_mut().for_each(|mut window| {
                    window.window.destroy_surface();
                });

                can_create_surface.flag = false;
            },
        );
    }

    fn suspended(&mut self, event_loop: &impl EventLoop) {
        let _ = event_loop;
    }

    fn exiting(&mut self, event_loop: &impl EventLoop) {
        let _ = event_loop;
    }

    fn render(&mut self, window_id: WindowId) {
        self.world.insert_resource(RenderWindowId { id: window_id });

        self.pre_layout_schedule.run(&mut self.world);
        self.layout_schedule.run(&mut self.world);
        self.render_schedule.run(&mut self.world);

        // extract and exec render
        let _ = self.world.run_system_cached(
            |q: Query<(&WindowBelonging, &RenderItem, &GlobalTransform)>, window_id: Res<RenderWindowId>, gpu: Res<GpuResource>| {
                // filter by window id and collect render items

                // get gpu context and texture atlas

                // create command encoder

                // get or insert render item (make cache)

                // collect all render items for the window

                // give the render items to the renderer

                // submit the command buffer to the gpu
            },
        );
    }

    fn window_event(
        &mut self,
        event_loop: &impl EventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        self.world.insert_resource(RenderWindowId { id: window_id });

        todo!()
    }

    fn window_destroyed(&mut self, event_loop: &impl EventLoop, window_id: WindowId) {
        // remove all resources related to the window
        // currently not needed
    }

    fn device_event(
        &mut self,
        event_loop: &impl EventLoop,
        window_id: WindowId,
        event: DeviceEvent,
    ) {
        self.world.insert_resource(RenderWindowId { id: window_id });
    }

    fn ui_command(&mut self, event_loop: &impl EventLoop, command: Self::Command) {
        // todo
    }

    // ---------------------------------------------------
    // default implementations (currently not implemented)
    // ---------------------------------------------------

    fn raw_device_event(
        &mut self,
        event_loop: &impl EventLoop,
        raw_device_id: RawDeviceId,
        raw_event: RawDeviceEvent,
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
