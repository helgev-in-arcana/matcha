//! Widget draw producers. Each redraw writes Objects into framework-owned storage.
//! Providers retain expensive shaping/decoding resources, not local Scenes/phases.
//! One shared builder per entity supports immutable in-flight extraction; Objects
//! themselves have no Arc or retained cache. Revision tracks prop invalidation.

use std::sync::Arc;

use crate::scene::Draw;
use bevy_ecs::component::Component;
use std::sync::atomic::{AtomicU64, Ordering};
static NEXT_REVISION: AtomicU64 = AtomicU64::new(1);
fn next_revision() -> u64 {
    NEXT_REVISION
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| v.checked_add(1))
        .expect("render revision space exhausted")
}
pub type DrawBuilder = dyn Fn(&RenderCtx, &mut Draw<'_>) + Send + Sync;

/// CPU-only state handed to a [`RenderItem`] builder. GPU work belongs in
/// render-interface resource generators contributed directly by the Scene.
pub struct RenderCtx {
    /// Resolved widget-local -> UI transform. Background-dependent generators
    /// need this to sample their own region of the full viewport snapshot.
    /// Draw accepts local transforms and resolves placement exactly once.
    pub transform: nalgebra::Matrix4<f32>,
    pub viewport_size: [f32; 2],
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
    /// access when it runs on the render thread. Every redraw reads the extracted
    /// focus state; focus changes also advance the draw revision.
    pub focused: bool,
    /// Whether the focus vertex is this entity or one of its descendants
    /// (CSS `:focus-within`). Always `true` when [`focused`](Self::focused) is.
    pub focus_within: bool,
    /// Whether the pointer is inside this entity's box (CSS `:hover`), whether
    /// directly or via a descendant. Arrives through the context for the same
    /// reason `focused` does; pointer transitions advance the draw revision.
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

/// A lightweight per-redraw writer. Capture reusable provider data, not a Scene.
#[derive(Component, Clone)]
pub struct RenderItem {
    pub revision: u64,
    pub builder: Arc<DrawBuilder>,
}
impl RenderItem {
    pub fn new(builder: impl Fn(&RenderCtx, &mut Draw<'_>) + Send + Sync + 'static) -> Self {
        Self {
            revision: next_revision(),
            builder: Arc::new(builder),
        }
    }
    /// Notify observers of changed draw properties without allocating a cache.
    pub fn invalidate(&mut self) {
        self.revision = next_revision();
    }
}
