//! Render dispatch: snapshot extraction and the [`RenderDriver`] that turns a
//! per-frame [`RenderSnapshot`] into pixels.
//!
//! M4 moves encode/submit/present off the main thread. The main thread runs the
//! render schedule, acquires the window's `SurfaceTexture`, extracts a flat list
//! of `(RenderItem, transform)` into a [`RenderSnapshot`], and hands it to a
//! [`RenderDriver`]. The default [`ThreadDriver`] forwards each snapshot to a
//! per-window worker thread that builds the (still-deferred) render nodes, calls
//! [`SceneRenderer::render`], and presents. The `RenderItem` builders run on
//! that worker thread, not the main thread.
//!
//! [`InlineDriver`] runs the same `build_and_present` synchronously; it exists to
//! isolate regressions between "the snapshot/extract split" and "the threading".

use std::{collections::HashMap, sync::Arc, sync::mpsc, thread::JoinHandle};

use bevy_ecs::{entity::Entity, world::World};
use matcha_window::window::WindowId;
use nalgebra::Matrix4;
use parking_lot::{Condvar, Mutex};
use render_interface::PixelMaskIndex;
use render_interface::{
    MaskDescriptor, MaskId, MaskSource, MeshId, MeshSource, PixelMask, Scene, TextureId,
};
use renderer::{SceneRenderer, SceneTarget};
use std::collections::HashSet;

use crate::{
    clip::ClipArena,
    components::{
        focus::{FocusWithin, Focused},
        input::{Active, Hovered},
        layout::{GlobalTransform, LayoutOutput},
        render::{RenderCtx, RenderItem, RenderOpacity},
    },
    traversal,
};

/// One drawable entity captured for a frame: the shared node cache, its deferred
/// builder, its window-space transform (already composed by M3 layout), the size
/// layout allocated to it (`LayoutOutput::size` — what the builder must draw at),
/// its current opacity (`1.0` if the entity has no `RenderOpacity`), and its
/// focus and pointer state.
pub struct RenderItemSnapshot {
    pub rebuild_each_frame: bool,
    /// Which entity this was extracted from. Nothing on the render path reads
    /// it — it is here so a frame can be traced back to the tree that produced
    /// it, by a debugger or a test asserting on paint order.
    pub entity: Entity,
    pub cache: Arc<Mutex<Option<Scene>>>,
    pub builder: Arc<dyn Fn(&RenderCtx, &mut Scene) + Send + Sync>,
    pub transform: Matrix4<f32>,
    pub size: [f32; 2],
    pub opacity: f32,
    pub focused: bool,
    pub focus_within: bool,
    pub hovered: bool,
    pub active: bool,
    /// Innermost enclosing clip, as an index into the frame's [`ClipArena`].
    /// The clips it inherits are that one's ancestors.
    pub clip: Option<u32>,
}

/// One window's drawable entities plus the clips they sit inside, in paint
/// order. Both come out of the same walk: a clip is only meaningful relative to
/// the items it encloses.
#[derive(Default)]
pub struct ExtractedFrame {
    pub items: Vec<RenderItemSnapshot>,
    pub clips: ClipArena,
}

/// Everything a [`RenderDriver`] needs to draw one window's frame. Owns the
/// acquired `SurfaceTexture` (moved from the main thread) and clones of the GPU
/// resources so the worker thread is self-contained.
pub struct RenderSnapshot {
    pub window_id: WindowId,
    pub surface_texture: wgpu::SurfaceTexture,
    pub format: wgpu::TextureFormat,
    pub viewport_size: [f32; 2],
    pub load_color: wgpu::Color,
    pub items: Vec<RenderItemSnapshot>,
    /// The frame's CPU clip geometry. Indices in
    /// [`RenderItemSnapshot::clip`] point into this.
    pub clips: ClipArena,
    pub core: Arc<Mutex<GuiRenderer>>,
}

