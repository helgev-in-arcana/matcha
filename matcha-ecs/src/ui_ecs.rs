//! `UiEcs`: the ECS-backed [`Application`] driver.
//!
//! Owns the ECS `World` and the frame schedules (no async runtime of its own —
//! T-1 removed the `tokio` dependency; GPU init blocks via
//! `futures::executor::block_on` and the `bevy_tasks` pools are initialised
//! explicitly up front). A single window is created as the UI root on `resumed`,
//! and rendering runs synchronously on the main thread. From M2 on, the model
//! lives in the world as a resource and [`ModelHandle::update`] calls queue
//! mutations that are drained and re-viewed on `ui_command`. From M5 on,
//! `device_event` resolves a pointer press through [`crate::pick`] and applies
//! the matched `Msg` to the model via a user-supplied `reducer`, reusing the
//! same Phase B (re-view) + redraw path as the model queue; keyboard and IME
//! events go to the focus path instead (see [`crate::keyboard`]).

use std::sync::{atomic::AtomicBool, mpsc, Arc, OnceLock};

use bevy_ecs::{
    entity::Entity,
    schedule::{IntoScheduleConfigs, Schedule, SystemSet},
    system::{Query, Res, ResMut, ScheduleSystem},
    world::World,
};
use bevy_tasks::{AsyncComputeTaskPool, ComputeTaskPool, TaskPoolBuilder};
use gpu_utils::{
    gpu::{Gpu, GpuDescriptor},
    texture_atlas::TextureAtlas,
};
use matcha_window::{
    adapter::{EventLoop, EventLoopProxy},
    application::Application,
    event::{
        device_event::{DeviceEvent, DeviceEventData},
        raw_device_event::{RawDeviceEvent, RawDeviceId},
        window_event::WindowEvent,
    },
    window::{Window as OsWindow, WindowConfig, WindowId},
};
use renderer::{CoreRenderer, MaskNode};

use crate::{
    components::{
        input::Message,
        view::ViewChildren,
        window::{Window as WindowComp, WindowBelonging},
    },
    focus::{run_validate_focus, sync_focus_components, Focus, FocusConfig},
    input::{
        dispatch_pointer_drag, dispatch_pointer_scroll, resolve_pointer_press,
        set_pointer_capture, MessageQueue,
    },
    keyboard::{dispatch_ime, dispatch_key, sync_ime_state},
    model::{ModelHandle, ModelResource},
    pick::{update_picker, PickQuery, Picker, PickerResource},
    pointer::{self, sync_cursor, sync_pointer_components},
    render::{build_and_present, extract_items, InlineDriver, RenderDriver, RenderSnapshot},
    resources::{
        ClipMask, FrameTime, GpuResource, RedrawRequest, RenderWindowRoot, RendererResource, ui_root,
    },
    view::{run_view, Scope},
};

#[cfg(not(web))]
use crate::render::ThreadDriver;

/// The render schedule's stages, run in this order every frame.
///
/// These are **timing contracts**, not feature buckets: each says what must be
/// true of the world by the time it ends, not what kind of work belongs in it.
/// [`PreLayout`](Self::PreLayout) and [`PreExtract`](Self::PreExtract) are open
/// extension points — see [`UiEcs::with_pre_layout_systems`] /
/// [`UiEcs::with_pre_extract_systems`] — and the core registers nothing in the
/// former and only contract plumbing in the latter.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MatchaSet {
    /// Settle everything layout reads, and the shape of the entity tree, for
    /// this frame: animated values, tree structure (including despawning
    /// entities whose [`ManualDespawn`](crate::components::view::ManualDespawn)
    /// owner is done with them). Open to registered systems; empty by default.
    PreLayout,
    /// Core: measure and arrange the tree, writing `LayoutOutput` /
    /// `GlobalTransform`.
    Layout,
    /// Settle the extract contract now that layout is known: the components
    /// extract reads (`RenderOpacity`, …) and the validity of each entity's
    /// cached render node. Open to registered systems; the core registers the
    /// invalidation, picking and focus plumbing here.
    PreExtract,
    /// Core: collect the frame's drawable entities into a snapshot.
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

