//! Scrolling: a clipped viewport whose content can be larger than it is.
//!
//! Layered so the part that is genuinely *about* scrolling stays free of the
//! backend:
//!
//! - [`geometry`] — pure functions over plain numbers. No ECS, no GPU. This is
//!   "the scrollbar as a function of the content length, the position and the
//!   visible width", and it would survive a change of backend unchanged.
//! - this module — the [`ScrollView`] widget: a fixed-size, clipping viewport
//!   holding one content child, plus the wheel handler.
//! - [`scrollbar`] — the bar widgets, ordinary widgets that read their
//!   enclosing `ScrollView`'s state.
//!
//! # How scrolling actually happens
//!
//! [`ScrollViewLayout::arrange`] places the content child at a **negative
//! origin**. That is the whole mechanism: `arrange_child` folds the origin into
//! the child's `GlobalTransform`, and drawing, picking and clipping all read
//! that transform, so they agree about where the content is without any of them
//! knowing that scrolling exists. The `Clip` marker sits on the viewport, whose
//! own box does not move, so the window stays put while the content slides
//! behind it.
//!
//! Nothing has to be invalidated by hand: the schedule re-runs layout every
//! frame, and the existing `invalidate_on_layout_change` picks up anything whose
//! `LayoutOutput` moved. There is no system to register — unlike `animation` and
//! `text_box`, this widget works as soon as it is declared.
//!
//! # Composition
//!
//! Use [`scroll_view`], which declares the viewport and its bars together:
//!
//! ```ignore
//! scroll_view(s, ScrollView::new(300.0, 200.0), |s| {
//!     s.node(Column::new().gap(8.0), |s| { /* content */ });
//! });
//! ```
//!
//! The bars are ordinary child widgets declared *after* the content, which is
//! what puts them on top — `ViewChildren` order is paint order. Because a
//! [`Scrollbar`] is nothing more than a widget that reads its parent's
//! [`ScrollState`], an application that wants a different bar can declare its
//! own in place of the built-in one; there is no slot to register and no
//! function pointer to swap.

pub mod geometry;
pub mod scrollbar;

use std::sync::{
    atomic::{AtomicU32, Ordering},
    Arc,
};

use bevy_ecs::{
    bundle::Bundle, change_detection::DetectChangesMut, component::Component, entity::Entity,
    world::EntityWorldMut,
};
use nalgebra::Matrix4;
use parking_lot::Mutex;

use matcha_ecs::{
    components::{
        input::{Pickable, PointerDispatch, PointerInput, PointerPhase},
        layout::{Clip, GlobalTransform},
        view::Key,
    },
    layout::{Constraints, Layout, LayoutCtx, LayoutDispatch, Measured},
    view::{Scope, Widget},
};

pub use geometry::{Axis, ScrollGeometry, ScrollbarMetrics, ScrollbarStyle};
pub use scrollbar::{ScrollThumb, Scrollbar, ScrollbarSlot};

/// What to do with content that does not fit, per axis.
///
/// Both values clip; they differ in whether the axis can be scrolled at all.
/// There is no "don't clip" value — that is simply not using a `ScrollView`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Overflow {
    /// Measure the content against the viewport on this axis, and clip whatever
    /// still overflows. No scrollbar, and the wheel does nothing.
    ///
    /// Because the content is measured against the viewport, text wraps to the
    /// viewport's width exactly as it would outside a `ScrollView`.
    #[default]
    Hidden,
    /// Measure the content unbounded on this axis, then clip and scroll it.
    /// A bar appears only while the content actually overflows.
    Auto,
}

impl Overflow {
    fn scrolls(self) -> bool {
        matches!(self, Overflow::Auto)
    }
}

/// A pair of `f32`s shared between layout and the input handlers, bit-cast into
/// atomics.
#[derive(Debug, Default)]
struct SharedVec2([AtomicU32; 2]);