/// The UI owns the reusable Scene; the backend owns GPU resources. Keeping the
/// pair under one driver lock permits the existing threaded presentation path
/// without moving Sources or sharing each Source through an Arc.
pub struct GuiRenderer {
    pub scene: Scene,
    pub backend: SceneRenderer,
    quad: MeshSource,
    clip: MaskSource,
    meshes: HashSet<MeshId>,
    textures: HashSet<TextureId>,
    masks: HashSet<MaskId>,
}
impl GuiRenderer {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let mut desc = MaskDescriptor::new([1, 1], wgpu::TextureFormat::R8Unorm);
        desc.usages = wgpu::TextureUsages::RENDER_ATTACHMENT;
        let clip = MaskSource::new(desc, |c| {
            let attachments = [Some(wgpu::RenderPassColorAttachment {
                view: c.target.view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::WHITE),
                    store: wgpu::StoreOp::Store,
                },
            })];
            let _pass = c
                .gpu
                .encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    color_attachments: &attachments,
                    ..Default::default()
                });
            Ok(())
        });
        Self {
            scene: Scene::default(),
            backend: SceneRenderer::new(device, queue),
            quad: crate::scene::unit_quad(),
            clip,
            meshes: HashSet::new(),
            textures: HashSet::new(),
            masks: HashSet::new(),
        }
    }
    /// Merge complete widget Scenes. Unreferenced definitions supplied by a
    /// widget remain retention hints; no paint-tree or bitmap conversion occurs.
    pub fn render_extracted(
        &mut self,
        items: &[RenderItemSnapshot],
        clips: &ClipArena,
        target: SceneTarget<'_>,
    ) -> Result<(), renderer::scene_renderer::SceneError> {
        self.assemble(items, clips, target.viewport)?;
        self.backend.render(&self.scene, target)
    }
    /// Resolve retained widget Scenes into one complete interface submission.
    /// Separate from GPU recording so construction costs can be measured directly.
    pub fn assemble(
        &mut self,
        items: &[RenderItemSnapshot],
        clips: &ClipArena,
        viewport: [f32; 2],
    ) -> Result<(), renderer::scene_renderer::SceneError> {
        for phase in &mut self.scene.phases {
            phase.objects.clear();
        }
        self.scene.pixel_masks.clear();
        self.meshes.clear();
        self.textures.clear();
        self.masks.clear();
        self.meshes.insert(self.quad.id());
        self.masks.insert(self.clip.id());
        if self.scene.resources.mesh(self.quad.id()).is_none() {
            self.scene
                .resources
                .insert_mesh(self.quad.clone())
                .expect("new shared quad");
        }
        if self.scene.resources.mask(self.clip.id()).is_none() {
            self.scene
                .resources
                .insert_mask(self.clip.clone())
                .expect("new shared clip");
        }
        for clip in clips.as_slice() {
            self.scene.pixel_masks.push(PixelMask {
                mesh: self.quad.id(),
                texture: self.clip.id(),
                transform: clip.transform,
                parent: clip.parent.map(PixelMaskIndex),
            });
        }
        for item in items {
            let ctx = RenderCtx {
                transform: item.transform,
                viewport_size: viewport,
                size: item.size,
                focused: item.focused,
                focus_within: item.focus_within,
                hovered: item.hovered,
                active: item.active,
            };
            let mut cache = item.cache.lock();
            let rebuild = cache.is_none() || item.rebuild_each_frame;
            let source = cache.get_or_insert_with(Scene::default);
            if rebuild {
                (item.builder)(&ctx, source);
            }
            self.meshes.extend(source.resources.mesh_ids());
            self.textures.extend(source.resources.texture_ids());
            self.masks.extend(source.resources.mask_ids());
            if let Err(error) = crate::scene::append_scene(
                &mut self.scene,
                &source,
                item.transform,
                item.clip.map(PixelMaskIndex),
                item.opacity,
            ) {
                // Failed redraws must not accumulate fresh dynamic definitions
                // from the widgets visited before the malformed fragment.
                self.retain_submitted_sources();
                return Err(renderer::scene_renderer::SceneError::Invalid(
                    error.to_string(),
                ));
            }
        }
        self.retain_submitted_sources();
        Ok(())
    }
    fn retain_submitted_sources(&mut self) {
        self.scene
            .resources
            .retain_meshes(|id| self.meshes.contains(&id));
        self.scene
            .resources
            .retain_textures(|id| self.textures.contains(&id));
        self.scene
            .resources
            .retain_masks(|id| self.masks.contains(&id));
    }
}