/// The ECS application driver, parameterised over the model type `M`, the
/// click-message type `Msg`, the view function `F`, and the reducer `R` that
/// applies a dispatched `Msg` to the model (`ECS_IMPLEMENTATION_PLAN.md` §6.2).
pub struct UiEcs<M, Msg, F, R>
where
    M: Send + Sync + 'static,
    Msg: Message,
    F: Fn(&M, &mut Scope) + Send + Sync + 'static,
    R: Fn(&mut M, Msg) + Send + Sync + 'static,
{
    world: World,
    view_fn: F,
    reducer: R,

    /// Phase A + B hook (model drain, re-run view). Empty for now; a future
    /// milestone may add systems here that must run before the drain.
    model_update_schedule: Schedule,
    /// Phase C (animation, layout, flush, extract).
    render_schedule: Schedule,

    /// Receiver half of the model mutation queue. `mpsc::Receiver` is not
    /// `Sync`, so it is wrapped in a `Mutex` purely to make `UiEcs` itself
    /// `Sync` (accessed via `get_mut()` — no runtime locking occurs, same
    /// pattern as `matcha-tree/src/ui_tree.rs`).
    model_receiver: parking_lot::Mutex<mpsc::Receiver<Box<dyn FnOnce(&mut M) + Send>>>,
    wake_pending: Arc<AtomicBool>,
    /// Filled once in `init()` (the event loop proxy is unavailable before
    /// then). `ModelHandle::update` calls made earlier still queue correctly;
    /// `init()` self-heals by waking immediately if a mutation is pending.
    proxy_slot: Arc<OnceLock<Box<dyn EventLoopProxy<Self>>>>,

    /// Turns per-frame snapshots into pixels. `ThreadDriver` by default
    /// natively (one worker thread per window), `InlineDriver` on the web —
    /// swap with [`UiEcs::with_render_driver`]. Wrapped in a `Mutex` only so the
    /// non-`Sync` channel sender inside does not make `UiEcs` itself `!Sync`
    /// (accessed via `get_mut()` — no runtime locking occurs, same pattern as
    /// `model_receiver`).
    render_driver: parking_lot::Mutex<Box<dyn RenderDriver>>,
}

