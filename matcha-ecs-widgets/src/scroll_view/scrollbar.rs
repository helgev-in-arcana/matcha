//! The scrollbar widgets.
//!
//! A [`Scrollbar`] is an **ordinary widget** that happens to read its parent
//! [`ScrollView`](super::ScrollView)'s [`ScrollState`]. It owns no scroll state
//! of its own, so an application that wants a differently-shaped bar can simply
//! declare its own widget following the same convention instead of the built-in
//! one — there is no slot to register and no function pointer to install.
//!
//! # Why the thumb is its own entity
//!
//! It would be shorter to draw the thumb inside the bar's own `RenderItem`, but
//! then its position would be baked into a cached render node and moving it
//! would need a hand-written invalidation path. As a child entity its position
//! is written by `arrange_child` like any other widget's, so the existing
//! `invalidate_on_layout_change` handles redrawing, and the thumb's hit area is
//! whatever picking already computed from its box. The bar and the thumb being
//! separately `Pickable` is also what lets "click the track" and "drag the
//! thumb" be two plain handlers rather than one handler doing hit-testing by
//! hand.

use bevy_ecs::{
    bundle::Bundle, change_detection::DetectChangesMut, component::Component, entity::Entity,
    hierarchy::ChildOf, world::EntityWorldMut,
};
use nalgebra::Matrix4;
use renderer::RenderNode;

use matcha_ecs::{
    components::{
        input::{Pickable, PointerDispatch, PointerInput, PointerPhase},
        layout::GlobalTransform,
        render::{RenderCtx, RenderItem},
        view::Key,
    },
    layout::{Constraints, Layout, LayoutCtx, LayoutDispatch},
    view::Widget,
};

use crate::{
    color_rect::RectGeometry,
    scroll_view::{
        geometry::{self, Axis, ScrollbarStyle},
        DragAnchor, ScrollAxes, ScrollState,
    },
    shape::{rounded_rect_node, ShapeCtx},
};

/// Marks an entity as a scrollbar rather than scrollable content.
///
/// [`ScrollViewLayout::arrange`](super::ScrollViewLayout) partitions its
/// children on this, so bars may be declared in any order relative to the
/// content — or omitted entirely, for a view driven only by the wheel.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub struct ScrollbarSlot(pub Axis);

/// A scrollbar's axis and styling; also its `Layout` impl.
#[derive(Component, Clone, Copy, PartialEq, Debug)]
pub struct ScrollbarLayout {
    pub axis: Axis,
    pub style: ScrollbarStyle,
}

/// A scroll thumb's axis and styling.
#[derive(Component, Clone, Copy, PartialEq, Debug)]
pub struct ScrollThumbStyle {
    pub axis: Axis,
    pub style: ScrollbarStyle,
}

/// Copy the enclosing scroll view's state down onto this entity, so reading it
/// later costs no tree walking.
///
/// Safe to do once at spawn: `ScrollState` is created in the view's `bundle()`
/// and never replaced by `patch`, so the shared handle stays valid for as long
/// as the entity lives — and a rebuilt `ScrollView` despawns its whole subtree,
/// taking these copies with it.
fn inherit_from_scroll_view(entity: &mut EntityWorldMut) {
    let Some(parent) = entity.get::<ChildOf>().map(|c| c.parent()) else {
        return;
    };
    let inherited = entity.world_scope(|world| {
        let state = world.get::<ScrollState>(parent)?.clone();
        let axes = world.get::<ScrollAxes>(parent).copied()?;
        Some((state, axes))
    });
    if let Some((state, axes)) = inherited {
        entity.insert((state, axes));
    }
}

/// Read the pieces a bar's handler needs. `None` while this bar has nothing to
/// scroll, which is also when it has no thumb to interact with.
fn bar_context(entity: &EntityWorldMut) -> Option<(ScrollState, ScrollAxes)> {
    let state = entity.get::<ScrollState>().cloned()?;
    let axes = entity.get::<ScrollAxes>().copied()?;
    Some((state, axes))
}

// ---------------------------------------------------------------------------
// Scrollbar (the track)
// ---------------------------------------------------------------------------

/// The track a [`ScrollThumb`] slides along. Laid out by the enclosing scroll
/// view to exactly the track rectangle, so its own box is its hit area.
pub struct Scrollbar {
    key: Key,
    axis: Axis,
    style: ScrollbarStyle,
}