/// Collect a window root's drawable entities and the clips enclosing them, in
/// paint order. Clones each entity's `RenderItem` (the `cache`/`builder`
/// `Arc`s are shared, not deep-copied) and its `GlobalTransform`; the builder
/// is not invoked here.
///
/// Order comes from [`crate::traversal::walk`], the same walk picking uses, so
/// what is drawn on top is what a click lands on.
pub fn extract_items(world: &World, root_entity: Entity) -> ExtractedFrame {
    let mut out = ExtractedFrame::default();
    traversal::walk(world, root_entity, None, &mut |world, entity, clip| {
        Some(extract_one(world, entity, *clip, &mut out))
    });
    out
}

/// Record `entity` if it draws, and return the innermost clip its children
/// sit inside — which may be one `entity` itself declared.
fn extract_one(
    world: &World,
    entity: Entity,
    clip: Option<u32>,
    out: &mut ExtractedFrame,
) -> Option<u32> {
    // `LayoutOutput` is written by the same `arrange_child` call that writes
    // `GlobalTransform`, so both are present on every laid-out entity;
    // `[0.0, 0.0]` only for one carrying a hand-inserted transform.
    let transform = world.get::<GlobalTransform>(entity).map(|t| t.affine);
    let size = world
        .get::<LayoutOutput>(entity)
        .map(|l| l.size)
        .unwrap_or([0.0, 0.0]);

    // A `Clip` covers the declaring entity too, not only its descendants, so
    // it is opened before the entity's own item is pushed. Shared with picking
    // so both agree on which clips apply to what.
    let own_clip = crate::clip::descend(&mut out.clips, world, entity, clip);

    if let (Some(item), Some(transform)) = (world.get::<RenderItem>(entity), transform) {
        let opacity = world
            .get::<RenderOpacity>(entity)
            .map(|o| o.0)
            .unwrap_or(1.0);
        out.items.push(RenderItemSnapshot {
            entity,
            rebuild_each_frame: item.rebuild_each_frame,
            cache: item.cache.clone(),
            builder: item.builder.clone(),
            transform,
            size,
            opacity,
            focused: world.get::<Focused>(entity).is_some(),
            focus_within: world.get::<FocusWithin>(entity).is_some(),
            hovered: world.get::<Hovered>(entity).is_some(),
            active: world.get::<Active>(entity).is_some(),
            clip: own_clip,
        });
    }

    own_clip
}

/// Build each item's (cached) render node and present the frame. Shared by both
/// drivers; runs on the worker thread under [`ThreadDriver`].
pub fn build_and_present(snapshot: RenderSnapshot) {
    let RenderSnapshot {
        window_id,
        surface_texture,
        format: _,
        viewport_size,
        load_color,
        items,
        clips,
        core,
    } = snapshot;

    let mut renderer = core.lock();
    let view = surface_texture
        .texture
        .create_view(&wgpu::TextureViewDescriptor::default());

    if let Err(e) = renderer.render_extracted(
        &items,
        &clips,
        SceneTarget {
            view: &view,
            viewport: viewport_size,
            clear: load_color,
            initial: None,
        },
    ) {
        log::error!("Scene render failed for window {window_id:?}: {e}");
        return;
    }

    surface_texture.present();
}

/// Consumes per-frame [`RenderSnapshot`]s. The main thread checks
/// [`is_busy`](RenderDriver::is_busy) before extracting a new frame, and calls
/// [`dispatch`](RenderDriver::dispatch) to hand one off.
pub trait RenderDriver: Send {
    /// Render (or enqueue) `snapshot`. May run synchronously or on a worker.
    fn dispatch(&mut self, snapshot: RenderSnapshot);

