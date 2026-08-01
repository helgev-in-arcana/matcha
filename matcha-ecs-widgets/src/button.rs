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
        focus::FocusPolicy,
        input::{Message, OnClick, Pickable},
        render::{RenderCtx, RenderItem},
        view::Key,
    },
    layout::LayoutDispatch,
    view::Widget,
};

use crate::{
    animation::Easing,
    color_rect::{solid_rect_node, RectColor, RectGeometry},
    interaction::{interaction_cell, ColorCell, InteractionColors},
    sizing::Sizing,
    text::{glyph_run_nodes, paint_tint_region, shape, FontCtx},
};
use std::time::Duration;

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
    focus_ring_color: [f32; 4],
}

/// A solid-colour rectangle with a centred text label that emits `Msg` on click.
pub struct Button<Msg: Message> {
    key: Key,
    sizing: Sizing,
    label: String,
    msg: Option<Msg>,
    w: f32,
    h: f32,
    color: [f32; 4],
    hover_color: Option<[f32; 4]>,
    active_color: Option<[f32; 4]>,
    transition: Option<(Duration, Easing)>,
    font_size: f32,
    label_color: [f32; 4],
    focus_ring_color: [f32; 4],
}

impl<Msg: Message> Button<Msg> {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            key: Key::Auto,
            sizing: Sizing::default(),
            label: label.into(),
            msg: None,
            w: 120.0,
            h: 40.0,
            color: [0.35, 0.35, 0.4, 1.0],
            hover_color: None,
            active_color: None,
            transition: None,
            font_size: 16.0,
            label_color: [1.0, 1.0, 1.0, 1.0],
            focus_ring_color: [0.45, 0.7, 1.0, 1.0],
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

    /// Fill colour while the pointer is over the button (CSS `:hover`).
    pub fn hover_color(mut self, color: [f32; 4]) -> Self {
        self.hover_color = Some(color);
        self
    }

    /// Fill colour while the button is held down (CSS `:active`). Falls back to
    /// the hover colour when unset.
    pub fn active_color(mut self, color: [f32; 4]) -> Self {
        self.active_color = Some(color);
        self
    }

    /// Ease between the state colours over `duration` instead of snapping
    /// (CSS `transition`).
    ///
    /// Needs `matcha_ecs_widgets::default_systems()` registered with
    /// `UiEcs::with_pre_layout_systems`; without it the colour stays at the
    /// base one.
    pub fn transition(mut self, duration: Duration, easing: Easing) -> Self {
        self.transition = Some((duration, easing));
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

    /// Override the default ring colour drawn around the button while it holds
    /// focus (components in `0.0..=1.0`).
    pub fn focus_ring_color(mut self, color: [f32; 4]) -> Self {
        self.focus_ring_color = color;
        self
    }

    crate::sizing_builders!();

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

    fn colors(&self) -> InteractionColors {
        InteractionColors {
            base: self.color,
            hover: self.hover_color,
            active: self.active_color,
            transition: self.transition,
        }
    }

    fn text_style(&self) -> ButtonTextStyle {
        ButtonTextStyle {
            font_size: self.font_size,
            label_color: self.label_color,
            focus_ring_color: self.focus_ring_color,
        }
    }

    /// Build a fresh `RenderItem` for `entity`, fetching (or lazily
    /// inserting) the shared `FontCtx` resource. Shared by `after_spawn` and
    /// `patch`, the two places a `Button` entity's `RenderItem` gets
    /// (re)built (it needs world access for `FontCtx`, so unlike `ColorRect`
    /// it cannot be built inside `bundle()`).
    fn rebuild_render_item(&self, entity: &mut EntityWorldMut) -> RenderItem {
        let font_ctx = entity.world_scope(|world| world.get_resource_or_insert_with(FontCtx::new).clone());
        // The cell survives this rebuild, so an in-flight hover transition is
        // not restarted by an unrelated prop change.
        let box_color = interaction_cell(entity, self.colors());
        button_render_item(
            font_ctx,
            box_color,
            self.label.clone(),
            self.font_size,
            self.label_color,
            self.focus_ring_color,
        )
    }
}

/// Width of the ring drawn around a focused button.
const FOCUS_RING_WIDTH: f32 = 2.0;

/// Build a `RenderItem` compositing a solid box with a single-line, centred,
/// shaped text label on top (no word-wrap — a button label is one line).
/// Drawn at the layout-allocated size (`ctx.size`), not the declared one.
///
/// The fill colour comes from a [`ColorCell`] rather than being captured
/// directly, so `:hover`/`:active` (and any transition between them) reach the
/// builder without a rebuild of the closure itself.
///
/// When the button holds focus (`ctx.focused`) the box is drawn as a ring in
/// `focus_ring_color` with the normal fill inset inside it. `focus.rs`'s
/// `sync_focus_components` invalidates the cached node on every focus
/// transition, so this is re-evaluated exactly when it changes.
fn button_render_item(
    font_ctx: FontCtx,
    box_color: ColorCell,
    label: String,
    font_size: f32,
    label_color: [f32; 4],
    focus_ring_color: [f32; 4],
) -> RenderItem {
    RenderItem::new(move |ctx: &RenderCtx| {
        let [w, h] = ctx.size;
        // Read live: `advance_interaction_colors` writes this between frames
        // and invalidates the cached node, so each rebuild sees the current
        // step of the hover/press transition.
        let box_color = box_color.get();

        let mut node = if ctx.focused {
            // Ring underneath, fill inset on top — the same two-quad technique
            // `Panel` uses for its border.
            let mut ring = solid_rect_node(ctx, w, h, focus_ring_color);
            let inset = FOCUS_RING_WIDTH;
            let fill = solid_rect_node(
                ctx,
                (w - inset * 2.0).max(0.0),
                (h - inset * 2.0).max(0.0),
                box_color,
            );
            ring.push_child(
                fill,
                Matrix4::new_translation(&Vector3::new(inset, inset, 0.0)),
            );
            ring
        } else {
            solid_rect_node(ctx, w, h, box_color)
        };

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
            OnClick(self.msg.clone()),
            self.geometry(),
            RectColor(self.color),
            self.sizing,
            LayoutDispatch::of::<RectGeometry>(),
            Pickable,
            FocusPolicy::Normal,
        )
    }

    fn after_spawn(&self, entity: &mut EntityWorldMut) {
        let item = self.rebuild_render_item(entity);
        entity.insert(item);
    }

    fn patch(&self, entity: &mut EntityWorldMut) {
        self.sync_sizing(entity);
        // Unconditional: the builder reads the colours through the cell, so a
        // changed hover colour or transition takes effect with no rebuild.
        interaction_cell(entity, self.colors());

        let mut changed = false;
        if let Some(mut label) = entity.get_mut::<ButtonLabel>() {
            changed |= label.set_if_neq(ButtonLabel(self.label.clone()));
        }
        if let Some(mut style) = entity.get_mut::<ButtonTextStyle>() {
            changed |= style.set_if_neq(self.text_style());
        }
        if let Some(mut on_click) = entity.get_mut::<OnClick<Msg>>() {
            on_click.set_if_neq(OnClick(self.msg.clone()));
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
