//! Button — a leaf widget carrying a label and an Elm-style message.
//!
//! The message is a `Copy` enum stored as a component; a dispatch system (not
//! implemented yet) would drain these on click. Keeping `OnClick<M>` always
//! present (as `Option<M>`) fixes the archetype regardless of props, so a
//! `Button<M>` is a single stable archetype.

use bevy_ecs::{
    bundle::Bundle, change_detection::DetectChangesMut, component::Component, world::EntityWorldMut,
};

use matcha_ecs::{components::view::Key, view::Widget};

/// Marker bound for Elm-style messages: a cheap, copyable value.
pub trait Message: Copy + PartialEq + Send + Sync + 'static {}
impl<T: Copy + PartialEq + Send + Sync + 'static> Message for T {}

/// The button's label.
#[derive(Component, Clone, PartialEq, Eq, Debug)]
pub struct ButtonLabel(pub String);

/// The message emitted on click, if any.
#[derive(Component, Clone, Copy, PartialEq, Debug)]
pub struct OnClick<M: Message>(pub Option<M>);

pub struct Button<M: Message> {
    key: Key,
    label: String,
    msg: Option<M>,
}

impl<M: Message> Button<M> {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            key: Key::Auto,
            label: label.into(),
            msg: None,
        }
    }

    pub fn on(mut self, msg: M) -> Self {
        self.msg = Some(msg);
        self
    }

    pub fn key(mut self, key: impl Into<Key>) -> Self {
        self.key = key.into();
        self
    }
}

impl<M: Message> Widget for Button<M> {
    fn key(&self) -> Key {
        self.key
    }

    fn bundle(&self) -> impl Bundle {
        (ButtonLabel(self.label.clone()), OnClick(self.msg))
    }

    fn patch(&self, entity: &mut EntityWorldMut) {
        if let Some(mut label) = entity.get_mut::<ButtonLabel>() {
            label.set_if_neq(ButtonLabel(self.label.clone()));
        }
        if let Some(mut on_click) = entity.get_mut::<OnClick<M>>() {
            on_click.set_if_neq(OnClick(self.msg));
        }
    }
}