    /// `true` if `window`'s previous frame is still in flight, in which case the
    /// caller should coalesce (request a redraw and skip this frame).
    fn is_busy(&self, window: WindowId) -> bool;

    /// Block until `window`'s in-flight frame (if any) has finished presenting.
    /// Must be called before any `Surface::configure` (e.g. on resize) for that
    /// window: wgpu forbids reconfiguring a surface while a `SurfaceTexture`
    /// acquired from it — via `get_surface_texture` on the main thread, moved
    /// into a [`RenderSnapshot`] and only presented later, on the render thread
    /// — has not yet been presented/dropped. A no-op if the window has no
    /// render thread yet (nothing can be in flight).
    fn wait_idle(&self, window: WindowId);
}

/// Synchronous driver: builds and presents on the calling (main) thread. Never
/// busy. Retained to isolate render-threading regressions from the M4 refactor.
#[derive(Default)]
pub struct InlineDriver;

impl RenderDriver for InlineDriver {
    fn dispatch(&mut self, snapshot: RenderSnapshot) {
        build_and_present(snapshot);
    }

    fn is_busy(&self, _window: WindowId) -> bool {
        false
    }

    fn wait_idle(&self, _window: WindowId) {}
}

/// Per-window worker-thread driver (the M4 default). Each window gets one thread
/// that owns nothing but the receiving end of a snapshot channel plus a shared
/// busy flag.
#[derive(Default)]
pub struct ThreadDriver {
    threads: HashMap<WindowId, WindowThread>,
}

struct WindowThread {
    sender: mpsc::Sender<RenderSnapshot>,
    /// `true` between `dispatch` and the worker finishing that frame (i.e.
    /// presenting its `SurfaceTexture`). A `Mutex`+`Condvar` rather than a bare
    /// `AtomicBool` so [`Busy::wait_idle`] can block efficiently instead of
    /// spin-polling.
    busy: Arc<Busy>,
    _handle: JoinHandle<()>,
}

struct Busy {
    flag: Mutex<bool>,
    cvar: Condvar,
}

impl Busy {
    fn new() -> Self {
        Self {
            flag: Mutex::new(false),
            cvar: Condvar::new(),
        }
    }

    fn set(&self, value: bool) {
        let mut guard = self.flag.lock();
        *guard = value;
        if !value {
            self.cvar.notify_all();
        }
    }

    fn get(&self) -> bool {
        *self.flag.lock()
    }

    fn wait_idle(&self) {
        let mut guard = self.flag.lock();
        while *guard {
            self.cvar.wait(&mut guard);
        }
    }
}

impl RenderDriver for ThreadDriver {
    fn dispatch(&mut self, snapshot: RenderSnapshot) {
        let window_id = snapshot.window_id;
        let thread = self
            .threads
            .entry(window_id)
            .or_insert_with(|| WindowThread::spawn(window_id));

        thread.busy.set(true);
        if let Err(e) = thread.sender.send(snapshot) {
            thread.busy.set(false);
            log::error!("render thread for window {window_id:?} has gone away: {e}");
        }
    }

    fn is_busy(&self, window: WindowId) -> bool {
        self.threads
            .get(&window)
            .map(|t| t.busy.get())
            .unwrap_or(false)
    }

    fn wait_idle(&self, window: WindowId) {
        if let Some(t) = self.threads.get(&window) {
            t.busy.wait_idle();
        }
    }
}

impl WindowThread {
    fn spawn(window_id: WindowId) -> Self {
        let (sender, receiver) = mpsc::channel::<RenderSnapshot>();
        let busy = Arc::new(Busy::new());
        let busy_worker = busy.clone();

        let handle = std::thread::Builder::new()
            .name(format!("matcha render ({window_id:?})"))
            .spawn(move || {
                while let Ok(snapshot) = receiver.recv() {
                    build_and_present(snapshot);
                    busy_worker.set(false);
                }
            })
            .expect("OS refused to spawn a render thread");

        Self {
            sender,
            busy,
            _handle: handle,
        }
    }
}
