//! Button — a clickable leaf widget: a solid-colour rect with a centred,
//! shaped text label, emitting an Elm-style message on click.
//!
//! `Message`/`OnClick<Msg>` now live in `matcha_ecs::components::input`
//! (moved there in M5 so core hit-test dispatch can read `OnClick<Msg>`
//! without knowing about `Button`); re-exported here for compatibility.
//!
//! The label is baked directly into `Button`'s own `RenderItem` (box quad +
//! shaped glyph quads composited into one node) rather than via a child
//! entity: a real child slot would need a new `Layout` impl that actually
//! arranges a child within the box (today's `RectGeometry::arrange` is a hard
//! leaf) and would break every existing `Button::new(label)` call site's
//! shape. Text shaping/rasterisation is reused from `crate::text` (the same
//! `FontCtx` resource, `shape`, `paint_tint_region`, `glyph_run_nodes` helpers
//! `Text` uses), so no shaping/stencil-cache logic is duplicated here.
//!
//! (A formerly-documented "known issue" here — intermittent corruption of
//! unrelated widgets while this widget rebuilt per click — was root-caused
//! and fixed on 2026-07-10: it was never atlas churn, but nondeterministic
//! instance ordering in `renderer`'s culling compute shader. See
//! `renderer/src/core_renderer/renderer_cull.wgsl` and CLAUDE.md.)

use bevy_ecs::{
    bundle::Bundle, change_detection::DetectChangesMut, component::Component, world::EntityWorldMut,
};
use nalgebra::{Matrix4, Vector3};

use matcha_ecs::{
    components::{
        input::{Message, OnClick, Pickable},
        render::{RenderCtx, RenderItem},
        view::Key,
    },
    layout::LayoutDispatch,
    view::Widget,
};

use crate::{
    color_rect::{solid_rect_node, RectColor, RectGeometry},
    text::{glyph_run_nodes, paint_tint_region, shape, FontCtx},
};

/// The button's label text.
#[derive(Component, Clone, PartialEq, Eq, Debug)]
pub struct ButtonLabel(pub String);

/// Draw-relevant label styling other than the string itself (kept separate
/// from `ButtonLabel` so `patch` can compare them independently, mirroring
/// `Text`'s `TextContent`/`TextStyle` split).
#[derive(Component, Clone, Copy, PartialEq, Debug)]
struct ButtonTextStyle {
    font_size: f32,
    label_color: [f32; 4],
}

/// A solid-colour rectangle with a centred text label that emits `Msg` on click.
pub struct Button<Msg: Message> {
    key: Key,
    label: String,
    msg: Option<Msg>,
    w: f32,
    h: f32,
    color: [f32; 4],
    font_size: f32,
    label_color: [f32; 4],
}

impl<Msg: Message> Button<Msg> {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            key: Key::Auto,
            label: label.into(),
            msg: None,
            w: 120.0,
            h: 40.0,
            color: [0.35, 0.35, 0.4, 1.0],
            font_size: 16.0,
            label_color: [1.0, 1.0, 1.0, 1.0],
        }
    }

    /// The message emitted when this button is clicked.
    pub fn on(mut self, msg: Msg) -> Self {
        self.msg = Some(msg);
        self
    }

    /// Override the default 120×40 size.
    pub fn size(mut self, w: f32, h: f32) -> Self {
        self.w = w;
        self.h = h;
        self
    }

    /// Override the default fill colour (components in `0.0..=1.0`).
    pub fn color(mut self, color: [f32; 4]) -> Self {
        self.color = color;
        self
    }

    /// Override the default 16px label size.
    pub fn font_size(mut self, font_size: f32) -> Self {
        self.font_size = font_size;
        self
    }

    /// Override the default white label colour (components in `0.0..=1.0`).
    pub fn label_color(mut self, color: [f32; 4]) -> Self {
        self.label_color = color;
        self
    }

    pub fn key(mut self, key: impl Into<Key>) -> Self {
        self.key = key.into();
        self
    }

    fn geometry(&self) -> RectGeometry {
        RectGeometry {
            w: self.w,
            h: self.h,
        }
    }

    fn text_style(&self) -> ButtonTextStyle {
        ButtonTextStyle {
            font_size: self.font_size,
            label_color: self.label_color,
        }
    }

    /// Build a fresh `RenderItem` for `entity`, fetching (or lazily
    /// inserting) the shared `FontCtx` resource. Shared by `after_spawn` and
    /// `patch`, the two places a `Button` entity's `RenderItem` gets
    /// (re)built (it needs world access for `FontCtx`, so unlike `ColorRect`
    /// it cannot be built inside `bundle()`).
    fn rebuild_render_item(&self, entity: &mut EntityWorldMut) -> RenderItem {
        let font_ctx = entity.world_scope(|world| world.get_resource_or_insert_with(FontCtx::new).clone());
        button_render_item(
            font_ctx,
            self.color,
            self.label.clone(),
            self.font_size,
            self.label_color,
        )
    }
}