impl<M, Msg, F, R> UiEcs<M, Msg, F, R>
where
    M: Send + Sync + 'static,
    Msg: Message,
    F: Fn(&M, &mut Scope) + Send + Sync + 'static,
    R: Fn(&mut M, Msg) + Send + Sync + 'static,
{
    /// Build a `UiEcs`: initialise the GPU, atlases and renderer, insert them
    /// (plus the initial model) as world resources, and wire the render
    /// schedule. `reducer` applies a `Msg` dispatched by a click (`device_event`)
    /// to the model, the same way `ModelHandle::update` applies a queued
    /// mutation.
    ///
    /// Only available natively: adapter and device requests are genuinely
    /// asynchronous, and blocking on a promise is not possible on the web —
    /// there the browser's event loop has to run for it to resolve. Web callers
    /// use [`Self::new_async`] and drive it with `wasm_bindgen_futures`.
    #[cfg(not(web))]
    pub fn new(model: M, view_fn: F, reducer: R) -> Self {
        futures::executor::block_on(Self::new_async(model, view_fn, reducer))
    }

    /// [`Self::new`] with an explicit GPU descriptor. Headless tests pass
    /// [`GpuDescriptor::noop`] to run the full driver without any GPU.
    #[cfg(not(web))]
    pub fn new_with_gpu(model: M, view_fn: F, reducer: R, gpu_desc: GpuDescriptor) -> Self {
        futures::executor::block_on(Self::new_with_gpu_async(model, view_fn, reducer, gpu_desc))
    }

    /// Build a `UiEcs`, awaiting GPU initialisation. The only constructor that
    /// works on the web.
    pub async fn new_async(model: M, view_fn: F, reducer: R) -> Self {
        Self::new_with_gpu_async(model, view_fn, reducer, GpuDescriptor::default()).await
    }

    /// [`Self::new_async`] with an explicit GPU descriptor.
    pub async fn new_with_gpu_async(
        model: M,
        view_fn: F,
        reducer: R,
        gpu_desc: GpuDescriptor,
    ) -> Self {
        let gpu = Gpu::new(gpu_desc).await.expect("GPU initialisation failed");
        Self::from_gpu(model, view_fn, reducer, gpu)
    }

    /// Everything after GPU initialisation, which is entirely synchronous.
    ///
    /// Split out so the web can `await` the GPU on its own and then land here,
    /// while native keeps a blocking constructor.
    pub fn from_gpu(model: M, view_fn: F, reducer: R, gpu: Gpu) -> Self {
        // First-wins statics (T-1): initialise explicitly before any bevy_ecs
        // system (e.g. a future `par_iter`) has a chance to lazily default-init
        // `ComputeTaskPool` to an all-cores pool of its own choosing.
        //
        // No-ops on the web, where bevy_tasks' pools are single-threaded and
        // `num_threads`/`thread_name` are documented as ignored.
        ComputeTaskPool::get_or_init(|| {
            TaskPoolBuilder::new()
                .num_threads(2)
                .thread_name("matcha compute".into())
                .build()
        });
        AsyncComputeTaskPool::get_or_init(|| {
            TaskPoolBuilder::new()
                .num_threads(2)
                .thread_name("matcha async".into())
                .build()
        });

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
        world.insert_resource(PickerResource::default());
        world.insert_resource(Focus::default());
        world.insert_resource(MessageQueue::<Msg>::default());
        world.insert_resource(FocusConfig::default());
        world.insert_resource(FrameTime(web_time::Instant::now()));
        world.insert_resource(RedrawRequest::default());

        let (sender, receiver) = mpsc::channel::<Box<dyn FnOnce(&mut M) + Send>>();
        let wake_pending = Arc::new(AtomicBool::new(false));
        let proxy_slot: Arc<OnceLock<Box<dyn EventLoopProxy<Self>>>> = Arc::new(OnceLock::new());
        let wake: crate::model::WakeFn = {
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
                MatchaSet::PreLayout,
                MatchaSet::Layout,
                MatchaSet::PreExtract,
                MatchaSet::Extract,
            )
                .chain(),
        );
        // PreLayout is deliberately empty: nothing the core itself needs to do
        // belongs there. Animation lives in `matcha-ecs-widgets` and registers
        // itself via `with_pre_layout_systems`.
        render_schedule.add_systems(crate::layout::run_layout.in_set(MatchaSet::Layout));
        render_schedule
            .add_systems(crate::systems::invalidate_on_layout_change.in_set(MatchaSet::PreExtract));
        render_schedule.add_systems(update_picker.in_set(MatchaSet::PreExtract));
        // Focus must be re-derived against the current tree before its derived
        // markers are synced: the focused entity may have been despawned or
        // rebuilt by this frame's reconcile pass.
        render_schedule.add_systems(
            (run_validate_focus, sync_focus_components, sync_ime_state)
                .chain()
                .in_set(MatchaSet::PreExtract),
        );
        // Hover is re-resolved here, after `update_picker`, so a widget that
        // appears under a stationary cursor comes up already hovered.
        render_schedule.add_systems(
            (sync_pointer_components, sync_cursor)
                .chain()
                .after(update_picker)
                .in_set(MatchaSet::PreExtract),
        );

        Self {
            world,
            view_fn,
            reducer,
            model_update_schedule: Schedule::default(),
            render_schedule,
            model_receiver: parking_lot::Mutex::new(receiver),
            wake_pending,
            proxy_slot,
            #[cfg(not(web))]
            render_driver: parking_lot::Mutex::new(Box::new(ThreadDriver::default())),
            // The web has one thread, and `std::thread::spawn` panics there.
            #[cfg(web)]
            render_driver: parking_lot::Mutex::new(Box::new(InlineDriver)),
        }
    }

    /// Register systems into [`MatchaSet::PreLayout`] — the stage that settles
    /// everything layout reads (animated values, tree structure) for the frame.
    ///
    /// Takes anything `add_systems` takes, so a plugin can express its own
    /// internal ordering (`(a, b).chain()`) or relate itself to another system
    /// (`.after(..)`). No ordering is imposed *between* separately-registered
    /// systems: if two of them depend on each other, say so explicitly.
    ///
    /// ```ignore
    /// UiEcs::new(model, view, reduce)
    ///     .with_pre_layout_systems(matcha_ecs_widgets::animation::default_systems())
    /// ```
    pub fn with_pre_layout_systems<Marker>(
        mut self,
        systems: impl IntoScheduleConfigs<ScheduleSystem, Marker>,
    ) -> Self {
        self.render_schedule
            .add_systems(systems.in_set(MatchaSet::PreLayout));
        self
    }

    /// Register systems into [`MatchaSet::PreExtract`] — the stage that settles
    /// the extract contract (the components extract reads, and the validity of
    /// each entity's cached render node) once layout is known. Same semantics
    /// as [`Self::with_pre_layout_systems`].
    pub fn with_pre_extract_systems<Marker>(
        mut self,
        systems: impl IntoScheduleConfigs<ScheduleSystem, Marker>,
    ) -> Self {
        self.render_schedule
            .add_systems(systems.in_set(MatchaSet::PreExtract));
        self
    }

    /// Swap the picking backend (default: [`crate::pick::RectPicker`], the 2D
    /// flat-rect implementation). A 3D application would install a BVH or
    /// GPU ID-buffer picker here instead; nothing downstream of picking cares
    /// which one is in use.
    pub fn with_picker(mut self, picker: impl Picker) -> Self {
        self.world.insert_resource(PickerResource(Box::new(picker)));
        self
    }

    /// Swap the render driver (default: [`ThreadDriver`] natively,
    /// [`InlineDriver`] on the web, which is the only one that works there).
    pub fn with_render_driver(mut self, driver: impl RenderDriver + 'static) -> Self {
        self.render_driver = parking_lot::Mutex::new(Box::new(driver));
        self
    }

    /// The current focus state. Focus lives in the ECS world rather than in
    /// the app model, so this is how an embedder reads it from outside.
    pub fn focus(&self) -> &Focus {
        self.world.resource::<Focus>()
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

        self.rerun_view_and_redraw();
    }

    /// Phase B (re-run the view against the current model) + request a
    /// redraw on every window. Shared by [`Self::process_model_update`] (model
    /// queue drain) and [`Self::dispatch_click`] (click -> reducer).
    fn rerun_view_and_redraw(&mut self) {
        let Some(root_entity) = ui_root(&self.world) else {
            return;
        };
        let view_fn = &self.view_fn;
        self.world
            .resource_scope::<ModelResource<M>, _>(|world, model| {
                run_view(world, root_entity, |s| view_fn(&model.0, s));
            });

        self.request_redraw_all();
    }

    /// Handle a pointer press at `pos` (window space): one pick serves both
    /// focus and click routing (see [`resolve_pointer_press`]).
    ///
    /// A click message goes through the reducer and needs the view re-run.
    /// A focus-only change does not: focus lives in the ECS world, not in the
    /// app model, so widgets read it straight from their entity — a redraw is
    /// enough, and re-running the view would be wasted work.
    fn on_pointer_press(&mut self, pos: [f32; 2], count: u32) {
        let query = PickQuery { viewport_pos: pos };
        let press = resolve_pointer_press::<Msg>(&mut self.world, &query, count);

        if let Some(msg) = press.click_msg {
            let mut model = self.world.resource_mut::<ModelResource<M>>();
            (self.reducer)(&mut model.0, msg);
            drop(model);
            self.rerun_view_and_redraw();
        } else if !self.drain_message_queue() && (press.focus_changed || press.pointer_changed) {
            self.request_redraw_all();
        }
    }

    /// Apply everything queued in [`MessageQueue`] through the reducer, then
    /// re-view once. Returns whether anything was applied.
    ///
    /// Keyboard/IME handlers and widget systems cannot reach the model or the
    /// reducer from where they run, so they queue instead; this is where the
    /// queue is redeemed.
    fn drain_message_queue(&mut self) -> bool {
        let messages = self.world.resource_mut::<MessageQueue<Msg>>().drain();
        if messages.is_empty() {
            return false;
        }
        {
            let mut model = self.world.resource_mut::<ModelResource<M>>();
            for msg in messages {
                (self.reducer)(&mut model.0, msg);
            }
        }
        self.rerun_view_and_redraw();
        true
    }

    /// Close out an input event: redeem anything it queued, and redraw if it
    /// changed something without producing a message.
    ///
    /// Every input arm ends the same way, because the two ways an event can
    /// matter are exhaustive. A message means app state moved, so the view is
    /// re-run (inside [`Self::drain_message_queue`]). No message but consumed
    /// means only ECS-side state moved — a caret, a scroll offset, a hover —
    /// which widgets read off their own entity, so a redraw suffices and
    /// re-running the view would be wasted. Unconsumed means nothing happened.
    ///
    /// Having this in one place is also what keeps adding an input kind to
    /// [`Application::device_event`] a one-line change.
    fn settle(&mut self, consumed: bool) {
        if consumed && !self.drain_message_queue() {
            self.request_redraw_all();
        }
    }

    /// The root entity of `window_id`'s view tree, if that is the window this
    /// app is driving.
    ///
    /// The window-id check is what the plain [`ui_root`] accessor cannot do:
    /// these callers are answering "is the window the event names *mine*?",
    /// which is a question that survives there being more than one root.
    fn root_of(&self, window_id: WindowId) -> Option<Entity> {
        let root = self.world.get_resource::<RenderWindowRoot>()?;
        (root.window_id == window_id).then_some(root.entity)
    }

    /// Ask one window to redraw, ignoring an id that is not ours.
    fn request_redraw_of(&self, window_id: WindowId) {
        if let Some(root) = self.root_of(window_id)
            && let Some(window_comp) = self.world.get::<WindowComp>(root)
        {
            window_comp.window.request_redraw();
        }
    }

    /// Ask every window for a redraw, without re-running the view.
    fn request_redraw_all(&mut self) {
        let _ = self.world.run_system_cached(|q: Query<&WindowComp>| {
            for window in q.iter() {
                window.window.request_redraw();
            }
        });
    }
}

