//! Button — a clickable leaf widget: a solid-colour rect (label drawing
//! arrives in M6) that emits an Elm-style message on click.
//!
//! `Message`/`OnClick<Msg>` now live in `matcha_ecs::components::input`
//! (moved there in M5 so core hit-test dispatch can read `OnClick<Msg>`
//! without knowing about `Button`); re-exported here for compatibility.

use bevy_ecs::{
    bundle::Bundle, change_detection::DetectChangesMut, component::Component, world::EntityWorldMut,
};

use matcha_ecs::{
    components::{
        input::{HitTestEnabled, Message, OnClick},
        render::RenderItem,
        view::Key,
    },
    layout::LayoutDispatch,
    view::Widget,
};

use crate::color_rect::{solid_rect_render_item, RectGeometry};

/// The button's label (not yet drawn — see M6).
#[derive(Component, Clone, PartialEq, Eq, Debug)]
pub struct ButtonLabel(pub String);

/// A solid-colour rectangle that emits `Msg` on click.
pub struct Button<Msg: Message> {
    key: Key,
    label: String,
    msg: Option<Msg>,
    w: f32,
    h: f32,
    color: [f32; 4],
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
}

impl<Msg: Message> Widget for Button<Msg> {
    fn key(&self) -> Key {
        self.key
    }

    fn bundle(&self) -> impl Bundle {
        (
            ButtonLabel(self.label.clone()),
            OnClick(self.msg),
            self.geometry(),
            LayoutDispatch::of::<RectGeometry>(),
            HitTestEnabled,
            solid_rect_render_item(self.w, self.h, self.color),
        )
    }

    fn patch(&self, entity: &mut EntityWorldMut) {
        if let Some(mut label) = entity.get_mut::<ButtonLabel>() {
            label.set_if_neq(ButtonLabel(self.label.clone()));
        }
        if let Some(mut on_click) = entity.get_mut::<OnClick<Msg>>() {
            on_click.set_if_neq(OnClick(self.msg));
        }

        let geometry = self.geometry();
        let mut changed = false;
        if let Some(mut g) = entity.get_mut::<RectGeometry>() {
            changed |= g.set_if_neq(geometry);
        }
        // `RectColor` is not carried by `Button` (unlike `ColorRect`), so a
        // colour-only change cannot be detected here; only geometry changes
        // rebuild the cached render item. Acceptable for v0.1 — button colour
        // is set once at construction in every current usage.
        if changed {
            let item = solid_rect_render_item(self.w, self.h, self.color);
            if let Some(mut existing) = entity.get_mut::<RenderItem>() {
                *existing = item;
            }
        }
    }
}