/// Build a `RenderItem` compositing a solid box with a single-line, centred,
/// shaped text label on top (no word-wrap — a button label is one line).
/// Drawn at the layout-allocated size (`ctx.size`), not the declared one.
fn button_render_item(
    font_ctx: FontCtx,
    box_color: [f32; 4],
    label: String,
    font_size: f32,
    label_color: [f32; 4],
) -> RenderItem {
    RenderItem::new(move |ctx: &RenderCtx| {
        let [w, h] = ctx.size;
        let mut node = solid_rect_node(ctx, w, h, box_color);

        let layout = shape(&font_ctx, &label, font_size, f32::MAX);
        let Some(tint_region) = paint_tint_region(ctx, label_color) else {
            return node;
        };

        let offset = Matrix4::new_translation(&Vector3::new(
            ((w - layout.total_width) / 2.0).max(0.0),
            ((h - layout.total_height) / 2.0).max(0.0),
            0.0,
        ));
        for (glyph_node, transform) in glyph_run_nodes(&font_ctx, ctx, &layout, &tint_region) {
            node.push_child(glyph_node, offset * transform);
        }

        node
    })
}

impl<Msg: Message> Widget for Button<Msg> {
    fn key(&self) -> Key {
        self.key
    }

    fn bundle(&self) -> impl Bundle {
        (
            ButtonLabel(self.label.clone()),
            self.text_style(),
            OnClick(self.msg),
            self.geometry(),
            RectColor(self.color),
            LayoutDispatch::of::<RectGeometry>(),
            Pickable,
        )
    }

    fn after_spawn(&self, entity: &mut EntityWorldMut) {
        let item = self.rebuild_render_item(entity);
        entity.insert(item);
    }

    fn patch(&self, entity: &mut EntityWorldMut) {
        let mut changed = false;
        if let Some(mut label) = entity.get_mut::<ButtonLabel>() {
            changed |= label.set_if_neq(ButtonLabel(self.label.clone()));
        }
        if let Some(mut style) = entity.get_mut::<ButtonTextStyle>() {
            changed |= style.set_if_neq(self.text_style());
        }
        if let Some(mut on_click) = entity.get_mut::<OnClick<Msg>>() {
            on_click.set_if_neq(OnClick(self.msg));
        }

        let geometry = self.geometry();
        if let Some(mut g) = entity.get_mut::<RectGeometry>() {
            changed |= g.set_if_neq(geometry);
        }
        if let Some(mut c) = entity.get_mut::<RectColor>() {
            changed |= c.set_if_neq(RectColor(self.color));
        }

        if changed {
            let item = self.rebuild_render_item(entity);
            if let Some(mut existing) = entity.get_mut::<RenderItem>() {
                *existing = item;
            }
        }
    }
}
