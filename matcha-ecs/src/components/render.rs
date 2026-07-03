//! Rendering component: a deferred, cached render-tree builder per widget entity.
//!
//! A widget entity that draws carries a [`RenderItem`]. It does not hold a
//! `RenderNode` directly; instead it holds a `builder` closure that produces one
//! given GPU resources ([`RenderCtx`]), plus a shared `cache` slot. The render
//! stage lazily fills the cache on first use and reuses it on subsequent frames.
//! Invalidation ([`RenderItem::invalidate`]) swaps the cache for a fresh empty
//! slot so the next frame rebuilds it.

use std::sync::Arc;

use bevy_ecs::component::Component;
use gpu_utils::texture_atlas::TextureAtlas;
use parking_lot::Mutex;
use renderer::RenderNode;

/// GPU resources handed to a [`RenderItem`] builder so it can allocate atlas
/// space and record draw commands while producing its [`RenderNode`].
pub struct RenderCtx<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub texture_atlas: &'a TextureAtlas,
    pub stencil_atlas: &'a TextureAtlas,
}

/// A cached, deferred render-tree source for one widget entity.
///
/// The `builder` captures the widget's draw-relevant props (color, size, …) and
/// returns a [`RenderNode`] when invoked. `cache` memoises the last built node so
/// unchanged entities are not re-rasterised every frame. Widgets must call
/// [`invalidate`](Self::invalidate) from their `patch` when (and only when) a
/// draw-relevant prop changed, since `RenderItem` cannot implement `PartialEq`.
#[derive(Component, Clone)]
pub struct RenderItem {
    pub cache: Arc<Mutex<Option<Arc<RenderNode>>>>,
    pub builder: Arc<dyn Fn(&RenderCtx) -> RenderNode + Send + Sync>,
}

impl RenderItem {
    /// Create a `RenderItem` from a builder closure. The cache starts empty and
    /// is filled lazily by the render stage.
    pub fn new(builder: impl Fn(&RenderCtx) -> RenderNode + Send + Sync + 'static) -> Self {
        Self {
            cache: Arc::new(Mutex::new(None)),
            builder: Arc::new(builder),
        }
    }

    /// Drop the cached node so the next render rebuilds it. Swaps in a fresh
    /// cache `Arc` rather than clearing the existing one, so any in-flight reader
    /// holding the old `Arc` is unaffected.
    pub fn invalidate(&mut self) {
        self.cache = Arc::new(Mutex::new(None));
    }
}