impl Scrollbar {
    pub fn new(axis: Axis, style: ScrollbarStyle) -> Self {
        Self {
            key: Key::Auto,
            axis,
            style,
        }
    }

    pub fn key(mut self, key: impl Into<Key>) -> Self {
        self.key = key.into();
        self
    }

    fn layout(&self) -> ScrollbarLayout {
        ScrollbarLayout {
            axis: self.axis,
            style: self.style,
        }
    }
}

fn track_render_item(shape: ShapeCtx, style: ScrollbarStyle) -> RenderItem {
    RenderItem::new(move |ctx: &RenderCtx| match style.track_color {
        Some(color) => rounded_rect_node(ctx, &shape, ctx.size, style.radius, color),
        None => RenderNode::new(),
    })
}

/// Clicking the track pages towards the click, the way a desktop scrollbar
/// does.
fn on_track_pointer(entity: &mut EntityWorldMut, input: &PointerInput) -> bool {
    let PointerPhase::Press { .. } = input.phase else {
        // Dragging is handled once, by the scroll view — see its `on_pointer`.
        return false;
    };
    let Some(layout) = entity.get::<ScrollbarLayout>().copied() else {
        return false;
    };
    let Some((state, axes)) = bar_context(entity) else {
        return false;
    };

    // A press anywhere on the bar starts a new interaction, so any thumb drag
    // that was in progress is over.
    state.set_drag(None);

    let geom = state.geometry();
    let Some(metrics) = geometry::metrics(layout.axis, &geom, axes.0, &layout.style) else {
        return false;
    };

    let main = layout.axis.main();
    // `local_pos` is relative to the track, and the track is this entity's box.
    let thumb_start = metrics.thumb[main] - metrics.track[main];
    let page = geom.viewport[main];

    let mut target = state.offset();
    if input.local_pos[main] < thumb_start {
        target[main] -= page;
    } else {
        target[main] += page;
    }
    state.set_offset(target);

    // Consumed either way: the press landed on the bar, so it must not also
    // reach the content behind it.
    true
}

impl Widget for Scrollbar {
    fn key(&self) -> Key {
        self.key
    }

    fn bundle(&self) -> impl Bundle {
        (
            ScrollbarSlot(self.axis),
            self.layout(),
            // Replaced in `after_spawn` by the enclosing view's shared handle;
            // `bundle()`'s type is fixed, so the slot has to exist up front.
            ScrollState::default(),
            ScrollAxes([false, false]),
            Pickable,
            LayoutDispatch::of::<ScrollbarLayout>(),
            PointerDispatch::new(on_track_pointer),
        )
    }

    fn after_spawn(&self, entity: &mut EntityWorldMut) {
        inherit_from_scroll_view(entity);
        let shape = ShapeCtx::get(entity);
        entity.insert(track_render_item(shape, self.style));
    }

    fn patch(&self, entity: &mut EntityWorldMut) {
        let changed = entity
            .get_mut::<ScrollbarLayout>()
            .is_some_and(|mut l| l.set_if_neq(self.layout()));
        if changed {
            let shape = ShapeCtx::get(entity);
            let item = track_render_item(shape, self.style);
            if let Some(mut existing) = entity.get_mut::<RenderItem>() {
                *existing = item;
            }
        }
    }
}

impl Layout for ScrollbarLayout {
    fn measure(&self, ctx: &mut LayoutCtx, me: Entity, _c: Constraints) -> [f32; 2] {
        // The enclosing view places this at the track rectangle regardless, so
        // what is reported here only matters for `LayoutOutput` — but it must
        // agree, or the hit area would not match what is drawn.
        let Some(metrics) = self.metrics(ctx, me) else {
            return [0.0, 0.0];
        };
        [metrics.track[2], metrics.track[3]]
    }

    fn arrange(&self, ctx: &mut LayoutCtx, me: Entity, _size: [f32; 2]) {
        let my_affine = ctx
            .world()
            .get::<GlobalTransform>(me)
            .map(|t| t.affine)
            .unwrap_or_else(Matrix4::identity);
        let Some(&thumb) = ctx.children(me).first() else {
            return;
        };

        let Some(metrics) = self.metrics(ctx, me) else {
            // Nothing to scroll: collapse the thumb rather than leaving it
            // where it last was.
            ctx.arrange_child(thumb, [0.0, 0.0], my_affine, [0.0, 0.0]);
            return;
        };

        // Thumb position relative to the track, since the track is our own box.
        let origin = [
            metrics.thumb[0] - metrics.track[0],
            metrics.thumb[1] - metrics.track[1],
        ];
        ctx.arrange_child(thumb, origin, my_affine, [metrics.thumb[2], metrics.thumb[3]]);
    }
}

