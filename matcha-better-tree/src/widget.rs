use std::any::Any;
use std::sync::atomic::{AtomicU64, Ordering};

use matcha_window::event::device_event::DeviceEvent;
use nalgebra::Matrix4;
use smallvec::SmallVec;

use crate::context::Context;
use crate::metrics::{Bounds, Constraints, Point};
pub use crate::ui_tree::{Children, DeclContext};

const COLOR_SMALLVEC: usize = 2;

/// Handle to a child, issued by `DeclContext::child`.
///
/// Unique across the whole process (all parents, frames and arenas), so a stale id, or one that
/// leaked through the model into another widget, resolves to nothing instead of to the wrong
/// child. Stable across frames for as long as the child is reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChildId {
    uid: u64,
}

impl ChildId {
    pub(crate) fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        // Only uniqueness is needed; no other memory is published through this counter.
        Self {
            uid: NEXT.fetch_add(1, Ordering::Relaxed),
        }
    }
}

pub trait View: Any {
    fn label(&self) -> Option<&str>;
    /// Explicit identity among siblings. `None` matches by type and order of appearance.
    fn key(&self) -> Option<u64> {
        None
    }
    fn build<'a>(&'a self, ctx: &mut DeclContext<'a>) -> impl Widget<V = Self>;
}

/// The widget's own update result. Child structure and descendant changes are detected
/// separately by the framework. Rendering cache invalidation is not part of this API yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub enum UpdateResult {
    /// This widget's state affecting measurement or child layout is unchanged.
    Unchanged,
    /// Invalidate this widget's and its ancestors' measurement and layout caches.
    Changed,
}

/// Whether the current event should reach further widgets or the next dispatch phase.
/// This does not capture future pointer events or affect cache invalidation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputPropagation {
    Continue,
    Stop,
}

/// Local state changes and event propagation are independent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub struct InputResult {
    pub update: UpdateResult,
    pub propagation: InputPropagation,
}

impl Default for InputResult {
    fn default() -> Self {
        Self {
            update: UpdateResult::Unchanged,
            propagation: InputPropagation::Continue,
        }
    }
}

pub trait Widget: Any {
    type V: View;

    /// Update this widget and declare every child, even when returning `Unchanged`.
    /// Report `Changed` when state affecting `measure` or `layout` changes. Descendant
    /// caches are retained until their own update or Constraints/Bounds keys require work.
    fn update<'a>(&mut self, view: &'a Self::V, ctx: &mut DeclContext<'a>) -> UpdateResult;

    /// Observe or intercept an event from root to target (inclusive), before normal input.
    /// `mouse_position()` is widget-local. Stop cancels the rest of preview and normal input.
    fn preview_input(&mut self, bounds: Bounds, event: &DeviceEvent, ctx: Context) -> InputResult {
        let _ = (bounds, event, ctx);
        InputResult::default()
    }

    /// Handle an event from target to root. `mouse_position()` is widget-local.
    /// The framework routes events; widgets do not forward them to children.
    /// Report Changed for state affecting measurement/layout, even when stopping propagation.
    fn input(&mut self, bounds: Bounds, event: &DeviceEvent, ctx: Context) -> InputResult;

    /// Test a point in this widget's local coordinates. The framework applies the relative
    /// affine's inverse before calling this method. Test only this widget's own shape;
    /// descendants are searched separately, even when this method returns false.
    fn is_inside(&self, b: Bounds, p: Point, ctx: Context) -> bool {
        let _ = ctx;

        (0.0 <= p.x && p.x <= b.x) && (0.0 <= p.y && p.y <= b.y) && (0.0 <= p.z && p.z <= b.z)
    }

    fn measure(&self, constraints: Constraints, children: Children<'_>, ctx: Context) -> Bounds;

    /// Lay out this widget's children using its assigned local bounds.
    /// Any child placements passed to `Children::layout` are relative to this widget.
    fn layout(&self, bounds: Bounds, children: Children<'_>, ctx: Context);

    fn render(&self, bounds: Bounds, ctx: Context) -> Draw;
}

pub struct Draw {
    pub color: SmallVec<[Color; COLOR_SMALLVEC]>,
    pub mask: Option<Mask>,
}

pub struct Color {
    pub mesh: render_interface::MeshSource,
    pub texture: render_interface::TextureSource,
    pub transform: Matrix4<f32>,
    pub opacity: f32,
}

pub struct Mask {
    pub mesh: render_interface::MeshSource,
    pub mask: render_interface::MaskSource,
    pub transform: Matrix4<f32>,
}

pub enum UiError {}
