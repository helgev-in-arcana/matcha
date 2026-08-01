//! `Panel` — a fixed-size, single-child container with CSS
//! border+background-color box-model semantics (a `div`'s decorated box
//! equivalent).
//!
//! Structurally mirrors `Padding` (single-child inset by the border) but,
//! unlike `Padding`, has a **fixed** own size (like `ColorRect`) rather than
//! auto-sizing to its child. Decoration is whatever `crate::box_style` can
//! express: background, border, corner radius, drop shadow. The child (a
//! separate `ViewChildren` entity) paints on top automatically, since
//! `matcha-ecs/src/render.rs`'s extract already supports an entity having its
//! own `RenderItem` *and* children with theirs, painting parent-then-children.

use bevy_ecs::{
    bundle::Bundle, change_detection::DetectChangesMut, component::Component, entity::Entity,
    world::EntityWorldMut,
};
use nalgebra::Matrix4;

use matcha_ecs::{
    components::{
        layout::{Clip, GlobalTransform},
        render::{RenderCtx, RenderItem},
        view::Key,
    },
    layout::{Constraints, Layout, LayoutCtx, LayoutDispatch, Measured},
    view::Widget,
};

use crate::box_style::{box_node, BoxShadow, BoxStyle, Corners, Sides};
use crate::shape::ShapeCtx;
use crate::sizing::Sizing;

/// A [`Panel`]'s fixed size and border inset — doubles as both data and the
/// `Layout` impl, mirroring `PaddingLayout`.
#[derive(Component, Clone, Copy, PartialEq, Debug)]
pub struct PanelLayout {
    pub w: f32,
    pub h: f32,
    /// Per-side border widths, which are what the child is inset by.
    pub border: Sides,
}

/// Draw-relevant decoration, carried separately so `patch` can detect an
/// appearance-only change without touching `PanelLayout`.
#[derive(Component, Clone, Copy, PartialEq, Debug)]
struct PanelStyle(BoxStyle);

/// A fixed `w`×`h` single-child container with an optional border and
/// background fill.
pub struct Panel {
    key: Key,
    sizing: Sizing,
    w: f32,
    h: f32,
    style: BoxStyle,
    clip: bool,
}

impl Panel {
    pub fn new(w: f32, h: f32) -> Self {
        Self {
            key: Key::Auto,
            sizing: Sizing::default(),
            w,
            h,
            style: BoxStyle {
                border_color: [0.5, 0.5, 0.55, 1.0],
                ..BoxStyle::default()
            },
            clip: false,
        }
    }

    /// Confine drawing (and clicking) to this panel's box — CSS
    /// `overflow: hidden`. A child laid out larger than the panel, or offset
    /// past its edge, is cut off instead of spilling over.
    ///
    /// Layout is unaffected: the child still measures and arranges exactly as
    /// it would without this, it simply does not paint outside. Clipping is to
    /// the panel's outer box, border included.
    pub fn clip(mut self, clip: bool) -> Self {
        self.clip = clip;
        self
    }

    /// Override the default transparent background (components in `0.0..=1.0`).
    pub fn background_color(mut self, color: [f32; 4]) -> Self {
        self.style.background = color;
        self
    }

    /// Override the default mid-gray border colour (components in `0.0..=1.0`).
    pub fn border_color(mut self, color: [f32; 4]) -> Self {
        self.style.border_color = color;
        self
    }

    /// Override the default 0px (no border) width.
    pub fn border_width(mut self, width: f32) -> Self {
        self.style.border = Sides::all(width);
        self
    }

    /// Per-side border widths, for a panel bordered on only some of them.
    pub fn borders(mut self, widths: Sides) -> Self {
        self.style.border = widths;
        self
    }

    /// Round every corner (CSS `border-radius`).
    pub fn radius(mut self, radius: f32) -> Self {
        self.style.radius = Corners::all(radius);
        self
    }

    /// Round each corner independently.
    pub fn corners(mut self, corners: Corners) -> Self {
        self.style.radius = corners;
        self
    }

    /// Cast a drop shadow (CSS `box-shadow`). Painted *outside* the panel's
    /// box, so it overlaps what is behind without affecting layout.
    pub fn shadow(mut self, shadow: BoxShadow) -> Self {
        self.style.shadow = Some(shadow);
        self
    }

    /// Everything `crate::box_style` can express, set at once.
    pub fn box_style(mut self, style: BoxStyle) -> Self {
        self.style = style;
        self
    }

