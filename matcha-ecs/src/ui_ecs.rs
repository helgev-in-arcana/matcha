//! `UiEcs`: the ECS-backed [`Application`] driver.
//!
//! Owns the ECS `World`, the frame schedules, and the async runtime. For M1 it
//! initialises the GPU up front, creates a single window on `resumed`, runs the
//! view function once to populate the world, and renders synchronously on the
//! main thread. Model updates, per-frame re-`run_view`, layout, input and a
//! dedicated render thread arrive in later milestones.

use std::sync::Arc;

use bevy_ecs::{
    entity::Entity,
    schedule::{IntoScheduleConfigs, Schedule, SystemSet},
    system::{Query, Res, ResMut},
    world::World,
};
use gpu_utils::{
    gpu::{Gpu, GpuDescriptor},
    texture_atlas::TextureAtlas,
};
use matcha_window::{
    adapter::{EventLoop, EventLoopProxy},
    application::Application,
    event::{
        device_event::DeviceEvent,
        raw_device_event::{RawDeviceEvent, RawDeviceId},
        window_event::WindowEvent,
    },
    window::{Window as OsWindow, WindowConfig, WindowId},
};
use renderer::{CoreRenderer, RenderNode};

use crate::{
    components::{
        layout::GlobalTransform,
        render::{RenderCtx, RenderItem},
        view::ViewChildren,
        window::{Window as WindowComp, WindowBelonging},
    },
    resources::{GpuResource, RenderWindowRoot, RendererResource},
    view::{run_view, Scope},
};

/// Ordering buckets for the render schedule (Phase C). Bodies are empty for now;
/// the `.chain()` only fixes their relative order so systems added to each set
/// run in the documented sequence as later milestones fill them in.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MatchaSet {
    Animation,
    Layout,
    Flush,
    Extract,
}

/// Gate consulted by `create_surface`: `true` once surfaces may be created.
#[derive(bevy_ecs::resource::Resource)]
pub struct CanCreateSurface {
    pub flag: bool,
}

/// The ECS application driver, parameterised over the view function `F`.
///
/// M1 has no model; `F` takes only a `&mut Scope`. M2 will add the model type
/// and change the signature to `Fn(&M, &mut Scope)`.
pub struct UiEcs<F: Fn(&mut Scope) + Send + Sync + 'static> {
    world: World,
    view_fn: F,

    /// Phase A + B (model drain, re-run view). Empty and unused in M1; driven
    /// from `ui_command` once the model loop lands in M2.
    #[allow(dead_code)]
    model_update_schedule: Schedule,
    /// Phase C (animation, layout, flush, extract).
    render_schedule: Schedule,

    /// Async runtime kept alive for the app's lifetime (used for GPU init now,
    /// background tasks from M2 on).
    _runtime: tokio::runtime::Runtime,
}

impl<F: Fn(&mut Scope) + Send + Sync + 'static> UiEcs<F> {
    /// Build a `UiEcs`: initialise the GPU, atlases and renderer, insert them as
    /// world resources, and wire the render schedule.
    pub fn new(view_fn: F) -> Self {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("failed to build tokio current-thread runtime");

        let gpu = runtime
            .block_on(Gpu::new(GpuDescriptor::default()))
            .expect("GPU initialisation failed");
        let (device, _queue) = gpu
            .context()
            .expect("GPU device/queue available immediately after Gpu::new");

        let atlas_extent = wgpu::Extent3d {
            width: 4096,
            height: 4096,
            depth_or_array_layers: 4,
        };
        let texture_atlas = TextureAtlas::new(
            &device,
            atlas_extent,
            wgpu::TextureFormat::Rgba8UnormSrgb,
            TextureAtlas::DEFAULT_MARGIN_PX,
        );
        let stencil_atlas = TextureAtlas::new(
            &device,
            atlas_extent,
            wgpu::TextureFormat::R8Unorm,
            TextureAtlas::DEFAULT_MARGIN_PX,
        );
        let core = Arc::new(CoreRenderer::new(&device));

        let mut world = World::new();
        world.insert_resource(GpuResource { gpu });
        world.insert_resource(RendererResource {
            core,
            texture_atlas,
            stencil_atlas,
        });
        world.insert_resource(CanCreateSurface { flag: false });

        let mut render_schedule = Schedule::default();
        render_schedule.configure_sets(
            (
                MatchaSet::Animation,
                MatchaSet::Layout,
                MatchaSet::Flush,
                MatchaSet::Extract,
            )
                .chain(),
        );
        render_schedule.add_systems(crate::systems::temp_place.in_set(MatchaSet::Layout));

        Self {
            world,
            view_fn,
            model_update_schedule: Schedule::default(),
            render_schedule,
            _runtime: runtime,
        }
    }

    /// Walk the window root's view tree and present one frame.
    fn present(&mut self, window_id: WindowId) {
        let Some(root) = self.world.get_resource::<RenderWindowRoot>() else {
            return;
        };
        // M1 is single-window; ignore redraws for any other id.
        if root.window_id != window_id {
            return;
        }
        let root_entity = root.entity;

        let (device, queue) = match self.world.resource::<GpuResource>().gpu.context() {
            Some(dq) => dq,
            None => return,
        };
        let (core, texture_atlas, stencil_atlas) = {
            let r = self.world.resource::<RendererResource>();
            (
                r.core.clone(),
                r.texture_atlas.clone(),
                r.stencil_atlas.clone(),
            )
        };

        let ctx = RenderCtx {
            device: &device,
            queue: &queue,
            texture_atlas: &texture_atlas,
            stencil_atlas: &stencil_atlas,
        };
        let mut pseudo_root = RenderNode::new();
        collect_render_nodes(&self.world, root_entity, &ctx, &mut pseudo_root);

        let Some(window_comp) = self.world.get::<WindowComp>(root_entity) else {
            return;
        };
        let window = &window_comp.window;
        let inner = window.inner_size();
        let size = [inner[0] as f32, inner[1] as f32];
        let format = window.format();

        let _ = window
            .surface()
            .rendering_with_surface_texture(&device, |view, _texture| {
                let _ = core.render(
                    &device,
                    &queue,
                    format,
                    view,
                    size,
                    &pseudo_root,
                    wgpu::Color {
                        r: 0.1,
                        g: 0.1,
                        b: 0.1,
                        a: 1.0,
                    },
                    &texture_atlas.texture(),
                    &stencil_atlas.texture(),
                );
            });
    }
}