impl SharedVec2 {
    fn get(&self) -> [f32; 2] {
        [
            f32::from_bits(self.0[0].load(Ordering::Relaxed)),
            f32::from_bits(self.0[1].load(Ordering::Relaxed)),
        ]
    }

    fn set(&self, v: [f32; 2]) {
        self.0[0].store(v[0].to_bits(), Ordering::Relaxed);
        self.0[1].store(v[1].to_bits(), Ordering::Relaxed);
    }
}

/// Where a thumb drag started, so the offset can be derived from the pointer's
/// total displacement rather than accumulated per-event deltas (which would
/// drift, and would fight the clamp at either end).
#[derive(Debug, Clone, Copy)]
pub(crate) struct DragAnchor {
    pub(crate) axis: Axis,
    /// Pointer position when the drag began, in the viewport's coordinates.
    pub(crate) grab_pos: f32,
    /// Where the thumb's leading edge was at that moment.
    pub(crate) grab_thumb_start: f32,
}

#[derive(Debug, Default)]
struct ScrollStateInner {
    offset: SharedVec2,
    content: SharedVec2,
    viewport: SharedVec2,
    drag: Mutex<Option<DragAnchor>>,
}

/// The live scroll position, and the sizes the bars need to describe it.
///
/// Shared cells rather than plain fields for the same reason `TextBoxLive` uses
/// them: a `Layout` impl only ever gets `&World`, so `arrange` cannot write a
/// component the ordinary way, and the handlers that move the offset run
/// outside the schedule entirely. Cloning is an `Arc` bump, which matters
/// because `LayoutDispatch` clones the layout component on every measure and
/// arrange.
///
/// The bars hold a clone of their viewport's `ScrollState`, so reading it costs
/// them no tree walking.
#[derive(Component, Clone, Default)]
pub struct ScrollState(Arc<ScrollStateInner>);

impl ScrollState {
    /// The current scroll state as the pure geometry layer wants it.
    pub fn geometry(&self) -> ScrollGeometry {
        ScrollGeometry {
            viewport: self.0.viewport.get(),
            content: self.0.content.get(),
            offset: self.0.offset.get(),
        }
    }

    /// The clamped scroll offset in pixels.
    pub fn offset(&self) -> [f32; 2] {
        self.geometry().clamped_offset()
    }

    /// Move to `offset`, clamped to what the content allows. Returns whether
    /// this actually changed anything — which is what decides whether a scroll
    /// was consumed or should chain to an enclosing scroll view.
    pub fn set_offset(&self, offset: [f32; 2]) -> bool {
        let geom = ScrollGeometry {
            offset,
            ..self.geometry()
        };
        let clamped = geom.clamped_offset();
        if clamped == self.0.offset.get() {
            return false;
        }
        self.0.offset.set(clamped);
        true
    }

    /// Publish what layout resolved, and re-clamp the offset against it — the
    /// content may have shrunk since the offset was last set.
    fn publish_sizes(&self, viewport: [f32; 2], content: [f32; 2]) {
        self.0.viewport.set(viewport);
        self.0.content.set(content);
        self.0.offset.set(self.geometry().clamped_offset());
    }

    pub(crate) fn take_drag(&self) -> Option<DragAnchor> {
        *self.0.drag.lock()
    }

    pub(crate) fn set_drag(&self, anchor: Option<DragAnchor>) {
        *self.0.drag.lock() = anchor;
    }
}

/// Which axes scroll, resolved for the pure geometry layer.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub struct ScrollAxes(pub [bool; 2]);

/// The scrollbar styling, carried so the bars can read it off their parent.
#[derive(Component, Clone, Copy, PartialEq, Debug)]
pub struct ScrollbarLook(pub ScrollbarStyle);

/// A [`ScrollView`]'s fixed size and overflow behaviour — both the data and the
/// `Layout` impl, mirroring `PanelLayout`.
#[derive(Component, Clone, Copy, PartialEq, Debug)]
pub struct ScrollViewLayout {
    pub w: f32,
    pub h: f32,
    pub overflow_x: Overflow,
    pub overflow_y: Overflow,
}