    crate::sizing_builders!();

    pub fn key(mut self, key: impl Into<Key>) -> Self {
        self.key = key.into();
        self
    }

    fn layout(&self) -> PanelLayout {
        PanelLayout {
            w: self.w,
            h: self.h,
            border: self.style.border,
        }
    }

    /// `Clip` is a marker with no data, so it can't live in `bundle()`'s fixed
    /// return type conditionally — it is inserted and removed here instead, on
    /// spawn and on every patch, so toggling `.clip(..)` takes effect.
    fn sync_clip(&self, entity: &mut EntityWorldMut) {
        if self.clip {
            entity.insert(Clip);
        } else {
            entity.remove::<Clip>();
        }
    }
}

/// Build a `RenderItem` painting the panel's decoration at the
/// layout-allocated size (`ctx.size`) — the same size `PanelLayout::arrange`
/// places the child within, so paint and child placement can never disagree
/// even when a parent layout stretches the panel beyond its declared size.
fn panel_render_item(shape: ShapeCtx, style: BoxStyle) -> RenderItem {
    RenderItem::new(move |ctx: &RenderCtx| box_node(ctx, &shape, ctx.size, &style))
}

impl Widget for Panel {
    fn key(&self) -> Key {
        self.key
    }

    fn bundle(&self) -> impl Bundle {
        (
            self.layout(),
            PanelStyle(self.style),
            self.sizing,
            LayoutDispatch::of::<PanelLayout>(),
        )
    }

    fn after_spawn(&self, entity: &mut EntityWorldMut) {
        self.sync_clip(entity);
        // Built here rather than in `bundle()`: it needs the `ShapeCtx`
        // resource, which only world access can reach.
        let shape = ShapeCtx::get(entity);
        entity.insert(panel_render_item(shape, self.style));
    }

    fn patch(&self, entity: &mut EntityWorldMut) {
        self.sync_sizing(entity);
        self.sync_clip(entity);
        let mut changed = false;
        if let Some(mut l) = entity.get_mut::<PanelLayout>() {
            changed |= l.set_if_neq(self.layout());
        }
        if let Some(mut c) = entity.get_mut::<PanelStyle>() {
            changed |= c.set_if_neq(PanelStyle(self.style));
        }
        if changed {
            let shape = ShapeCtx::get(entity);
            let item = panel_render_item(shape, self.style);
            if let Some(mut existing) = entity.get_mut::<RenderItem>() {
                *existing = item;
            }
        }
    }
}

impl Layout for PanelLayout {
    fn measure(&self, ctx: &mut LayoutCtx, me: Entity, c: Constraints) -> Measured {
        // Fixed size: the child is not consulted (unlike `Padding`, which
        // sizes itself to its child). `Sizing` overrides or bounds the
        // declared `w`/`h`.
        let sizing = Sizing::of(ctx, me);
        let inner = sizing.content_constraints(c);
        let content = [
            self.w.clamp(inner.min_width(), inner.max_width()),
            self.h.clamp(inner.min_height(), inner.max_height()),
        ];
        sizing.measured(c, Measured::exact(content))
    }

    fn arrange(&self, ctx: &mut LayoutCtx, me: Entity, size: [f32; 2]) {
        let my_affine = ctx
            .world()
            .get::<GlobalTransform>(me)
            .map(|t| t.affine)
            .unwrap_or_else(Matrix4::identity);

        if let Some(&child) = ctx.children(me).first() {
            let inner_origin = [self.border.left, self.border.top];
            let inner_size = [
                (size[0] - self.border.left - self.border.right).max(0.0),
                (size[1] - self.border.top - self.border.bottom).max(0.0),
            ];
            let child_c = Constraints::from_max_size(inner_size);
            let child_size = ctx.measure_child_size(child, child_c);
            // Unlike `Padding` (which sizes itself to its child, so the child
            // always fills the inner area exactly), `Panel` is fixed-size:
            // the inner area can be larger than the child. Centre the child
            // in the leftover space rather than anchoring it top-left.
            let origin = [
                inner_origin[0] + ((inner_size[0] - child_size[0]) / 2.0).max(0.0),
                inner_origin[1] + ((inner_size[1] - child_size[1]) / 2.0).max(0.0),
            ];
            ctx.arrange_child(child, origin, my_affine, child_size);
        }
    }
}
