//! `Checkbox` — a clickable leaf widget rendering a bordered box, filled when
//! `checked`, emitting an Elm-style message on click.
//!
//! Fully declarative like every other widget here: the app passes the
//! current `checked` bool on every `view()` call (compared via `patch`'s
//! `set_if_neq`, same pattern as `Button`'s `.color()`) — there is no
//! internal widget-side toggle state. A text label ("Remember me") is
//! deliberately not part of this widget; compose one externally (e.g.
//! `Row > (Checkbox, Text)`), matching HTML where `<input type=checkbox>` and
//! its `<label>` are siblings, not one element.
//!
//! (A formerly-documented "known issue" here — intermittent corruption of
//! unrelated widgets while this widget rebuilt per toggle — was root-caused
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

use crate::color_rect::{solid_rect_node, RectGeometry};

/// Draw-relevant checkbox state, tracked so `patch` can detect changes and
/// rebuild the cached render item.
#[derive(Component, Clone, Copy, PartialEq, Debug)]
struct CheckboxState {
    checked: bool,
    border_color: [f32; 4],
    fill_color: [f32; 4],
    border_width: f32,
}

/// A `size`×`size` checkbox: an outer bordered box, filled with `fill_color`
/// (inset by `border_width`) when `checked`.
pub struct Checkbox<Msg: Message> {
    key: Key,
    checked: bool,
    size: f32,
    border_color: [f32; 4],
    fill_color: [f32; 4],
    border_width: f32,
    msg: Option<Msg>,
}

impl<Msg: Message> Checkbox<Msg> {
    pub fn new(checked: bool) -> Self {
        Self {
            key: Key::Auto,
            checked,
            size: 20.0,
            border_color: [0.6, 0.6, 0.65, 1.0],
            fill_color: [0.25, 0.5, 0.9, 1.0],
            border_width: 2.0,
            msg: None,
        }
    }

    /// The message emitted when this checkbox is clicked.
    pub fn on(mut self, msg: Msg) -> Self {
        self.msg = Some(msg);
        self
    }

    /// Override the default 20px size.
    pub fn size(mut self, size: f32) -> Self {
        self.size = size;
        self
    }

    /// Override the default border colour (components in `0.0..=1.0`).
    pub fn border_color(mut self, color: [f32; 4]) -> Self {
        self.border_color = color;
        self
    }

    /// Override the default fill colour (components in `0.0..=1.0`).
    pub fn fill_color(mut self, color: [f32; 4]) -> Self {
        self.fill_color = color;
        self
    }

    /// Override the default 2px border width.
    pub fn border_width(mut self, width: f32) -> Self {
        self.border_width = width;
        self
    }

    pub fn key(mut self, key: impl Into<Key>) -> Self {
        self.key = key.into();
        self
    }

    fn geometry(&self) -> RectGeometry {
        RectGeometry {
            w: self.size,
            h: self.size,
        }
    }

    fn state(&self) -> CheckboxState {
        CheckboxState {
            checked: self.checked,
            border_color: self.border_color,
            fill_color: self.fill_color,
            border_width: self.border_width,
        }
    }
}

/// Build a `RenderItem` compositing the outer border box with an inset fill
/// box on top, only when `checked`. Drawn at the layout-allocated size
/// (`ctx.size`) — normally the declared square, but a stretching parent
/// layout may allocate a non-square box, and paint must track layout.
fn checkbox_render_item(state: CheckboxState) -> RenderItem {
    RenderItem::new(move |ctx: &RenderCtx| {
        let [w, h] = ctx.size;
        let mut node = solid_rect_node(ctx, w, h, state.border_color);
        if state.checked {
            let inset = state.border_width;
            let fill_w = (w - inset * 2.0).max(0.0);
            let fill_h = (h - inset * 2.0).max(0.0);
            let fill_node = solid_rect_node(ctx, fill_w, fill_h, state.fill_color);
            let transform = Matrix4::new_translation(&Vector3::new(inset, inset, 0.0));
            node.push_child(fill_node, transform);
        }
        node
    })
}

impl<Msg: Message> Widget for Checkbox<Msg> {
    fn key(&self) -> Key {
        self.key
    }

    fn bundle(&self) -> impl Bundle {
        (
            self.geometry(),
            self.state(),
            OnClick(self.msg),
            LayoutDispatch::of::<RectGeometry>(),
            Pickable,
            checkbox_render_item(self.state()),
        )
    }

    fn patch(&self, entity: &mut EntityWorldMut) {
        let mut changed = false;
        if let Some(mut g) = entity.get_mut::<RectGeometry>() {
            changed |= g.set_if_neq(self.geometry());
        }
        if let Some(mut s) = entity.get_mut::<CheckboxState>() {
            changed |= s.set_if_neq(self.state());
        }
        if let Some(mut on_click) = entity.get_mut::<OnClick<Msg>>() {
            on_click.set_if_neq(OnClick(self.msg));
        }

        if changed {
            let item = checkbox_render_item(self.state());
            if let Some(mut existing) = entity.get_mut::<RenderItem>() {
                *existing = item;
            }
        }
    }
}