impl ScrollViewLayout {
    fn axes(&self) -> [bool; 2] {
        [self.overflow_x.scrolls(), self.overflow_y.scrolls()]
    }
}

/// A fixed-size viewport that clips its content and can scroll it.
///
/// Vertical by default: `ScrollView::new(w, h)` scrolls up and down and wraps
/// text to `w`, which is what a block of prose wants. Horizontal scrolling is
/// opt-in via [`ScrollView::overflow_x`] — see its docs for what that costs.
pub struct ScrollView {
    key: Key,
    w: f32,
    h: f32,
    overflow_x: Overflow,
    overflow_y: Overflow,
    style: ScrollbarStyle,
}

impl ScrollView {
    /// A `w`x`h` viewport that scrolls vertically.
    pub fn new(w: f32, h: f32) -> Self {
        Self {
            key: Key::Auto,
            w,
            h,
            overflow_x: Overflow::Hidden,
            overflow_y: Overflow::Auto,
            style: ScrollbarStyle::default(),
        }
    }

    /// Whether the content may scroll horizontally. Defaults to
    /// [`Overflow::Hidden`].
    ///
    /// Note what enabling this does to text. A scrollable axis is measured
    /// unbounded, and `Text`/`RichText` take their wrap width from the width
    /// they are measured against — so a horizontally scrollable view does not
    /// wrap text at all, behaving like CSS `white-space: nowrap` rather than
    /// CSS `overflow-x`. That is usually what a timeline or a code view wants,
    /// and usually not what a paragraph wants.
    pub fn overflow_x(mut self, overflow: Overflow) -> Self {
        self.overflow_x = overflow;
        self
    }

    /// Whether the content may scroll vertically. Defaults to
    /// [`Overflow::Auto`].
    pub fn overflow_y(mut self, overflow: Overflow) -> Self {
        self.overflow_y = overflow;
        self
    }

    /// Thickness of the scrollbars, across their short axis.
    pub fn thickness(mut self, thickness: f32) -> Self {
        self.style.thickness = thickness;
        self
    }

    /// Margin between a scrollbar and the viewport's edges.
    pub fn gap(mut self, gap: f32) -> Self {
        self.style.gap = gap;
        self
    }

    /// Corner radius of the scrollbar track and thumb.
    pub fn radius(mut self, radius: f32) -> Self {
        self.style.radius = radius;
        self
    }

    pub fn thumb_color(mut self, color: [f32; 4]) -> Self {
        self.style.thumb_color = color;
        self
    }

    /// Colour of the track behind the thumb; `None` leaves the thumb floating
    /// over the content with no track drawn.
    pub fn track_color(mut self, color: Option<[f32; 4]>) -> Self {
        self.style.track_color = color;
        self
    }

    /// Replace the whole scrollbar style at once.
    pub fn style(mut self, style: ScrollbarStyle) -> Self {
        self.style = style;
        self
    }

    pub fn key(mut self, key: impl Into<Key>) -> Self {
        self.key = key.into();
        self
    }

    /// The style the bars will read off this viewport.
    pub fn scrollbar_style(&self) -> ScrollbarStyle {
        self.style
    }

    fn layout(&self) -> ScrollViewLayout {
        ScrollViewLayout {
            w: self.w,
            h: self.h,
            overflow_x: self.overflow_x,
            overflow_y: self.overflow_y,
        }
    }
}

