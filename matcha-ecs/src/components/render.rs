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
    /// The size layout allocated to this entity (`LayoutOutput::size`).
    /// Builders must draw at *this* size, not a constructor-declared one: a
    /// parent layout may allocate more than the widget asked for (e.g.
    /// `AlignItems::Stretch`, or a min-size constraint), and the entity's
    /// hit-test rect and child arrangement already use the allocated size —
    /// drawing at any other size desynchronises paint from layout.
    /// `[0.0, 0.0]` if the entity has never been laid out.
    pub size: [f32; 2],
    /// Whether this entity is the focus vertex (CSS `:focus`).
    ///
    /// Focus has to arrive through the context rather than being read from the
    /// world, for the same reason `size` does: a builder is a
    /// closure captured back at `bundle()`/`patch()` time and has no world
    /// access when it runs (on the render thread, no less). The rebuild is
    /// triggered by `focus::sync_focus_components`, which invalidates the
    /// cached node of every entity whose focus state changed.
    pub focused: bool,
    /// Whether the focus vertex is this entity or one of its descendants
    /// (CSS `:focus-within`). Always `true` when [`focused`](Self::focused) is.
    pub focus_within: bool,
    /// Whether the pointer is inside this entity's box (CSS `:hover`), whether
    /// directly or via a descendant. Arrives through the context for the same
    /// reason `focused` does; `pointer::sync_pointer_components` invalidates
    /// the cached node on every transition.
    pub hovered: bool,
    /// Whether a held press landed inside this entity and the pointer has not
    /// left it since (CSS `:active`).
    pub active: bool,
}

/// A widget's current opacity, `0.0` (invisible) to `1.0` (fully visible).
///
/// One of the two components the extract stage reads off a drawable entity
/// (the other being `GlobalTransform`). The core only ever *reads* it: whoever
/// wants to animate opacity writes it from a registered PreLayout system. An
/// entity without this component renders at full opacity.
///
/// Applied at draw time, so changing it costs nothing beyond a redraw — a fade
/// does not re-rasterise anything, and a builder never sees it.
#[derive(Component, Clone, Copy, PartialEq, Debug)]
pub struct RenderOpacity(pub f32);

/// Where this entity sits among its siblings, low to high; declaration order
/// breaks ties. Absent means `0`.
///
/// Reorders **painting and picking together** — see [`crate::traversal`],
/// which is the only thing that reads it, and whose docs give the two
/// restrictions that keep stacking to a single stable sort. A subtree moves as
/// a unit, and a child never goes behind its parent however negative this is.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub struct ZIndex(pub i32);

impl Default for RenderOpacity {
    fn default() -> Self {
        RenderOpacity(1.0)
    }
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
