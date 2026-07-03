//! `UiEcs`: the ECS-backed [`Application`] driver.
//!
//! Owns the ECS `World`, the frame schedules, and the async runtime. GPU init
//! happens up front, a single window is created as the UI root on `resumed`,
//! and rendering runs synchronously on the main thread. From M2 on, the model
//! lives in the world as a resource and [`ModelHandle::update`] calls queue
//! mutations that are drained and re-viewed on `ui_command`. Layout, input and
//! a dedicated render thread arrive in later milestones.

use std::sync::{atomic::AtomicBool, Arc, OnceLock};

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
use tokio::sync::mpsc;

use crate::{
    components::{
        layout::GlobalTransform,
        render::{RenderCtx, RenderItem},
        view::ViewChildren,
        window::{Window as WindowComp, WindowBelonging},
    },
    model::{ModelHandle, ModelResource},
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

/// Commands delivered into the event loop from outside (currently only
/// [`ModelHandle::update`]). Routed to [`UiEcs::ui_command`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiCommand {
    /// The model queue has (or may have) pending mutations: drain them and
    /// re-run the view.
    ModelUpdated,
}

/// The ECS application driver, parameterised over the model type `M` and view
/// function `F`.
pub struct UiEcs<M, F>
where
    M: Send + Sync + 'static,
    F: Fn(&M, &mut Scope) + Send + Sync + 'static,
{
    world: World,
    view_fn: F,

    /// Phase A + B hook (model drain, re-run view). Empty for now; a future
    /// milestone may add systems here that must run before the drain.
    model_update_schedule: Schedule,
    /// Phase C (animation, layout, flush, extract).
    render_schedule: Schedule,

    /// Async runtime kept alive for the app's lifetime (used for GPU init now,
    /// background tasks from a later milestone on).
    _runtime: tokio::runtime::Runtime,

    /// Receiver half of the model mutation queue. `UnboundedReceiver` is not
    /// `Sync`, so it is wrapped in a `Mutex` purely to make `UiEcs` itself
    /// `Sync` (accessed via `get_mut()` — no runtime locking occurs, same
    /// pattern as `matcha-tree/src/ui_tree.rs`).
    model_receiver: parking_lot::Mutex<mpsc::UnboundedReceiver<Box<dyn FnOnce(&mut M) + Send>>>,
    wake_pending: Arc<AtomicBool>,
    /// Filled once in `init()` (the event loop proxy is unavailable before
    /// then). `ModelHandle::update` calls made earlier still queue correctly;
    /// `init()` self-heals by waking immediately if a mutation is pending.
    proxy_slot: Arc<OnceLock<Box<dyn EventLoopProxy<Self>>>>,
}

impl<M, F> UiEcs<M, F>
where
    M: Send + Sync + 'static,
    F: Fn(&M, &mut Scope) + Send + Sync + 'static,
{
    /// Build a `UiEcs`: initialise the GPU, atlases and renderer, insert them
    /// (plus the initial model) as world resources, and wire the render
    /// schedule.
    pub fn new(model: M, view_fn: F) -> Self {
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
        world.insert_resource(ModelResource(model));

        let (sender, receiver) = mpsc::unbounded_channel::<Box<dyn FnOnce(&mut M) + Send>>();
        let wake_pending = Arc::new(AtomicBool::new(false));
        let proxy_slot: Arc<OnceLock<Box<dyn EventLoopProxy<Self>>>> = Arc::new(OnceLock::new());
        let wake: Arc<dyn Fn() + Send + Sync> = {
            let proxy_slot = proxy_slot.clone();
            Arc::new(move || {
                if let Some(proxy) = proxy_slot.get() {
                    proxy.send_command(UiCommand::ModelUpdated);
                }
            })
        };
        world.insert_resource(ModelHandle::new(sender, wake_pending.clone(), wake));

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
            model_receiver: parking_lot::Mutex::new(receiver),
            wake_pending,
            proxy_slot,
        }
    }

    /// Clone a handle for mutating the model from any thread. Safe to call
    /// before the event loop starts (`ModelHandle::update` queues correctly
    /// even before `init()` fills in the wake proxy).
    pub fn model_handle(&self) -> ModelHandle<M> {
        self.world.resource::<ModelHandle<M>>().clone()
    }

    /// Phase A (drain queued mutations into the model) + Phase B (re-run the
    /// view against the updated model) + request a redraw on every window.
    fn process_model_update(&mut self) {
        self.model_update_schedule.run(&mut self.world);

        self.wake_pending
            .store(false, std::sync::atomic::Ordering::Release);
        {
            let receiver = self.model_receiver.get_mut();
            let mut model = self.world.resource_mut::<ModelResource<M>>();
            while let Ok(f) = receiver.try_recv() {
                f(&mut model.0);
            }
        }

        let Some(root) = self.world.get_resource::<RenderWindowRoot>() else {
            return;
        };
        let root_entity = root.entity;
        let view_fn = &self.view_fn;
        self.world
            .resource_scope::<ModelResource<M>, _>(|world, model| {
                run_view(world, root_entity, |s| view_fn(&model.0, s));
            });

        let _ = self.world.run_system_cached(|q: Query<&WindowComp>| {
            for window in q.iter() {
                window.window.request_redraw();
            }
        });
    }

    /// Walk the window root's view tree and present one frame.
    fn present(&mut self, window_id: WindowId) {
        let Some(root) = self.world.get_resource::<RenderWindowRoot>() else {
            return;
        };
        // M1/M2 are single-window; ignore redraws for any other id.
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

impl<M, F> Application for UiEcs<M, F>
where
    M: Send + Sync + 'static,
    F: Fn(&M, &mut Scope) + Send + Sync + 'static,
{
    type Command = UiCommand;

    fn init(&mut self, proxy: Box<dyn EventLoopProxy<Self>>, _event_loop: &impl EventLoop) {
        // `set` only fails if already set; `init` runs exactly once per app.
        let _ = self.proxy_slot.set(proxy);
        // Self-heal: a `ModelHandle::update` call made before `init()` ran
        // could not reach a proxy yet, so replay the wake now if one is due.
        if self
            .wake_pending
            .load(std::sync::atomic::Ordering::Acquire)
        {
            if let Some(proxy) = self.proxy_slot.get() {
                proxy.send_command(UiCommand::ModelUpdated);
            }
        }
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

        let view_fn = &self.view_fn;
        self.world
            .resource_scope::<ModelResource<M>, _>(|world, model| {
                run_view(world, entity, |s| view_fn(&model.0, s));
            });

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
        // Per-window resource teardown is not needed for the single window M1/M2 support.
    }

    fn device_event(
        &mut self,
        _event_loop: &impl EventLoop,
        _window_id: WindowId,
        _event: DeviceEvent,
    ) {
        // Input handling arrives in M5.
    }

    fn ui_command(&mut self, _event_loop: &impl EventLoop, command: Self::Command) {
        match command {
            UiCommand::ModelUpdated => self.process_model_update(),
        }
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