impl ScrollbarLayout {
    /// This bar's geometry, or `None` when it has nothing to show.
    fn metrics(&self, ctx: &LayoutCtx, me: Entity) -> Option<geometry::ScrollbarMetrics> {
        let state = ctx.world().get::<ScrollState>(me)?;
        let axes = ctx.world().get::<ScrollAxes>(me)?;
        geometry::metrics(self.axis, &state.geometry(), axes.0, &self.style)
    }
}

// ---------------------------------------------------------------------------
// ScrollThumb
// ---------------------------------------------------------------------------

/// The draggable thumb. A layout leaf — its size and position are entirely the
/// enclosing [`Scrollbar`]'s decision.
pub struct ScrollThumb {
    key: Key,
    axis: Axis,
    style: ScrollbarStyle,
}

impl ScrollThumb {
    pub fn new(axis: Axis, style: ScrollbarStyle) -> Self {
        Self {
            key: Key::Auto,
            axis,
            style,
        }
    }

    pub fn key(mut self, key: impl Into<Key>) -> Self {
        self.key = key.into();
        self
    }

    fn info(&self) -> ScrollThumbStyle {
        ScrollThumbStyle {
            axis: self.axis,
            style: self.style,
        }
    }
}

fn thumb_render_item(shape: ShapeCtx, style: ScrollbarStyle) -> RenderItem {
    RenderItem::new(move |ctx: &RenderCtx| {
        rounded_rect_node(ctx, &shape, ctx.size, style.radius, style.thumb_color)
    })
}

/// Pressing the thumb records where it was grabbed. The drag itself is handled
/// by the scroll view, which stays under the pointer even when the cursor
/// wanders off the thumb.
fn on_thumb_pointer(entity: &mut EntityWorldMut, input: &PointerInput) -> bool {
    let PointerPhase::Press { .. } = input.phase else {
        return false;
    };
    let Some(info) = entity.get::<ScrollThumbStyle>().copied() else {
        return false;
    };
    let Some((state, axes)) = bar_context(entity) else {
        return false;
    };
    let Some(metrics) = geometry::metrics(info.axis, &state.geometry(), axes.0, &info.style) else {
        return false;
    };

    // `local_pos` is relative to the thumb, so adding the thumb's own start
    // puts the grab back into the viewport's coordinates, which is the space
    // the scroll view will deliver the drag in.
    let main = info.axis.main();
    let grab_thumb_start = metrics.thumb[main];
    state.set_drag(Some(DragAnchor {
        axis: info.axis,
        grab_pos: grab_thumb_start + input.local_pos[main],
        grab_thumb_start,
    }));

    // Consumed so the press does not also reach the content behind the bar.
    true
}

impl Widget for ScrollThumb {
    fn key(&self) -> Key {
        self.key
    }

    fn bundle(&self) -> impl Bundle {
        (
            self.info(),
            // A pure leaf: the bar dictates both size and position, so the
            // declared geometry is never consulted. Reuses `ColorRect`'s leaf
            // layout rather than introducing another no-op `Layout` impl.
            RectGeometry { w: 0.0, h: 0.0 },
            LayoutDispatch::of::<RectGeometry>(),
            ScrollState::default(),
            ScrollAxes([false, false]),
            Pickable,
            PointerDispatch::new(on_thumb_pointer),
        )
    }

    fn after_spawn(&self, entity: &mut EntityWorldMut) {
        // One hop up is the `Scrollbar`, which inherited these from the view.
        inherit_from_scroll_view(entity);
        let shape = ShapeCtx::get(entity);
        entity.insert(thumb_render_item(shape, self.style));
    }

    fn patch(&self, entity: &mut EntityWorldMut) {
        let changed = entity
            .get_mut::<ScrollThumbStyle>()
            .is_some_and(|mut i| i.set_if_neq(self.info()));
        if changed {
            let shape = ShapeCtx::get(entity);
            let item = thumb_render_item(shape, self.style);
            if let Some(mut existing) = entity.get_mut::<RenderItem>() {
                *existing = item;
            }
        }
    }
}