/// Rendering: building a frame's snapshot and presenting it, either dispatched
/// to `render_driver` ([`Application::render`]) or synchronously on the calling
/// thread ([`Self::render_sync`], used for resize).
impl<M, Msg, F, R> UiEcs<M, Msg, F, R>
where
    M: Send + Sync + 'static,
    Msg: Message,
    F: Fn(&M, &mut Scope) + Send + Sync + 'static,
    R: Fn(&mut M, Msg) + Send + Sync + 'static,
{
    /// Build a [`RenderSnapshot`] for `window_id`: acquire the surface texture,
    /// extract the drawable items, and clone the GPU resources. Returns `None`
    /// if the frame should be skipped (wrong window, no GPU/root, or the
    /// surface has no texture to give this frame).
    fn build_snapshot(&mut self, window_id: WindowId) -> Option<RenderSnapshot> {
        let root_entity = self.root_of(window_id)?;

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

        let frame = extract_items(&self.world, root_entity);
        let clips = self.resolve_clips(&frame.clips, &device, &queue, &stencil_atlas);

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
            items: frame.items,
            clips,
            device,
            queue,
            core,
            texture_atlas,
            stencil_atlas,
        })
    }

    /// Pair each extracted clip rectangle with the shared coverage texel,
    /// allocating that texel on first use.
    ///
    /// Extraction is deliberately GPU-free (which is what makes clipping
    /// headlessly testable), so attaching the image happens here — this is the
    /// only place holding the device, queue and atlas. Same lazy-insert pattern
    /// the text and image widgets use for their own caches.
    fn resolve_clips(
        &mut self,
        clips: &crate::clip::ClipArena,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        stencil_atlas: &TextureAtlas,
    ) -> Vec<MaskNode> {
        if clips.is_empty() {
            return Vec::new();
        }

        if self.world.get_resource::<ClipMask>().is_none() {
            match stencil_atlas.allocate(device, queue, [1, 1]) {
                Ok(region) => {
                    if let Err(e) = region.write_data(queue, &[0xff]) {
                        log::error!("failed to write the shared clip coverage texel: {e}");
                        return Vec::new();
                    }
                    self.world.insert_resource(ClipMask { region });
                }
                Err(e) => {
                    log::error!("failed to allocate the shared clip coverage texel: {e}");
                    return Vec::new();
                }
            }
        }

        let region = self.world.resource::<ClipMask>().region.clone();
        clips
            .as_slice()
            .iter()
            .map(|rect| MaskNode {
                parent: rect.parent,
                transform: rect.transform,
                region: region.clone(),
            })
            .collect()
    }

    /// Advance animation/layout and build this frame's snapshot for
    /// `window_id`. Shared by [`Application::render`] and [`Self::render_sync`],
    /// which differ only in how they hand the result off.
    fn advance_and_snapshot(&mut self, window_id: WindowId) -> Option<RenderSnapshot> {
        self.world
            .insert_resource(FrameTime(web_time::Instant::now()));
        // Systems that need a follow-up frame (an in-flight animation, say)
        // re-request it during the run; start from a clean slate each frame.
        self.world.resource_mut::<RedrawRequest>().reset();
        self.render_schedule.run(&mut self.world);
        self.build_snapshot(window_id)
    }

    /// Request another redraw if any system asked for one during this frame
    /// (via [`RedrawRequest::request`]), so e.g. an in-progress fade keeps
    /// animating. Shared by [`Application::render`] and [`Self::render_sync`].
    fn request_redraw_if_requested(&mut self, window_id: WindowId) {
        if !self.world.resource::<RedrawRequest>().is_requested() {
            return;
        }
        self.request_redraw_of(window_id);
    }

    /// Render and present `window_id` synchronously on the calling thread,
    /// bypassing `render_driver`. Used after a resize so the new-size frame
    /// is presented before the resize handler returns, instead of being
    /// deferred to the render thread.
    fn render_sync(&mut self, window_id: WindowId) {
        if let Some(snapshot) = self.advance_and_snapshot(window_id) {
            build_and_present(snapshot);
        }
        self.request_redraw_if_requested(window_id);
    }
}

