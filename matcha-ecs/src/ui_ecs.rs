//! `UiEcs`: the ECS-backed [`Application`] driver.
//!
//! Owns the ECS `World`, the frame schedules, and the async runtime. GPU init
//! happens up front, a single window is created as the UI root on `resumed`,
//! and rendering runs synchronously on the main thread. From M2 on, the model
//! lives in the world as a resource and [`ModelHandle::update`] calls queue
//! mutations that are drained and re-viewed on `ui_command`. Layout, input and
//! a dedicated render thread arrive in later milestones.

use std::{
    sync::{atomic::AtomicBool, Arc, OnceLock},
    time::{Duration, Instant},
};

use bevy_ecs::{
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
use renderer::CoreRenderer;
use tokio::sync::mpsc;

use crate::{
    components::{
        view::ViewChildren,
        window::{Window as WindowComp, WindowBelonging},
    },
    model::{ModelHandle, ModelResource},
    render::{extract_items, RenderDriver, RenderSnapshot, ThreadDriver},
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

/// How long after the last `Resized` event `render()` keeps forcing the
/// synchronous fallback path (see the `render_driver`/resize-jitter note on
/// [`UiEcs::last_resize`]).
const RESIZE_SYNC_WINDOW: Duration = Duration::from_millis(200);

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

    /// Turns per-frame snapshots into pixels. `ThreadDriver` by default (one
    /// worker thread per window); wrapped in a `Mutex` only so the non-`Sync`
    /// channel sender inside does not make `UiEcs` itself `!Sync` (accessed via
    /// `get_mut()` — no runtime locking occurs, same pattern as `model_receiver`).
    render_driver: parking_lot::Mutex<Box<dyn RenderDriver>>,

    /// Set on every `WindowEvent::Resized`. While recent (within
    /// `RESIZE_SYNC_WINDOW`), `render()` bypasses `render_driver` and calls
    /// `render::build_and_present` synchronously instead.
    ///
    /// Root cause this works around: before M4, `get_surface_texture` (whose
    /// lazy reconfigure-on-`Outdated` is the *only* place the swapchain is
    /// resized — see `WindowSurface::get_surface_texture`) and `present()` ran
    /// back-to-back on the main thread, so the acquired texture was presented
    /// at (almost) the size it was acquired at. M4 inserted a channel hop plus
    /// encode/submit between those two steps, and the `is_busy` coalescing
    /// guard lets several more `Resized` deltas land on the main thread while a
    /// frame is in flight. During a continuous drag-resize this grows the gap
    /// between "acquired at size A" and "presented while the OS window is
    /// already size D", which the compositor bridges by stretching — the
    /// jitter. Forcing the same-thread synchronous path during (and briefly
    /// after) a resize closes that gap back to ~0, matching pre-M4 behaviour,
    /// while steady-state (non-resize) frames keep the async `ThreadDriver`.
    last_resize: Option<Instant>,
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
        render_schedule.add_systems(crate::layout::run_layout.in_set(MatchaSet::Layout));
        render_schedule
            .add_systems(crate::systems::invalidate_on_layout_change.in_set(MatchaSet::Flush));

        Self {
            world,
            view_fn,
            model_update_schedule: Schedule::default(),
            render_schedule,
            _runtime: runtime,
            model_receiver: parking_lot::Mutex::new(receiver),
            wake_pending,
            proxy_slot,
            render_driver: parking_lot::Mutex::new(Box::new(ThreadDriver::default())),
            last_resize: None,
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

    /// Build a [`RenderSnapshot`] for `window_id`: acquire the surface texture on
    /// the main thread, extract the drawable items, and clone the GPU resources.
    /// Returns `None` when the frame should be skipped (wrong window, no GPU/root,
    /// or the surface could not yield a texture this frame).
    fn build_snapshot(&mut self, window_id: WindowId) -> Option<RenderSnapshot> {
        let root = self.world.get_resource::<RenderWindowRoot>()?;
        // M1–M4 are single-window; ignore redraws for any other id.
        if root.window_id != window_id {
            return None;
        }
        let root_entity = root.entity;

        let (device, queue) = self.world.resource::<GpuResource>().gpu.context()?;
        let (core, texture_atlas, stencil_atlas) = {
            let r = self.world.resource::<RendererResource>();
            (
                r.core.clone(),
                r.texture_atlas.clone(),
                r.stencil_atlas.clone(),
            )
        };

        let window_comp = self.world.get::<WindowComp>(root_entity)?;
        let window = &window_comp.window;
        let inner = window.inner_size();
        let viewport_size = [inner[0] as f32, inner[1] as f32];
        let format = window.format();

        let surface_texture = match window.surface().get_surface_texture(&device) {
            Ok(Some(texture)) => texture,
            Ok(None) => return None,
            Err(e) => {
                log::warn!("failed to acquire surface texture for {window_id:?}: {e}");
                return None;
            }
        };

        let items = extract_items(&self.world, root_entity);

        Some(RenderSnapshot {
            window_id,
            surface_texture,
            format,
            viewport_size,
            load_color: wgpu::Color {
                r: 0.1,
                g: 0.1,
                b: 0.1,
                a: 1.0,
            },
            items,
            device,
            queue,
            core,
            texture_atlas,
            stencil_atlas,
        })
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
        // Coalesce: if the previous frame for this window is still encoding on its
        // render thread, request another redraw and drop this one.
        if self.render_driver.get_mut().is_busy(window_id) {
            if let Some(root) = self.world.get_resource::<RenderWindowRoot>() {
                if root.window_id == window_id {
                    let root_entity = root.entity;
                    if let Some(window_comp) = self.world.get::<WindowComp>(root_entity) {
                        window_comp.window.request_redraw();
                    }
                }
            }
            return;
        }

        self.render_schedule.run(&mut self.world);
        let Some(snapshot) = self.build_snapshot(window_id) else {
            return;
        };

        let is_resizing = self
            .last_resize
            .is_some_and(|t| t.elapsed() < RESIZE_SYNC_WINDOW);
        if is_resizing {
            // See `last_resize` doc comment: close the acquire-to-present gap
            // during a live resize by staying on the main thread.
            crate::render::build_and_present(snapshot);
        } else {
            self.render_driver.get_mut().dispatch(snapshot);
        }
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
                // Surface reconfiguration itself still lands in a later
                // milestone (lazy Outdated-triggered reconfigure inside
                // `get_surface_texture` is what resizes the swapchain); this
                // timestamp only drives the resize-jitter workaround on
                // `last_resize`.
                self.last_resize = Some(Instant::now());
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
