//! Text — a leaf widget carrying a string.

use bevy_ecs::{
    bundle::Bundle, change_detection::DetectChangesMut, component::Component, world::EntityWorldMut,
};

use matcha_ecs::{components::view::Key, view::Widget};

/// The displayed string.
#[derive(Component, Clone, PartialEq, Eq, Debug)]
pub struct TextContent(pub String);

pub struct Text {
    key: Key,
    content: String,
}

impl Text {
    pub fn new(content: impl Into<String>) -> Self {
        Self {
            key: Key::Auto,
            content: content.into(),
        }
    }

    pub fn key(mut self, key: impl Into<Key>) -> Self {
        self.key = key.into();
        self
    }
}

impl Widget for Text {
    fn key(&self) -> Key {
        self.key
    }

    fn bundle(&self) -> impl Bundle {
        TextContent(self.content.clone())
    }

    fn patch(&self, entity: &mut EntityWorldMut) {
        if let Some(mut c) = entity.get_mut::<TextContent>() {
            c.set_if_neq(TextContent(self.content.clone()));
        }
    }
}
