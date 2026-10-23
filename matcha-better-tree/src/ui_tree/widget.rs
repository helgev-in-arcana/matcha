use std::any::Any;

use matcha_window::event::device_event::DeviceEvent;
use nalgebra::Matrix4;
use smallvec::SmallVec;

use crate::metrics::{Bounds, Constraints, Point};
use crate::ui_tree::Children;
pub use crate::ui_tree::DeclContext;
use crate::ui_tree::context::Context;

const COLOR_SMALLVEC: usize = 2;

pub trait View: Any {
    fn label(&self) -> Option<&str>;
    /// Explicit identity among siblings. `None` matches by type and order of appearance.
    fn key(&self) -> Option<u64> {
        None
    }
    fn build<'a>(&'a self, ctx: &mut DeclContext<'a, '_>) -> impl Widget<V = Self>;
}

pub trait Widget: Any {
    type V: View;

    /// Update this widget and declare every child, even when returning empty flags.
    /// Report LAYOUT for changes affecting measurement/child layout, RENDER for changes to
    /// this widget's drawing, or LAYOUT | RENDER for both. Descendant layout caches are retained until
    /// their own update or Constraints/Bounds keys require work.
    fn update<'a>(&mut self, view: &'a Self::V, ctx: &mut DeclContext<'a, '_>) -> UpdateResult;

    /// Observe or intercept an event from root to target (inclusive), before normal input.
    /// `mouse_position()` is widget-local. Stop cancels the rest of preview and normal input.
    fn preview_input(
        &mut self,
        bounds: Bounds,
        event: &DeviceEvent,
        ctx: &mut Context,
    ) -> InputResult {
        let _ = (bounds, event, ctx);
        InputResult::default()
    }

    /// Handle an event from target to root. `mouse_position()` is widget-local.
    /// The framework routes events; widgets do not forward them to children.
    /// Report LAYOUT and/or RENDER independently, even when stopping propagation.
    fn input(&mut self, bounds: Bounds, event: &DeviceEvent, ctx: &mut Context) -> InputResult;

    /// Test a point in this widget's local coordinates. The framework applies the relative
    /// affine's inverse before calling this method. Test only this widget's own shape;
    /// descendants are searched separately, even when this method returns false.
    fn is_inside(&self, b: Bounds, p: Point, ctx: &mut Context) -> bool {
        let _ = ctx;

        (0.0 <= p.x && p.x <= b.x) && (0.0 <= p.y && p.y <= b.y) && (0.0 <= p.z && p.z <= b.z)
    }

    fn measure(
        &self,
        constraints: Constraints,
        children: Children<'_>,
        ctx: &mut Context,
    ) -> Bounds;

    /// Lay out this widget's children using its assigned local bounds.
    /// Any child placements passed to `Children::layout` are relative to this widget.
    /// Re-running layout alone does not invalidate this widget's drawing. Updates affecting
    /// both child layout and this widget's appearance must also request RENDER.
    fn layout(&self, bounds: Bounds, children: Children<'_>, ctx: &mut Context);

    /// Whether any source returned by render reads the phase-start snapshot (Backdrop).
    /// The minimal pipeline refreshes this widget and its descendants on every Scene build,
    /// since earlier siblings and the renderer's external initial image can change independently.
    fn uses_backdrop(&self) -> bool {
        false
    }

    /// Produce widget-local drawing definitions. The framework assigns the submitted content
    /// IDs and retains them while the draw cache is valid. Generators must capture immutable
    /// parameters; mutable external resources require an explicit update result.
    /// If any generator reads the snapshot, uses_backdrop must return true.
    fn render(&self, bounds: Bounds, ctx: &mut Context) -> Draw;
}

bitflags::bitflags! {
    /// Independent invalidation requests for this widget. Empty means no change.
    /// Child structure and descendant layout changes are detected separately by the framework.
    #[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
    #[must_use]
    pub struct UpdateResult: u8 {
        /// Invalidate measurement and child layout on this widget and its ancestors.
        /// Does not invalidate drawing when the widget's own Bounds stay unchanged.
        const LAYOUT = 1 << 0;
        /// Invalidate this widget's local drawing without invalidating measurement/layout.
        const RENDER = 1 << 1;
    }
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
            update: UpdateResult::empty(),
            propagation: InputPropagation::Continue,
        }
    }
}

/// Own content is painted before children, which are painted in declaration order.
#[derive(Clone, Default)]
pub struct Draw {
    pub color: SmallVec<[Color; COLOR_SMALLVEC]>,
    /// Coverage applied to this widget's colors and its entire descendant subtree.
    /// Nested coverage multiplies. This is a visual mask, not a pointer hit-test shape.
    pub mask: Option<Mask>,
}

#[derive(Clone)]
pub struct Color {
    pub mesh: render_interface::MeshSource,
    pub texture: render_interface::TextureSource,
    /// Object-local -> widget-local; ancestors' placement is composed during Scene assembly.
    pub transform: Matrix4<f32>,
    pub opacity: f32,
}

#[derive(Clone)]
pub struct Mask {
    pub mesh: render_interface::MeshSource,
    pub mask: render_interface::MaskSource,
    /// Mask-local -> widget-local. Parent masks contribute coverage, not another transform.
    pub transform: Matrix4<f32>,
}

#[derive(Debug, thiserror::Error)]
pub enum UiError {
    #[error("layout must be up to date before rendering")]
    LayoutRequired,
    #[error(transparent)]
    Resource(#[from] render_interface::DuplicateResource),
    #[error("mask index capacity exceeded")]
    MaskIndexOverflow,
}