/// Declare a scroll view with its scrollbars.
///
/// This is a function rather than a plain `s.node(..)` because the bars are
/// child entities and only the view function can declare children — see the
/// module docs. `content` declares the single child that scrolls.
pub fn scroll_view(s: &mut Scope, view: ScrollView, content: impl FnOnce(&mut Scope)) {
    let style = view.scrollbar_style();
    s.node(view, |s| {
        content(s);
        // Declared after the content, so they paint over it. Both are declared
        // unconditionally: whether an axis overflows is only known after
        // layout, and a bar with nothing to scroll measures to nothing and
        // draws nothing.
        for axis in [Axis::Y, Axis::X] {
            s.node(Scrollbar::new(axis, style), |s| {
                s.leaf(ScrollThumb::new(axis, style));
            });
        }
    });
}

/// Pointer handling for the viewport itself: the wheel, and continuing a thumb
/// drag.
///
/// Dragging is handled here rather than on the thumb so that one implementation
/// serves both; the thumb only records where it was grabbed. The press captures
/// the pointer (`matcha_ecs::input::PointerCapture`), so the drag keeps
/// arriving here for as long as the button is held, wherever in the window the
/// cursor goes.
fn on_pointer(entity: &mut EntityWorldMut, input: &PointerInput) -> bool {
    let Some(state) = entity.get::<ScrollState>().cloned() else {
        return false;
    };
    let Some(axes) = entity.get::<ScrollAxes>().copied() else {
        return false;
    };
    let Some(look) = entity.get::<ScrollbarLook>().copied() else {
        return false;
    };

    match input.phase {
        PointerPhase::Scroll { delta } => {
            // Winit's sign: a positive delta means "scroll up", i.e. move the
            // content down, which is a smaller offset.
            let current = state.offset();
            let target = [
                if axes.0[0] {
                    current[0] - delta[0]
                } else {
                    current[0]
                },
                if axes.0[1] {
                    current[1] - delta[1]
                } else {
                    current[1]
                },
            ];
            // Not consuming a scroll we cannot act on is what produces scroll
            // chaining: the event bubbles on to the next scrollable ancestor.
            state.set_offset(target)
        }

        PointerPhase::Press { .. } => {
            // A press that reached the viewport landed on neither thumb nor
            // track (those consume it), so whatever drag was in progress is
            // over. Left unconsumed so it still reaches the content.
            state.set_drag(None);
            false
        }

        PointerPhase::Drag => {
            let Some(anchor) = state.take_drag() else {
                return false;
            };
            let main = anchor.axis.main();
            // `local_pos` is relative to the viewport, the same space the thumb
            // recorded its grab in. Deriving from total displacement rather
            // than accumulating per-event deltas keeps the thumb pinned to the
            // cursor even after the offset clamps at either end.
            let thumb_start = anchor.grab_thumb_start + (input.local_pos[main] - anchor.grab_pos);
            let mut target = state.offset();
            target[main] = geometry::offset_for_thumb_start(
                anchor.axis,
                &state.geometry(),
                axes.0,
                thumb_start,
                &look.0,
            );
            state.set_offset(target);
            // Consumed regardless of whether the offset actually moved: the
            // drag belongs to this scrollbar, and must not also drag out a text
            // selection in the content behind it.
            true
        }
    }
}

impl Widget for ScrollView {
    fn key(&self) -> Key {
        self.key
    }

    fn bundle(&self) -> impl Bundle {
        (
            self.layout(),
            ScrollState::default(),
            ScrollAxes(self.layout().axes()),
            ScrollbarLook(self.style),
            // The viewport's own box is the clip, and it does not move when the
            // content scrolls.
            Clip,
            // Without this the wheel never arrives: dispatch starts from a pick
            // hit, and picking only records entities that are `Pickable`. Plain
            // containers and text are not, so a scroll view full of them would
            // have nothing under the pointer at all.
            Pickable,
            LayoutDispatch::of::<ScrollViewLayout>(),
            PointerDispatch::new(on_pointer),
        )
    }

