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

use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    thread::JoinHandle,
};

use bevy_ecs::{entity::Entity, world::World};
use gpu_utils::texture_atlas::TextureAtlas;
use matcha_window::window::WindowId;
use nalgebra::Matrix4;
use parking_lot::Mutex;
use renderer::{CoreRenderer, RenderNode};

use crate::{
    animation::{Animated, Opacity},
    components::{
        layout::GlobalTransform,
        render::{RenderCtx, RenderItem},
        view::ViewChildren,
    },
};

/// One drawable entity captured for a frame: the shared node cache, its deferred
/// builder, its window-space transform (already composed by M3 layout), and its
/// current opacity (M7; `1.0` if the entity has no `Animated<Opacity>`).
pub struct RenderItemSnapshot {
    pub cache: Arc<Mutex<Option<Arc<RenderNode>>>>,
    pub builder: Arc<dyn Fn(&RenderCtx) -> RenderNode + Send + Sync>,
    pub transform: Matrix4<f32>,
    pub opacity: f32,
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
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub core: Arc<CoreRenderer>,
    pub texture_atlas: Arc<TextureAtlas>,
    pub stencil_atlas: Arc<TextureAtlas>,
}

/// Depth-first (paint order) collection of a window root's drawable entities.
/// Clones each entity's `RenderItem` (the `cache`/`builder` `Arc`s are shared,
/// not deep-copied) and its `GlobalTransform`; the builder is not invoked here.
pub fn extract_items(world: &World, root_entity: Entity) -> Vec<RenderItemSnapshot> {
    let mut out = Vec::new();
    extract_recursive(world, root_entity, &mut out);
    out
}

fn extract_recursive(world: &World, entity: Entity, out: &mut Vec<RenderItemSnapshot>) {
    let Some(view_children) = world.get::<ViewChildren>(entity) else {
        return;
    };
    let children: Vec<Entity> = view_children.slots.iter().map(|(_, e)| *e).collect();

    for child in children {
        if let (Some(item), Some(transform)) = (
            world.get::<RenderItem>(child),
            world.get::<GlobalTransform>(child),
        ) {
            let opacity = world
                .get::<Animated<Opacity>>(child)
                .map(|a| a.0 .0)
                .unwrap_or(1.0);
            out.push(RenderItemSnapshot {
                cache: item.cache.clone(),
                builder: item.builder.clone(),
                transform: transform.affine,
                opacity,
            });
        }
        extract_recursive(world, child, out);
    }
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
        device,
        queue,
        core,
        texture_atlas,
        stencil_atlas,
    } = snapshot;

    let mut nodes: Vec<(Arc<RenderNode>, Matrix4<f32>)> = Vec::with_capacity(items.len());
    for item in &items {
        // Opacity varies per item (M7), so `RenderCtx` is built fresh per item
        // rather than shared across the loop.
        let ctx = RenderCtx {
            device: &device,
            queue: &queue,
            texture_atlas: &texture_atlas,
            stencil_atlas: &stencil_atlas,
            opacity: item.opacity,
        };
        let node = build_node(&item.cache, &item.builder, &ctx);
        nodes.push((node, item.transform));
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
    /// `true` between `dispatch` and the worker finishing that frame.
    busy: Arc<AtomicBool>,
    _handle: JoinHandle<()>,
}

impl RenderDriver for ThreadDriver {
    fn dispatch(&mut self, snapshot: RenderSnapshot) {
        let window_id = snapshot.window_id;
        let thread = self
            .threads
            .entry(window_id)
            .or_insert_with(|| WindowThread::spawn(window_id));

        thread.busy.store(true, Ordering::Release);
        if let Err(e) = thread.sender.send(snapshot) {
            thread.busy.store(false, Ordering::Release);
            log::error!("render thread for window {window_id:?} has gone away: {e}");
        }
    }

    fn is_busy(&self, window: WindowId) -> bool {
        self.threads
            .get(&window)
            .map(|t| t.busy.load(Ordering::Acquire))
            .unwrap_or(false)
    }
}

impl WindowThread {
    fn spawn(window_id: WindowId) -> Self {
        let (sender, receiver) = mpsc::channel::<RenderSnapshot>();
        let busy = Arc::new(AtomicBool::new(false));
        let busy_worker = busy.clone();

        let handle = std::thread::Builder::new()
            .name(format!("matcha render ({window_id:?})"))
            .spawn(move || {
                while let Ok(snapshot) = receiver.recv() {
                    build_and_present(snapshot);
                    busy_worker.store(false, Ordering::Release);
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