impl<M, Msg, F, R> Application for UiEcs<M, Msg, F, R>
where
    M: Send + Sync + 'static,
    Msg: Message,
    F: Fn(&M, &mut Scope) + Send + Sync + 'static,
    R: Fn(&mut M, Msg) + Send + Sync + 'static,
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
        // Messages queued by systems during the previous frame's schedule (a
        // widget reacting to focus loss, say) are redeemed here, before this
        // frame's layout, so the view reflects them.
        self.drain_message_queue();

        // Coalesce: if the previous frame for this window is still encoding on its
        // render thread, request another redraw and drop this one.
        if self.render_driver.get_mut().is_busy(window_id) {
            self.request_redraw_of(window_id);
            return;
        }

        if let Some(snapshot) = self.advance_and_snapshot(window_id) {
            self.render_driver.get_mut().dispatch(snapshot);
        }

        self.request_redraw_if_requested(window_id);
    }

    fn window_event(
        &mut self,
        event_loop: &impl EventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized { inner_size, .. } => {
                // wgpu forbids reconfiguring a surface while a SurfaceTexture
                // acquired from it hasn't been presented yet, so wait for any
                // frame still in flight on the render thread first.
                self.render_driver.get_mut().wait_idle(window_id);

                let Some((device, _queue)) = self.world.resource::<GpuResource>().gpu.context()
                else {
                    return;
                };

                let mut windows = self.world.query::<&WindowComp>();
                let Some(window_comp) = windows
                    .iter(&self.world)
                    .find(|window_comp| window_comp.window.id() == window_id)
                else {
                    return;
                };

                window_comp.window.surface().resize(
                    [inner_size[0].round() as u32, inner_size[1].round() as u32],
                    &device,
                );

                // Render synchronously instead of just requesting a redraw, so
                // the resized frame is presented before this handler returns.
                // Blocks the event loop (all windows) for one frame; accepted
                // tradeoff for now.
                self.render_sync(window_id);
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
        event: DeviceEvent,
    ) {
        // Hover first, so a press is resolved against an up-to-date chain.
        //
        // There is no move-specific arm because a plain cursor move produces no
        // `MouseInput` of its own once `matcha-window`'s state machine has
        // processed it — its whole payload is the updated position, which every
        // mouse event carries. Keyboard events are excluded so an early
        // keystroke cannot claim the pointer is at the origin.
        if let DeviceEventData::MouseInput { event: mouse, .. } = event.event() {
            let left = matches!(mouse, Some(matcha_window::event::device_event::MouseInput::Left));
            let position = (!left).then(|| event.mouse_viewport_position());
            let moved = pointer::set_position(&mut self.world, position);
            self.settle(moved);
        }

        let at_pointer = PickQuery {
            viewport_pos: event.mouse_viewport_position(),
        };

        // `on_click` only fires on the primary-button press edge (not every
        // move/release), so a hit is always a genuine new click. This one arm
        // does not go through `settle`: a click can produce a message *and*
        // change focus, and the reducer has to run before the view is re-run.
        if let Some(count) = event.on_click(|count| count) {
            self.on_pointer_press(at_pointer.viewport_pos, count);
        }

        // Releasing the button ends the drag the press had captured, so the
        // next one starts from whatever the next press lands on.
        if event.on_click_released(|_count| ()).is_some() {
            set_pointer_capture(&mut self.world, None);
            let released = pointer::set_pressed(&mut self.world, None);
            self.settle(released);
        }

        // A drag continues an interaction a press already started (dragging out
        // a text selection, say), so it goes straight to positioned delivery
        // without touching focus or click routing.
        if event.on_drag(|_from, _button| ()).is_some() {
            let consumed = dispatch_pointer_drag(&mut self.world, &at_pointer);
            self.settle(consumed);
        }

        // Scrolling is positioned like a drag but starts no interaction, so it
        // likewise bypasses focus and click routing. An unconsumed scroll means
        // nothing under the pointer could move.
        if let Some(delta) = event.on_scroll(|delta| delta) {
            let consumed = dispatch_pointer_scroll(&mut self.world, &at_pointer, delta);
            self.settle(consumed);
        }

        // Keyboard and IME have no spatial origin: they go to whatever holds
        // focus. Both walk the focus path root->leaf; see `keyboard.rs`.
        if let Some(key_input) = event.on_key_down(|input| input.clone()) {
            // Tab moves focus before the focused widget is offered the key: no
            // widget in this workspace wants a literal tab character, and a
            // `Tab` that fell through would be typed instead of navigating.
            let consumed = crate::tab_order::handle_tab_key(&mut self.world, &key_input)
                || dispatch_key(&mut self.world, &key_input);
            self.settle(consumed);
        }

        if let Some(ime_event) = event.on_ime(|ime| ime.clone()) {
            let consumed = dispatch_ime(&mut self.world, &ime_event);
            self.settle(consumed);
        }
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