    fn patch(&self, entity: &mut EntityWorldMut) {
        if let Some(mut l) = entity.get_mut::<ScrollViewLayout>() {
            l.set_if_neq(self.layout());
        }
        if let Some(mut a) = entity.get_mut::<ScrollAxes>() {
            a.set_if_neq(ScrollAxes(self.layout().axes()));
        }
        if let Some(mut s) = entity.get_mut::<ScrollbarLook>() {
            s.set_if_neq(ScrollbarLook(self.style));
        }
        // `ScrollState` is deliberately untouched: it is live interaction
        // state, not something the view declares. Re-declaring a `ScrollView`
        // must not throw away where the user had scrolled to.
    }
}

impl Layout for ScrollViewLayout {
    fn measure(&self, _ctx: &mut LayoutCtx, _me: Entity, c: Constraints) -> Measured {
        // `clamp`, not `min`: a parent `Column`/`Row` with the default
        // `AlignItems::Stretch` passes `min == max`, and ignoring the minimum
        // would leave `LayoutOutput` disagreeing with the space actually
        // allocated — which would desync the clip rect and the pick rect from
        // what is drawn.
        Measured::exact([
            self.w.clamp(c.min_width(), c.max_width()),
            self.h.clamp(c.min_height(), c.max_height()),
        ])
    }

    fn arrange(&self, ctx: &mut LayoutCtx, me: Entity, size: [f32; 2]) {
        let my_affine = ctx
            .world()
            .get::<GlobalTransform>(me)
            .map(|t| t.affine)
            .unwrap_or_else(Matrix4::identity);
        let Some(state) = ctx.world().get::<ScrollState>(me).cloned() else {
            return;
        };

        // Content is whatever child is not a scrollbar. Identified by the
        // marker rather than by position, so the bars may be declared in any
        // order (or not at all, for a view driven only by the wheel).
        let children = ctx.children(me);
        let (bars, content): (Vec<Entity>, Vec<Entity>) = children
            .into_iter()
            .partition(|&e| ctx.world().get::<ScrollbarSlot>(e).is_some());

        if let Some(&child) = content.first() {
            let axes = self.axes();
            // A scrollable axis is measured unbounded, so the content reports
            // its natural extent instead of being squeezed into the viewport.
            //
            // A non-scrolling axis is pinned to `min == max == viewport`, not
            // left as `[0, viewport]`: `Column`/`Row` measure fit-content, so a
            // loose maximum would give the content a box narrower than the
            // viewport, and any `AlignItems::Stretch` inside it would then
            // stretch to the content rather than to the viewport.
            let span = |i: usize| {
                if axes[i] {
                    [0.0, Constraints::UNBOUNDED]
                } else {
                    [size[i], size[i]]
                }
            };
            let content_size = ctx.measure_child_size(child, Constraints::new(span(0), span(1)));

            state.publish_sizes(size, content_size);
            let offset = state.offset();
            // The negative origin *is* the scroll. Everything downstream —
            // drawing, picking, clipping — reads the resulting transform.
            ctx.arrange_child(child, [-offset[0], -offset[1]], my_affine, content_size);
        } else {
            state.publish_sizes(size, [0.0, 0.0]);
        }

        // Bars overlay the viewport and do not scroll with the content. Each is
        // placed at exactly its track rectangle so that its box *is* its hit
        // area — giving a bar the whole viewport would make it occlude every
        // click meant for the content behind it.
        let style = ctx
            .world()
            .get::<ScrollbarLook>(me)
            .map(|s| s.0)
            .unwrap_or_default();
        let geom = state.geometry();
        for bar in bars {
            let track = ctx
                .world()
                .get::<ScrollbarSlot>(bar)
                .map(|slot| slot.0)
                .and_then(|axis| geometry::metrics(axis, &geom, self.axes(), &style))
                .map(|m| m.track);
            match track {
                Some([x, y, w, h]) => ctx.arrange_child(bar, [x, y], my_affine, [w, h]),
                // Nothing to scroll on this axis: collapse the bar so it
                // neither paints nor picks.
                None => ctx.arrange_child(bar, [0.0, 0.0], my_affine, [0.0, 0.0]),
            }
        }
    }
}
