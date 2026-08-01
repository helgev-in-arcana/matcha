//! Render dispatch: snapshot extraction and the [`RenderDriver`] that turns a
//! per-frame [`RenderSnapshot`] into pixels.
//!
//! M4 moves encode/submit/present off the main thread. The main thread runs the
//! render schedule, acquires the window's `SurfaceTexture`, extracts a flat list
//! of `(RenderItem, transform)` into a [`RenderSnapshot`], and hands it to a
//! [`RenderDriver`]. The default [`ThreadDriver`] forwards each snapshot to a
//! per-window worker thread that builds the (still-deferred) render nodes, calls
//! [`CoreRenderer::render_flat`], and presents. The `RenderItem` builders run on
//! that worker thread, not the main thread.
//!
//! [`InlineDriver`] runs the same `build_and_present` synchronously; it exists to
//! isolate regressions between "the snapshot/extract split" and "the threading".

use std::{collections::HashMap, sync::mpsc, sync::Arc, thread::JoinHandle};

use bevy_ecs::{entity::Entity, world::World};
use gpu_utils::texture_atlas::TextureAtlas;
use matcha_window::window::WindowId;
use nalgebra::Matrix4;
use parking_lot::{Condvar, Mutex};
use renderer::{CoreRenderer, FlatItem, MaskNode, RenderNode};

use crate::{
    clip::ClipArena,
    components::{
        focus::{FocusWithin, Focused},
        layout::{Clip, GlobalTransform, LayoutOutput},
        render::{RenderCtx, RenderItem, RenderOpacity},
    },
    traversal,
};

/// One drawable entity captured for a frame: the shared node cache, its deferred
/// builder, its window-space transform (already composed by M3 layout), the size
/// layout allocated to it (`LayoutOutput::size` — what the builder must draw at),
/// its current opacity (`1.0` if the entity has no `RenderOpacity`), and its
/// focus state.
pub struct RenderItemSnapshot {
    /// Which entity this was extracted from. Nothing on the render path reads
    /// it — it is here so a frame can be traced back to the tree that produced
    /// it, by a debugger or a test asserting on paint order.
    pub entity: Entity,
    pub cache: Arc<Mutex<Option<Arc<RenderNode>>>>,
    pub builder: Arc<dyn Fn(&RenderCtx) -> RenderNode + Send + Sync>,
    pub transform: Matrix4<f32>,
    pub size: [f32; 2],
    pub opacity: f32,
    pub focused: bool,
    pub focus_within: bool,
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
    /// The frame's clips, already paired with their coverage image. Indices in
    /// [`RenderItemSnapshot::clip`] point into this.
    pub clips: Vec<MaskNode>,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub core: Arc<CoreRenderer>,
    pub texture_atlas: Arc<TextureAtlas>,
    pub stencil_atlas: Arc<TextureAtlas>,
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
    // it is opened before the entity's own item is pushed.
    let own_clip = match (world.get::<Clip>(entity).is_some(), transform) {
        (true, Some(transform)) => Some(out.clips.push(clip, transform, size)),
        _ => clip,
    };

    if let (Some(item), Some(transform)) = (world.get::<RenderItem>(entity), transform) {
        let opacity = world
            .get::<RenderOpacity>(entity)
            .map(|o| o.0)
            .unwrap_or(1.0);
        out.items.push(RenderItemSnapshot {
            entity,
            cache: item.cache.clone(),
            builder: item.builder.clone(),
            transform,
            size,
            opacity,
            focused: world.get::<Focused>(entity).is_some(),
            focus_within: world.get::<FocusWithin>(entity).is_some(),
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
        format,
        viewport_size,
        load_color,
        items,
        clips,
        device,
        queue,
        core,
        texture_atlas,
        stencil_atlas,
    } = snapshot;

    let mut nodes: Vec<FlatItem> = Vec::with_capacity(items.len());
    for item in &items {
        // Size and focus vary per item, so `RenderCtx` is built fresh per item
        // rather than shared across the loop. Opacity is deliberately not in
        // it: it is applied at draw time, so it never reaches a builder and
        // never invalidates a cached node.
        let ctx = RenderCtx {
            device: &device,
            queue: &queue,
            texture_atlas: &texture_atlas,
            stencil_atlas: &stencil_atlas,
            size: item.size,
            focused: item.focused,
            focus_within: item.focus_within,
        };
        let node = build_node(&item.cache, &item.builder, &ctx);
        nodes.push(
            FlatItem::new(node, item.transform)
                .with_alpha(item.opacity)
                .with_clip(item.clip),
        );
    }

    let view = surface_texture
        .texture
        .create_view(&wgpu::TextureViewDescriptor::default());

    if let Err(e) = core.render_flat(
        &device,
        &queue,
        format,
        &view,
        viewport_size,
        &nodes,
        &clips,
        load_color,
        &texture_atlas.texture(),
        &stencil_atlas.texture(),
    ) {
        log::error!("render_flat failed for window {window_id:?}: {e}");
    }

    surface_texture.present();
}

/// Fetch (building on first use) an item's render node. In debug builds this
/// first tries a non-blocking lock and warns on contention: the §7.4 invariant
/// is that the main thread and render thread never hold this lock at once.
fn build_node(
    cache: &Arc<Mutex<Option<Arc<RenderNode>>>>,
    builder: &Arc<dyn Fn(&RenderCtx) -> RenderNode + Send + Sync>,
    ctx: &RenderCtx,
) -> Arc<RenderNode> {
    #[cfg(debug_assertions)]
    {
        match cache.try_lock() {
            Some(mut guard) => {
                return guard
                    .get_or_insert_with(|| Arc::new(builder(ctx)))
                    .clone();
            }
            None => {
                log::warn!(
                    "render cache lock contended on render thread; \
                     main and render threads should never lock it at once"
                );
            }
        }
    }

    let mut guard = cache.lock();
    guard.get_or_insert_with(|| Arc::new(builder(ctx))).clone()
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
