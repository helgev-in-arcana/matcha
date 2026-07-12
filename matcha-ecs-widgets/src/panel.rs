//! `Panel` — a fixed-size, single-child container with CSS
//! border+background-color box-model semantics (a `div`'s decorated box
//! equivalent).
//!
//! Structurally mirrors `Padding` (single-child inset by `border_width`) but,
//! unlike `Padding`, has a **fixed** own size (like `ColorRect`) rather than
//! auto-sizing to its child. Draws a `border_color` box plus an inset
//! `background_color` box via `solid_rect_node` (twice — same technique as
//! `Checkbox`'s border/fill); the child (a separate `ViewChildren` entity)
//! paints on top automatically — `matcha-ecs/src/render.rs`'s
//! `extract_recursive` already supports an entity having its own `RenderItem`
//! *and* children with their own `RenderItem`s, painting parent-then-children.

use bevy_ecs::{
    bundle::Bundle, change_detection::DetectChangesMut, component::Component, entity::Entity,
    world::EntityWorldMut,
};
use nalgebra::{Matrix4, Vector3};

use matcha_ecs::{
    components::{
        layout::GlobalTransform,
        render::{RenderCtx, RenderItem},
        view::Key,
    },
    layout::{Constraints, Layout, LayoutCtx, LayoutDispatch},
    view::Widget,
};

use crate::color_rect::solid_rect_node;

/// A [`Panel`]'s fixed size and border inset — doubles as both data and the
/// `Layout` impl, mirroring `PaddingLayout`.
#[derive(Component, Clone, Copy, PartialEq, Debug)]
pub struct PanelLayout {
    pub w: f32,
    pub h: f32,
    pub border_width: f32,
}

/// Draw-relevant colours, carried separately so `patch` can detect
/// colour-only changes without touching `PanelLayout`.
#[derive(Component, Clone, Copy, PartialEq, Debug)]
struct PanelColors {
    background_color: [f32; 4],
    border_color: [f32; 4],
}

/// A fixed `w`×`h` single-child container with an optional border and
/// background fill.
pub struct Panel {
    key: Key,
    w: f32,
    h: f32,
    background_color: [f32; 4],
    border_color: [f32; 4],
    border_width: f32,
}

impl Panel {
    pub fn new(w: f32, h: f32) -> Self {
        Self {
            key: Key::Auto,
            w,
            h,
            background_color: [0.0, 0.0, 0.0, 0.0],
            border_color: [0.5, 0.5, 0.55, 1.0],
            border_width: 0.0,
        }
    }

    /// Override the default transparent background (components in `0.0..=1.0`).
    pub fn background_color(mut self, color: [f32; 4]) -> Self {
        self.background_color = color;
        self
    }

    /// Override the default mid-gray border colour (components in `0.0..=1.0`).
    pub fn border_color(mut self, color: [f32; 4]) -> Self {
        self.border_color = color;
        self
    }

    /// Override the default 0px (no border) width.
    pub fn border_width(mut self, width: f32) -> Self {
        self.border_width = width;
        self
    }

    pub fn key(mut self, key: impl Into<Key>) -> Self {
        self.key = key.into();
        self
    }

    fn layout(&self) -> PanelLayout {
        PanelLayout {
            w: self.w,
            h: self.h,
            border_width: self.border_width,
        }
    }

    fn colors(&self) -> PanelColors {
        PanelColors {
            background_color: self.background_color,
            border_color: self.border_color,
        }
    }
}

/// Build a `RenderItem` compositing the border box with an inset background
/// box on top.
fn panel_render_item(layout: PanelLayout, colors: PanelColors) -> RenderItem {
    RenderItem::new(move |ctx: &RenderCtx| {
        let mut node = solid_rect_node(ctx, layout.w, layout.h, colors.border_color);
        let inset = layout.border_width;
        let inner_w = (layout.w - inset * 2.0).max(0.0);
        let inner_h = (layout.h - inset * 2.0).max(0.0);
        let fill = solid_rect_node(ctx, inner_w, inner_h, colors.background_color);
        let transform = Matrix4::new_translation(&Vector3::new(inset, inset, 0.0));
        node.push_child(fill, transform);
        node
    })
}

impl Widget for Panel {
    fn key(&self) -> Key {
        self.key
    }

    fn bundle(&self) -> impl Bundle {
        (
            self.layout(),
            self.colors(),
            LayoutDispatch::of::<PanelLayout>(),
            panel_render_item(self.layout(), self.colors()),
        )
    }

    fn patch(&self, entity: &mut EntityWorldMut) {
        let mut changed = false;
        if let Some(mut l) = entity.get_mut::<PanelLayout>() {
            changed |= l.set_if_neq(self.layout());
        }
        if let Some(mut c) = entity.get_mut::<PanelColors>() {
            changed |= c.set_if_neq(self.colors());
        }
        if changed {
            let item = panel_render_item(self.layout(), self.colors());
            if let Some(mut existing) = entity.get_mut::<RenderItem>() {
                *existing = item;
            }
        }
    }
}

impl Layout for PanelLayout {
    fn measure(&self, _ctx: &mut LayoutCtx, _me: Entity, c: Constraints) -> [f32; 2] {
        [
            self.w.clamp(c.min_width(), c.max_width()),
            self.h.clamp(c.min_height(), c.max_height()),
        ]
    }

    fn arrange(&self, ctx: &mut LayoutCtx, me: Entity, size: [f32; 2]) {
        let my_affine = ctx
            .world()
            .get::<GlobalTransform>(me)
            .map(|t| t.affine)
            .unwrap_or_else(Matrix4::identity);

        if let Some(&child) = ctx.children(me).first() {
            let inset = self.border_width;
            let inner_size = [(size[0] - inset * 2.0).max(0.0), (size[1] - inset * 2.0).max(0.0)];
            let child_c = Constraints::from_max_size(inner_size);
            let child_size = ctx.measure_child(child, child_c);
            // Unlike `Padding` (which sizes itself to its child, so the child
            // always fills the inner area exactly), `Panel` is fixed-size:
            // the inner area can be larger than the child. Centre the child
            // in the leftover space rather than anchoring it top-left.
            let origin = [
                inset + ((inner_size[0] - child_size[0]) / 2.0).max(0.0),
                inset + ((inner_size[1] - child_size[1]) / 2.0).max(0.0),
            ];
            ctx.arrange_child(child, origin, my_affine, child_size);
        }
    }
}
