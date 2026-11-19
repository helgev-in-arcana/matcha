use std::any::Any;

pub use matcha_window::adapter::ControlFlow;
use matcha_window::event::device_event::DeviceEvent;
use nalgebra::Matrix4;
use smallvec::SmallVec;

use crate::metrics::{Bounds, Constraints, Point};
use crate::ui_tree::Children;
use crate::ui_tree::context::Context;
pub use crate::ui_tree::{DeclContext, MountKind};

const COLOR_SMALLVEC: usize = 2;

pub trait View: Any {
    /// Properties this parent associates with each child. Equal values must be
    /// interchangeable for measurement and layout. Use `()` for ordinary children.
    type ChildData: PartialEq + utils::MaybeSendSync + 'static;

    fn label(&self) -> Option<&str>;
    /// Explicit identity among siblings. `None` matches by type and order of appearance.
    fn key(&self) -> Option<u64> {
        None
    }
    fn build<'a>(&'a self, ctx: &mut DeclContext<'a, '_, Self::ChildData>)
    -> impl Widget<V = Self>;
}

pub trait Widget: Any + utils::MaybeSendSync {
    type V: View;

    /// Used by the framework when it mounts this widget with its child links.
    fn mount_kind() -> MountKind<Self>
    where
        Self: Sized,
    {
        MountKind::widget()
    }

    /// Update this widget and declare every child, even when returning empty flags.
    /// Report LAYOUT for changes affecting measurement/child layout, RENDER for changes to
    /// this widget's drawing, or LAYOUT | RENDER for both. Descendant layout caches are retained until
    /// their own update or Constraints/Bounds keys require work.
    fn update<'a>(
        &mut self,
        view: &'a Self::V,
        ctx: &mut DeclContext<'a, '_, <Self::V as View>::ChildData>,
    ) -> UpdateResult;

    /// Advance local state before measurement, layout, and rendering.
    /// Every retained widget in the current window is polled, children first, even while
    /// awaiting entry or exit. Read the shared pass time through `ctx.frame_time()`.
    /// Each full pass replaces previous requests, so an early poll must repeat any deadline
    /// that still matters. `WaitUntil` requests a wakeup; execution may occur later.
    ///
    /// An active child omitted from its parent's declaration is polled as
    /// [`WidgetPhase::Exiting`]. Return [`Lifecycle::FinishExit`] to drop it on this pass,
    /// or [`Lifecycle::Keep`] to keep it in layout and drawing. A new declaration with
    /// the same explicit key and view type updates that widget and cancels its exit.
    /// Use [`WidgetPhase::default_lifecycle`] for phases this widget does not customize.
    fn poll(&mut self, info: PollInfo, _ctx: &mut Context) -> Lifecycle {
        info.phase.default_lifecycle()
    }

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
        children: Children<'_, <Self::V as View>::ChildData>,
        ctx: &mut Context,
    ) -> Bounds;

    /// Lay out this widget's children using its assigned local bounds.
    /// Any child placements passed to [`Child::layout`](super::Child::layout) are relative to this widget.
    /// Re-running layout alone does not invalidate this widget's drawing. Updates affecting
    /// both child layout and this widget's appearance must also request RENDER.
    fn layout(
        &self,
        bounds: Bounds,
        children: Children<'_, <Self::V as View>::ChildData>,
        ctx: &mut Context,
    );

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

/// This widget's participation in its parent's layout. An inactive ancestor can still
/// hide an active descendant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WidgetPhase {
    /// Mounted and polled, but not yet included in layout, drawing, or hit testing.
    PendingEnter,
    /// Declared and participating in the tree.
    Active,
    /// No longer declared. Still included in layout and drawing, but not in hit testing.
    Exiting,
}

impl WidgetPhase {
    /// Enter or finish exiting on this poll; while active, request no work.
    pub fn default_lifecycle(self) -> Lifecycle {
        match self {
            Self::PendingEnter => Lifecycle::Enter {
                update: UpdateResult::empty(),
                next_poll: ControlFlow::Wait,
            },
            Self::Active => Lifecycle::default(),
            Self::Exiting => Lifecycle::FinishExit,
        }
    }
}

/// Tree state observed after polling this widget's children.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PollInfo {
    pub phase: WidgetPhase,
    /// A direct child joined, left, or changed position in the active child list since
    /// mounting or the previous poll. Multiple changes are reported once. Descendant
    /// cache changes and updates to a child's properties alone do not set this flag.
    pub active_children_changed: bool,
}

/// This widget's requested action. Only the cache and timing requests are aggregated;
/// entry and exit apply to this node alone.
#[derive(Debug, Clone, Copy, PartialEq)]
#[must_use]
pub enum Lifecycle {
    /// Keep the current phase. In particular, `ControlFlow::Wait` does not remove a widget.
    Keep {
        update: UpdateResult,
        next_poll: ControlFlow,
    },
    /// Join the active children. The tree handles relayout. In any other phase
    /// this behaves like `Keep` with the same work.
    Enter {
        update: UpdateResult,
        next_poll: ControlFlow,
    },
    /// Release this subtree, including its wake requests. In any phase other than
    /// `Exiting`, this has no effect.
    FinishExit,
}

impl Default for Lifecycle {
    fn default() -> Self {
        Self::Keep {
            update: UpdateResult::empty(),
            next_poll: ControlFlow::Wait,
        }
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