/// Depth-first walk of `entity`'s view children, appending each entity's cached
/// `RenderNode` (built on demand) to `out` under its `GlobalTransform`.
///
/// M1 places every widget with an absolute transform, so flattening the tree
/// into a single pseudo root is correct. Nested transforms arrive with M3.
fn collect_render_nodes(world: &World, entity: Entity, ctx: &RenderCtx, out: &mut RenderNode) {
    let Some(view_children) = world.get::<ViewChildren>(entity) else {
        return;
    };
    let children: Vec<Entity> = view_children.slots.iter().map(|(_, e)| *e).collect();

    for child in children {
        if let (Some(item), Some(transform)) = (
            world.get::<RenderItem>(child),
            world.get::<GlobalTransform>(child),
        ) {
            let node = {
                let mut cache = item.cache.lock();
                cache
                    .get_or_insert_with(|| Arc::new((item.builder)(ctx)))
                    .clone()
            };
            out.push_child(node, transform.affine);
        }
        collect_render_nodes(world, child, ctx, out);
    }
}

impl<F: Fn(&mut Scope) + Send + Sync + 'static> Application for UiEcs<F> {
    type Command = ();

    fn init(&mut self, _proxy: Box<dyn EventLoopProxy<Self>>, _event_loop: &impl EventLoop) {
        // M1 needs no proxy. M2 wires a type-erased wake function for the model
        // update loop.
    }

    fn resumed(&mut self, event_loop: &impl EventLoop) {
        // `resumed` can fire more than once; only create the window the first time.
        if self.world.contains_resource::<RenderWindowRoot>() {
            return;
        }

        let config = WindowConfig::default()
            .with_title("matcha-ecs")
            .with_inner_size([800u32, 600u32]);
        let window = match OsWindow::new(&config, event_loop) {
            Ok(w) => w,
            Err(e) => {
                log::error!("failed to create window: {e}");
                return;
            }
        };
        let window_id = window.id();

        let entity = self.world.spawn_empty().id();
        self.world.entity_mut(entity).insert((
            WindowComp { window },
            ViewChildren::default(),
            WindowBelonging {
                window_id,
                window_entity: entity,
            },
        ));

        // Populate the view tree once (per-frame re-run arrives in M2).
        let view_fn = &self.view_fn;
        run_view(&mut self.world, entity, |s| view_fn(s));

        self.world
            .insert_resource(RenderWindowRoot { entity, window_id });

        if let Some(window_comp) = self.world.get::<WindowComp>(entity) {
            window_comp.window.request_redraw();
        }
    }

    fn create_surface(&mut self, event_loop: &impl EventLoop) {
        let _ = event_loop;

        if !self.world.contains_resource::<GpuResource>() {
            return;
        }

        let _ = self.world.run_system_cached(
            |mut q: Query<&mut WindowComp>,
             gpu: Res<GpuResource>,
             mut can_create_surface: ResMut<CanCreateSurface>| {
                let (device, _) = gpu
                    .gpu
                    .context()
                    .expect("GPU device must exist while create_surface runs");

                for mut window in q.iter_mut() {
                    if let Err(e) = window.window.create_surface(&gpu.gpu.instance(), &device) {
                        log::error!("failed to create surface: {e}");
                    }
                }

                can_create_surface.flag = true;
            },
        );
    }

    fn destroy_surface(&mut self, event_loop: &impl EventLoop) {
        let _ = event_loop;

        let _ = self.world.run_system_cached(
            |mut q: Query<&mut WindowComp>, mut can_create_surface: ResMut<CanCreateSurface>| {
                for mut window in q.iter_mut() {
                    window.window.destroy_surface();
                }
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
        self.render_schedule.run(&mut self.world);
        self.present(window_id);
    }

    fn window_event(
        &mut self,
        event_loop: &impl EventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized { inner_size, .. } => {
                // Surface reconfiguration lands in a later milestone; log for now.
                log::debug!("window resized to {inner_size:?}");
            }
            _ => {}
        }
    }

    fn window_destroyed(&mut self, _event_loop: &impl EventLoop, _window_id: WindowId) {
        // Per-window resource teardown is not needed for M1's single window.
    }

    fn device_event(&mut self, _event_loop: &impl EventLoop, _window_id: WindowId, _event: DeviceEvent) {
        // Input handling arrives in M5.
    }

    fn ui_command(&mut self, _event_loop: &impl EventLoop, _command: Self::Command) {
        // No commands in M1 (`Command = ()`).
    }

    // ---------------------------------------------------------------
    // default implementations (currently no-ops)
    // ---------------------------------------------------------------

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
