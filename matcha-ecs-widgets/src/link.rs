//! `Link` — click-dispatching inline text: underline + accent colour, no
//! background box. Distinct from `Button` because it has no box and is meant
//! to sit inline within text flow (e.g. as a `Row` sibling next to `Text`)
//! rather than as a boxed control.
//!
//! Wraps a [`RichText`] by composition and delegates every `Widget` method to
//! it, rather than reimplementing shaping or duplicating `Text`'s pipeline —
//! `RichText` already renders real underline decoration
//! (`RichText::underline`), which `Text` does not. `Link`'s clickable area is
//! always its whole entity bounding box (hit-testing is strictly per-entity,
//! never per-glyph — see `matcha-ecs/src/input.rs`), so v1 stays single-style
//! (no multi-span `RichText::span` support): spans would add API surface with
//! no hit-test benefit.
//!
//! `Link` is a same-box-model leaf like every other widget here — it does not
//! achieve true CSS inline reflow (text wrapping around an anchor embedded
//! mid-paragraph). "Inline" here means composing it as a sibling inside a
//! `Row` alongside `Text`/other `Link`s to approximate a line of mixed
//! plain/clickable text.

use bevy_ecs::{bundle::Bundle, change_detection::DetectChangesMut, world::EntityWorldMut};

use matcha_ecs::{
    components::{
        focus::FocusPolicy,
        input::{Message, OnClick, Pickable},
        view::Key,
    },
    view::Widget,
};

use crate::rich_text::RichText;

/// Default link accent colour (a CSS-familiar blue).
const LINK_ACCENT: [f32; 4] = [0.02, 0.4, 0.84, 1.0];

/// Click-dispatching inline text, styled by default as underlined
/// accent-coloured text with no background box.
pub struct Link<Msg: Message> {
    key: Key,
    text: RichText,
    msg: Option<Msg>,
}

impl<Msg: Message> Link<Msg> {
    pub fn new(content: impl Into<String>) -> Self {
        Self {
            key: Key::Auto,
            text: RichText::new(content).color(LINK_ACCENT).underline(true),
            msg: None,
        }
    }

    /// The message emitted when this link is clicked.
    pub fn on(mut self, msg: Msg) -> Self {
        self.msg = Some(msg);
        self
    }

    /// Override the default accent colour (components in `0.0..=1.0`).
    pub fn color(mut self, color: [f32; 4]) -> Self {
        self.text = self.text.color(color);
        self
    }

    /// Override the default 16px text size.
    pub fn font_size(mut self, font_size: f32) -> Self {
        self.text = self.text.font_size(font_size);
        self
    }

    /// Override whether the link is underlined (default `true`).
    pub fn underline(mut self, enabled: bool) -> Self {
        self.text = self.text.underline(enabled);
        self
    }

    pub fn key(mut self, key: impl Into<Key>) -> Self {
        self.key = key.into();
        self
    }
}

impl<Msg: Message> Widget for Link<Msg> {
    fn key(&self) -> Key {
        self.key
    }

    fn bundle(&self) -> impl Bundle {
        (
            self.text.bundle(),
            OnClick(self.msg.clone()),
            Pickable,
            FocusPolicy::Normal,
        )
    }

    fn after_spawn(&self, entity: &mut EntityWorldMut) {
        Widget::after_spawn(&self.text, entity);
    }

    fn patch(&self, entity: &mut EntityWorldMut) {
        Widget::patch(&self.text, entity);
        if let Some(mut on_click) = entity.get_mut::<OnClick<Msg>>() {
            on_click.set_if_neq(OnClick(self.msg.clone()));
        }
    }
}
